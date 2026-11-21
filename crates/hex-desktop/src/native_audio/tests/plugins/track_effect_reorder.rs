//! Signed same-Track reorder through the actual Resident callback.
use super::*;
use hex_project::{DeviceChainOwner, DeviceSelectionIds};
use hex_session::{
    device_gesture::{ParameterId, ParameterValue},
    service::{DeviceInsertOutcome, GraphEditOperation},
};

const QUANTUM: usize = 256;
const BRIDGE: u32 = 2 * QUANTUM as u32;
const TOLERANCE: f32 = 3e-6;

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_mixed_track_reorder_uses_one_live_cut_and_keeps_the_bed() {
    const RATE: u32 = 48_000;
    const FADE: usize = 240;
    const DELAY_QUANTA: usize = 6;
    const FRAMES: u32 = 131_072;
    const SIGNAL: i16 = 8192;

    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection(), "Fx", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: BRIDGE,
        }),
        RATE,
        directory.join("plugin-catalog.json"),
        8 * u64::from(FRAMES),
    );
    assert!(h.next().error.is_none());
    let mut applied = h.applied(0).0;
    let imported = h.run(Request::Import(files.constant_wav(
        "signal.wav",
        RATE,
        FRAMES,
        &[SIGNAL, 0],
    )));
    let track = imported.state.project.tracks()[0].id();
    applied = h.applied(applied).0;
    h.run(Request::Import(write_dry_bed(&files, RATE, FRAMES)));
    applied = h.applied(applied).0;

    let mut delay =
        hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static("hex.delay"))
            .unwrap();
    // Linked Delay, 32 ms (six exact SDK quanta), fully wet, no feedback or filter.
    for (parameter, value) in [
        (5, ParameterValue::Bool(false)),  // left sync
        (15, ParameterValue::Bool(false)), // filter
        (26, ParameterValue::Bool(false)), // high quality
        (1, ParameterValue::Float(32.0)),  // left time
        (2, ParameterValue::Float(0.0)),   // feedback
        (3, ParameterValue::Float(1.0)),   // dry/wet
    ] {
        delay = hex_engine::patch_native_device_parameter(
            &delay,
            ParameterId::from_static(parameter),
            value,
        )
        .unwrap();
    }
    let added = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: delay,
    }));
    let Some(Created::Device(delay)) = added.created else {
        panic!("one native Delay");
    };
    applied = h.applied(applied).0;
    h.run(Request::InsertPluginEffect(request(
        &added.state,
        PluginEffectDestination::Track(track),
    )));
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let Some(Created::Device(effect)) = inserted.created else {
        panic!("one signed Track effect");
    };
    h.applied(applied);
    let predecessor = status(&h).recording_source().unwrap();
    assert_eq!(predecessor.context.editor_revision, inserted.state.revision);
    wait(|| h.plugin_progress.lock().unwrap().source_bindings[0].is_some());
    let binding = |h: &ResidentHarness| {
        h.plugin_progress.lock().unwrap().source_bindings[0].map(|(id, binding, _)| (id, binding))
    };
    let original = binding(&h).expect("installed signed effect");
    assert_eq!(original.0, effect.value());
    assert_eq!(original.1.root, predecessor.output_identity);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .main_latency()
            .unwrap()
            .arrival_frames,
        BRIDGE,
    );
    let stored = &inserted.state;
    let owner = DeviceChainOwner::Track(track);
    let selected = DeviceSelectionIds::new(&[delay]).unwrap();
    let mut expected = hex_project::Editor::new(stored.project.as_ref().clone(), 8).unwrap();
    expected
        .execute(Command::ReorderDevices {
            owner,
            selected: selected.clone(),
            before: None,
        })
        .unwrap();
    let [first, second] = expected.project().track(track).unwrap().devices() else {
        panic!("one reordered effect and one Delay");
    };
    assert_eq!([first.id(), second.id()], [effect, delay]);
    h.enable_confirmed_device_inserts();

    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let wet = f32::from(SIGNAL) / 32768.0;
    let old_wet = |start: u64| {
        let block = start as usize / QUANTUM;
        if block < DELAY_QUANTA + 2 {
            0.0
        } else {
            wet * (0.5 + (block - 2) as f32 * 0.125)
        }
    };
    let render = |h: &ResidentHarness| {
        let start = status(h).position.load(Acquire);
        let pcm = instruments::render_replay_block(h);
        assert_eq!(status(h).position.load(Acquire), start + QUANTUM as u64);
        (start, pcm)
    };
    for block in 0..=DELAY_QUANTA + 2 {
        let (start, pcm) = render(&h);
        assert_eq!(start, (block * QUANTUM) as u64);
        assert_block(pcm, start, |_| old_wet(start));
    }

    let prepared = h
        .device_insert_probe()
        .observe_next_foreign_effect_binding(effect);
    let (publication_ready, enqueue, _busy) = h.probe_next_native_publication();
    let (submitted, resume) = h.probe_next_submitted_device_insert();
    h.service
        .submit(Request::ReorderDevices {
            project_generation: predecessor.context.project_generation,
            revision: stored.revision,
            owner,
            selected,
            before: None,
        })
        .unwrap();
    let stage_deadline = Instant::now() + Duration::from_millis(800);
    let mut enqueued = false;
    let exact = loop {
        h.pump();
        if let Ok(exact) = submitted.try_recv() {
            break exact;
        }
        if !enqueued && publication_ready.try_recv().is_ok() {
            enqueue.send(()).unwrap();
            enqueued = true;
        }
        while let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.project, stored.project);
            assert_eq!(event.state.revision, stored.revision);
            assert!(
                !event.request_complete && event.error.is_none(),
                "terminal before submission: status={:?} error={:?} receipt={:?}",
                event.status,
                event.error,
                event.device_insert_complete,
            );
        }
        assert!(
            Instant::now() < stage_deadline,
            "hosted reorder did not stage"
        );
        // Fresh source preparation waits for native mailbox progress on
        // Session. Pace the old worker only while publication is still gated.
        if !enqueued {
            let (start, pcm) = render(&h);
            assert_block(pcm, start, |_| old_wet(start));
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(exact.output_identity, predecessor.output_identity);
    assert_eq!(exact.context.editor_revision, stored.revision + 1);
    assert!(exact.audio_revision > predecessor.audio_revision);
    assert_eq!(status(&h).recording_source(), Some(predecessor));
    assert_eq!(binding(&h), Some(original));
    resume.send(()).unwrap();
    let begun = h.next();
    assert!(
        !begun.request_complete && begun.error.is_none(),
        "terminal before the callback: status={:?} error={:?}",
        begun.status,
        begun.error,
    );
    assert_eq!(begun.state.project, stored.project);
    assert_eq!(begun.state.revision, stored.revision);
    assert_eq!(status(&h).recording_source(), Some(predecessor));

    let deadline = Instant::now() + Duration::from_millis(800);
    let mut offered = None;
    let mut cut = None;
    let mut faded = false;
    let fresh = 'publication: loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            if !event.request_complete {
                assert_eq!(event.state.project, stored.project);
                assert_eq!(event.state.revision, stored.revision);
            }
            if let Some(offer) = event.device_insert_confirmation.as_ref() {
                assert!(offered.is_none() && !faded && cut.is_none());
                assert_eq!(offer.operation(), GraphEditOperation::MoveDevice);
                assert_eq!(status(&h).recording_source(), Some(predecessor));
                let submission = offer.submission();
                assert_eq!(submission.output_identity, predecessor.output_identity);
                assert_eq!(submission.context.editor_revision, stored.revision + 1);
                assert!(submission.audio_revision > predecessor.audio_revision);
                offered = Some(submission);
                assert!(offer.cancel());
            }
            if event.request_complete {
                let receipt = event.device_insert_complete.unwrap_or_else(|| {
                    panic!(
                        "reorder ended without an exact callback receipt: {:?}, {:?}",
                        event.status, event.error,
                    )
                });
                match receipt.outcome {
                    DeviceInsertOutcome::Installed => {
                        assert!(offered.is_none() && faded);
                        assert!(cut.is_some(), "terminal before the exact callback cut");
                        assert_eq!(event.state.project.as_ref(), expected.project());
                        assert_eq!(event.state.revision, stored.revision + 1);
                        assert_eq!(event.error, None);
                        let source = status(&h).recording_source().unwrap();
                        assert_eq!(source.output_identity, predecessor.output_identity);
                        assert_eq!(source.output_identity, receipt.submission.output_identity);
                        assert_eq!(source.audio_revision, receipt.submission.audio_revision);
                        assert_eq!(source.context, receipt.submission.context);
                        let (prepared_owner, fresh) = prepared
                            .try_recv()
                            .unwrap()
                            .expect("fresh reordered worker");
                        assert_eq!(
                            prepared_owner,
                            hex_engine::ProcessorChainOwner::Track(track.value())
                        );
                        assert_eq!(fresh.root, predecessor.output_identity);
                        assert_eq!(fresh.instance, effect.value());
                        assert_ne!(fresh, original.1);
                        break 'publication fresh;
                    }
                    DeviceInsertOutcome::Cancelled => {
                        assert_eq!(offered, Some(receipt.submission));
                        assert_eq!(event.state.project, stored.project);
                        assert_eq!(event.state.revision, stored.revision);
                        assert_eq!(status(&h).recording_source(), Some(predecessor));
                        assert_eq!(binding(&h), Some(original));
                        let (start, pcm) = render(&h);
                        assert_block(pcm, start, |_| old_wet(start));
                        panic!(
                            "same-latency hosted reorder offered an interruption; Cancel kept the predecessor and its PCM"
                        );
                    }
                    outcome => {
                        assert!(!faded && cut.is_none());
                        assert_eq!(event.state.project, stored.project);
                        assert_eq!(event.state.revision, stored.revision);
                        assert_eq!(status(&h).recording_source(), Some(predecessor));
                        assert_eq!(binding(&h), Some(original));
                        let (start, pcm) = render(&h);
                        assert_block(pcm, start, |_| old_wet(start));
                        panic!("hosted reorder ended at {outcome:?}: {:?}", event.error);
                    }
                }
            }
        }
        assert!(Instant::now() < deadline, "no typed reorder outcome");
        let (start, pcm) = render(&h);
        if status(&h).recording_source() != Some(predecessor) {
            let origin = *cut.get_or_insert(start);
            assert!(start - origin < u64::from(BRIDGE) + (DELAY_QUANTA * QUANTUM) as u64);
            assert_block(pcm, start, |_| 0.0);
        } else {
            let old = old_wet(start);
            if !faded && (pcm[0][0] - old).abs() > TOLERANCE {
                faded = true;
                assert_block(pcm, start, |index| {
                    let gain = 1.0 - (index + 1).min(FADE) as f32 / FADE as f32;
                    old * gain
                });
            } else {
                assert_block(pcm, start, |_| if faded { 0.0 } else { old });
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let first_wet = cut.unwrap() + u64::from(BRIDGE) + (DELAY_QUANTA * QUANTUM) as u64;
    let mut second_wet_block = false;
    for _ in 0..=DELAY_QUANTA + 3 {
        let (start, pcm) = render(&h);
        let left = if start < first_wet {
            0.0
        } else if start == first_wet {
            wet * 0.5
        } else {
            assert_eq!(start, first_wet + QUANTUM as u64);
            wet * 0.625
        };
        assert_block(pcm, start, |_| left);
        if start > first_wet {
            second_wet_block = true;
            break;
        }
    }
    assert!(
        second_wet_block,
        "new chain did not complete its second wet quantum"
    );
    let terminal_poll = h.control_polls.load(Acquire);
    let retirement_deadline = Instant::now() + Duration::from_millis(800);
    loop {
        h.pump();
        if h.control_polls.load(Acquire) > terminal_poll
            && h.plugin_records.load(Acquire) == 1
            && binding(&h) == Some((effect.value(), fresh))
        {
            break;
        }
        assert!(
            Instant::now() < retirement_deadline,
            "the old worker did not return or the new binding was not adopted"
        );
        render(&h);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .main_latency()
            .unwrap()
            .arrival_frames,
        BRIDGE,
    );
}

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

fn write_dry_bed(files: &Files, rate: u32, frames: u32) -> PathBuf {
    let path = files.constant_wav("bed.wav", rate, frames, &[0, 0]);
    let mut wave = fs::read(&path).unwrap();
    let data_start = wave.len() - frames as usize * 4;
    for (index, frame) in wave[data_start..].chunks_exact_mut(4).enumerate() {
        frame[2..].copy_from_slice(&bed_sample(index as u64).to_le_bytes());
    }
    fs::write(&path, wave).unwrap();
    path
}

fn bed_sample(source_frame: u64) -> i16 {
    // This period is coprime to the bridge quantum and fade.
    512 + (source_frame % 127) as i16 * 8
}

fn assert_block(pcm: [hex_engine::Frame; QUANTUM], start: u64, left_at: impl Fn(usize) -> f32) {
    for (offset, actual) in pcm.into_iter().enumerate() {
        let frame = start + offset as u64;
        let right = frame
            .checked_sub(u64::from(BRIDGE))
            .map_or(0.0, |source| f32::from(bed_sample(source)) / 32768.0);
        let expected = [left_at(offset), right];
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, b)| (*a - b).abs() < TOLERANCE),
            "frame {frame}: expected {expected:?}, got {actual:?}",
        );
    }
}
