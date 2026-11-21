//! Actual SDK witness for moving an authored gate between two bare MIDI lanes.
use super::*;
use hex_audio::prepared::FreshRuntimeCancellationResult;
use hex_project::{ClipId, DeviceSelectionIds, TrackId};
use hex_session::service::DeviceInsertOutcome;

const BRIDGE: u64 = 512;
const BLOCK: u64 = 256;
const DONOR_GAIN: f32 = 0.25;
const RECEIVER_GAIN: f32 = 0.75;
const REFRESHED_DONOR_GAIN: f32 = 0.5;

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn direct_vst3_cut_preserves_original_gate_release_and_new_receiver_onset() {
    verify_direct_vst3_cut(AfterCut::Refresh);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn second_live_vst3_move_returns_the_original_worker_and_held_gate() {
    verify_direct_vst3_cut(AfterCut::MoveBack);
}

enum AfterCut {
    Refresh,
    MoveBack,
}

fn verify_direct_vst3_cut(after_cut: AfterCut) {
    const CHILD: &str = "HEX_TEST_HELD_VST3_TRACK_CUT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let _native = native_fixture();
    let catalog = Files::new();
    let directory = catalog.0.canonicalize().unwrap();
    let mut selected = selection();
    let module = directory.join("original.vst3");
    super::super::ui::copy_original_module(&selected.module, &module);
    selected.module = module;
    hex_ui::write_plugin_effect_test_catalog(&directory, &selected, "Instrument|Synth", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: BRIDGE as u32,
        }),
        directory.join("plugin-catalog.json"),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let (donor, donor_clip) = add_lane(&mut h, &mut applied, "held source");
    let (receiver, receiver_clip) = add_lane(&mut h, &mut applied, "destination");
    let donor_gate = BeatTime::from_quarters(1).unwrap();
    let new_onset = BeatTime::from_ratio(3, 2).unwrap();
    let new_gate = BeatTime::from_ratio(1, 8).unwrap();
    for (clip, start, duration, velocity) in [
        (donor_clip, BeatTime::ZERO, donor_gate, 63),
        (
            receiver_clip,
            BeatTime::ZERO,
            BeatTime::from_ratio(5, 4).unwrap(),
            119,
        ),
        (receiver_clip, new_onset, new_gate, 85),
    ] {
        // The controlled SDK variant requires pitch 60 and channel 0 and
        // distinguishes simultaneous named note IDs by summed velocity.
        h.run(Request::Edit(Command::AddMidiNote {
            clip,
            start,
            duration,
            velocity,
            pitch: 60,
            channel: 0,
        }));
        (applied, _) = h.applied(applied);
    }
    let mixed = h.run(Request::Edit(Command::SetTrackGains {
        gains: vec![(donor, DONOR_GAIN), (receiver, RECEIVER_GAIN)],
    }));
    (applied, _) = h.applied(applied);
    let mut insert = request(&mixed.state, PluginEffectDestination::Instrument(donor));
    insert.selection = selected;
    h.run(Request::InsertPluginEffect(insert));
    let before = terminal(&mut h);
    assert!(before.plugin_effect_complete[0].accepted);
    let instrument = before
        .state
        .project
        .track(donor)
        .unwrap()
        .instrument()
        .unwrap();
    let device = instrument.id();
    h.applied(applied);
    assert_eq!(before.state.project.tracks().len(), 2);
    assert!(
        before
            .state
            .project
            .tracks()
            .iter()
            .all(|track| track.clips().is_empty() && track.devices().is_empty())
    );
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .processor_latency(device.value(), instrument.state().type_id())
            .unwrap()
            .stage_frames,
        BRIDGE as u32
    );

    let binding = |h: &ResidentHarness| {
        let observation = h.plugin_progress.lock().unwrap();
        let [Some((id, binding, _)), None] = observation.source_bindings else {
            panic!("one accepted SDK worker");
        };
        assert_eq!(id, device.value());
        binding
    };
    let worker = binding(&h);
    h.enable_confirmed_device_inserts();
    let control = &mut h.audio.as_mut().unwrap().control;
    control.set_audio_diagnostics(hex_audio::audio_diagnostics::AudioDiagnosticsFlags::ALL);
    control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..5 {
        render_completed_block(&h);
    }
    assert_sample_level(&render_completed_block(&h), DONOR_GAIN * 63.0 / 127.0);
    let predecessor = status(&h).recording_source().unwrap();

    let stage = |h: &mut ResidentHarness, attempt: &str| {
        let prepared_worker = h
            .device_insert_probe()
            .observe_next_foreign_binding(receiver, device);
        let (submitted, resume) = h.probe_next_submitted_device_insert();
        h.service
            .submit(Request::MoveTrackDevices {
                project_generation: 0,
                revision: before.state.revision,
                source: donor,
                selected: DeviceSelectionIds::new(&[device]).unwrap(),
                destination: receiver,
            })
            .unwrap();
        let exact = submitted
            .recv_timeout(Duration::from_millis(800))
            .unwrap_or_else(|timeout| {
                let failure = h.service.try_event().ok().and_then(|event| event.error);
                panic!(
                    "held VST3 move {attempt} was not submitted: {timeout}; session error={failure:?}; prepared worker={:?}",
                    prepared_worker.try_recv()
                );
            });
        assert_eq!(
            prepared_worker.try_recv().unwrap(),
            Some((
                hex_engine::ProcessorChainOwner::Track(donor.value()),
                worker
            ))
        );
        resume.send(()).unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.request_complete);
        assert_eq!(pending.state.project, before.state.project);
        assert_eq!(status(h).recording_source(), Some(predecessor));
        exact
    };
    let cancelled = stage(&mut h, "before cancellation");
    wait(|| {
        match h
            .audio
            .as_ref()
            .unwrap()
            .control
            .cancel_fresh_runtime_publication(cancelled.audio_revision)
        {
            FreshRuntimeCancellationResult::Busy => false,
            state => {
                assert_eq!(state, FreshRuntimeCancellationResult::Cancelled);
                true
            }
        }
    });
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        assert_sample_level(&render_continuous_block(&h).1, DONOR_GAIN * 63.0 / 127.0);
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.project, before.state.project);
            if event.request_complete {
                let completion = event.device_insert_complete.unwrap();
                assert_eq!(completion.submission, cancelled);
                assert_eq!(completion.outcome, DeviceInsertOutcome::Cancelled);
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "cancelled held move receipt timed out"
        );
    }
    assert_eq!(status(&h).recording_source(), Some(predecessor));
    assert_eq!(binding(&h), worker);

    let exact = stage(&mut h, "after cancellation");

    let mut cut = None;
    let mut pcm_after_cut = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(800);
    let installed = loop {
        let (frame, pcm) = render_continuous_block(&h);
        if status(&h)
            .recording_source()
            .is_some_and(|source| source.audio_revision == exact.audio_revision)
        {
            cut.get_or_insert(frame);
            pcm_after_cut.extend(pcm);
        }
        h.pump();
        if let Ok(event) = h.service.try_event() {
            if let Some(error) = event.error.as_ref() {
                assert_eq!(status(&h).recording_source(), Some(predecessor));
                assert_eq!(binding(&h), worker);
                while let Some(row) = h.audio.as_mut().unwrap().control.take_audio_diagnostic() {
                    eprintln!("held VST3 cut diagnostic {row:?}");
                }
                panic!("prepared original VST3 worker was refused by callback: {error}");
            }
            assert!(event.device_insert_confirmation.is_none());
            if event.request_complete {
                break event;
            }
            assert_eq!(event.state.project, before.state.project);
        }
        assert!(
            Instant::now() < deadline,
            "exact held move receipt timed out"
        );
    };
    let completion = installed.device_insert_complete.unwrap();
    assert_eq!(completion.submission, exact);
    assert_eq!(completion.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(
        status(&h).recording_source(),
        Some(hex_audio::prepared::AppliedAudioSource {
            audio_revision: exact.audio_revision,
            context: exact.context,
            output_identity: exact.output_identity,
        })
    );
    assert_eq!(binding(&h), worker);
    assert!(
        installed
            .state
            .project
            .track(donor)
            .unwrap()
            .instrument()
            .is_none()
    );
    assert_eq!(
        installed
            .state
            .project
            .track(receiver)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );

    let map = installed.state.project.tempo_map();
    let donor_off = map.beat_to_frame(donor_gate).unwrap() + BRIDGE;
    let receiver_on = map.beat_to_frame(new_onset).unwrap() + BRIDGE;
    let receiver_off = map
        .beat_to_frame(BeatTime::from_raw(new_onset.raw() + new_gate.raw()))
        .unwrap()
        + BRIDGE;
    let cut = cut.expect("installed callback frame");
    assert!(cut + BLOCK < donor_off);
    let inverse = match after_cut {
        AfterCut::Refresh => None,
        AfterCut::MoveBack => {
            let selected = DeviceSelectionIds::new(&[device]).unwrap();
            let request = |revision| Request::MoveTrackDevices {
                project_generation: exact.context.project_generation,
                revision,
                source: receiver,
                selected: selected.clone(),
                destination: donor,
            };
            let refreshed = cancel_or_refuse_inverse_and_refresh(
                &mut h,
                request(installed.state.revision),
                &installed.state,
                donor,
                worker,
                donor_off,
            );
            let mut expected =
                hex_project::Editor::new(refreshed.project.as_ref().clone(), 8).unwrap();
            expected
                .execute(Command::MoveTrackDevices {
                    source: receiver,
                    selected: selected.clone(),
                    destination: donor,
                })
                .unwrap();
            Some((
                request(refreshed.revision),
                refreshed,
                expected.project().clone(),
                REFRESHED_DONOR_GAIN,
            ))
        }
    };
    if let Some((request, moved, expected, gain)) = inverse {
        assert_inverse_continues_held_gate(
            &mut h, request, &expected, &moved, worker, donor_off, gain,
        );
        return;
    }

    let refreshed = h.run(Request::Edit(Command::SetTrackGains {
        gains: vec![(donor, 0.5)],
    }));
    assert!(refreshed.request_complete);
    let refresh_deadline = Instant::now() + Duration::from_millis(800);
    loop {
        pcm_after_cut.extend(render_continuous_block(&h).1);
        if let Some(source) = status(&h).recording_source()
            && source.audio_revision > exact.audio_revision
        {
            assert_eq!(source.context.editor_revision, refreshed.state.revision);
            assert_eq!(source.output_identity, predecessor.output_identity);
            break;
        }
        assert!(cursor(&h) < donor_off && Instant::now() < refresh_deadline);
    }
    assert!(
        cursor(&h) < donor_off,
        "refresh must keep the original gate held"
    );
    assert_eq!(binding(&h), worker);
    while cursor(&h) < receiver_off + BLOCK {
        assert!(Instant::now() < deadline + Duration::from_secs(2));
        pcm_after_cut.extend(render_continuous_block(&h).1);
    }
    // The first callback's final sample has cleared the short paired fader.
    // It must already carry the original voice from the retained SDK bridge.
    for (offset, frame) in pcm_after_cut
        .into_iter()
        .enumerate()
        .skip(BLOCK as usize - 1)
    {
        let at = cut + offset as u64;
        let velocity = if at < donor_off {
            63.0
        } else if at >= receiver_on && at < receiver_off {
            85.0
        } else {
            0.0
        };
        let expected = RECEIVER_GAIN * velocity / 127.0;
        assert!(
            frame
                .into_iter()
                .all(|sample| (sample - expected).abs() < 1e-5),
            "frame {at} after cut {cut}: {frame:?}, expected {expected}"
        );
    }
    assert_eq!(status(&h).render_fault_snapshot(), None);
}

fn cancel_or_refuse_inverse_and_refresh(
    h: &mut ResidentHarness,
    request: Request,
    moved: &hex_session::service::ViewState,
    donor: TrackId,
    worker: hex_audio::plugin_effects::EffectBinding,
    donor_off: u64,
) -> Arc<hex_session::service::ViewState> {
    let predecessor = status(h).recording_source().unwrap();
    let (submitted, resume) = h.probe_next_submitted_device_insert();
    h.service.submit(request).unwrap();
    let deadline = Instant::now() + Duration::from_millis(800);
    let terminal = loop {
        if let Ok(exact) = submitted.try_recv() {
            resume.send(()).unwrap();
            let pending = h.next();
            assert!(!pending.request_complete && pending.error.is_none());
            assert_eq!(pending.state.project, moved.project);
            wait(|| {
                match h
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .cancel_fresh_runtime_publication(exact.audio_revision)
                {
                    FreshRuntimeCancellationResult::Busy => false,
                    state => {
                        assert_eq!(state, FreshRuntimeCancellationResult::Cancelled);
                        true
                    }
                }
            });
            let cancelled = loop {
                assert_sample_level(&render_continuous_block(h).1, RECEIVER_GAIN * 63.0 / 127.0);
                h.pump();
                if let Ok(event) = h.service.try_event()
                    && event.request_complete
                {
                    break event;
                }
                assert!(Instant::now() < deadline && cursor(h) < donor_off);
            };
            let receipt = cancelled.device_insert_complete.unwrap();
            assert_eq!(receipt.submission, exact);
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Cancelled);
            break cancelled;
        }
        if let Ok(refused) = h.service.try_event() {
            assert!(refused.request_complete);
            assert!(refused.error.is_some());
            eprintln!(
                "first inverse refused before submission: {:?}",
                refused.error
            );
            break refused;
        }
        assert!(
            Instant::now() < deadline,
            "first inverse never reached admission"
        );
        std::thread::yield_now();
    };
    assert_eq!(terminal.state.project, moved.project);
    assert_eq!(terminal.state.revision, moved.revision);
    assert_eq!(status(h).recording_source(), Some(predecessor));
    assert_sample_level(&render_continuous_block(h).1, RECEIVER_GAIN * 63.0 / 127.0);

    let refreshed = h.run(Request::Edit(Command::SetTrackGains {
        gains: vec![(donor, REFRESHED_DONOR_GAIN)],
    }));
    assert!(refreshed.request_complete);
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        assert_sample_level(&render_continuous_block(h).1, RECEIVER_GAIN * 63.0 / 127.0);
        if let Some(source) = status(h).recording_source()
            && source.audio_revision > predecessor.audio_revision
        {
            assert_eq!(source.context.editor_revision, refreshed.state.revision);
            assert_eq!(source.output_identity, predecessor.output_identity);
            break;
        }
        assert!(Instant::now() < deadline && cursor(h) < donor_off);
    }
    let bindings = h.plugin_progress.lock().unwrap().source_bindings;
    assert!(
        matches!(bindings, [Some((id, binding, _)), None] if id == worker.instance && binding == worker)
    );
    refreshed.state
}

fn assert_inverse_continues_held_gate(
    h: &mut ResidentHarness,
    request: Request,
    expected: &hex_project::Project,
    moved: &hex_session::service::ViewState,
    worker: hex_audio::plugin_effects::EffectBinding,
    donor_off: u64,
    donor_gain: f32,
) {
    let predecessor = status(h).recording_source().unwrap();
    assert_sample_level(&render_continuous_block(h).1, RECEIVER_GAIN * 63.0 / 127.0);
    let (preparing, resume, _) = h.probe_next_native_publication();
    h.service.submit(request).unwrap();
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        match preparing.try_recv() {
            Ok(()) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(error) => panic!("inverse publication observation closed: {error}"),
        }
        if let Ok(event) = h.service.try_event() {
            panic!(
                "inverse request returned before preparing its cut: error={:?}, status={:?}, complete={}",
                event.error, event.status, event.request_complete
            );
        }
        assert!(Instant::now() < deadline && cursor(h) + BLOCK < donor_off);
        assert_sample_level(&render_continuous_block(h).1, RECEIVER_GAIN * 63.0 / 127.0);
        h.pump();
    }
    assert_eq!(status(h).recording_source(), Some(predecessor));
    resume.send(()).unwrap();
    let pending = loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            break event;
        }
        assert!(
            Instant::now() < deadline,
            "inverse request published no Pending event"
        );
        std::thread::yield_now();
    };
    assert_eq!(status(h).recording_source(), Some(predecessor));
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert!(
        !pending.request_complete,
        "inverse request completed before Audio rendered its receipt: {:?}",
        pending.status
    );
    assert_eq!(pending.state.project, moved.project);
    assert_eq!(pending.state.revision, moved.revision);

    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        let (_, output) = render_continuous_block(h);
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            if event.request_complete {
                let completion = event.device_insert_complete.unwrap();
                assert_eq!(completion.outcome, DeviceInsertOutcome::Installed);
                assert_eq!(
                    status(h).recording_source(),
                    Some(hex_audio::prepared::AppliedAudioSource {
                        output_identity: completion.submission.output_identity,
                        audio_revision: completion.submission.audio_revision,
                        context: completion.submission.context,
                    })
                );
                assert_eq!(event.state.project.as_ref(), expected);
                assert_eq!(event.state.revision, moved.revision + 1);
                assert!(
                    cursor(h) < donor_off,
                    "inverse request must complete while the note is held"
                );
                let bindings = h.plugin_progress.lock().unwrap().source_bindings;
                assert!(
                    matches!(bindings, [Some((id, binding, _)), None] if id == worker.instance && binding == worker)
                );
                assert_sample_level(&output[output.len() - 1..], donor_gain * 63.0 / 127.0);
                break;
            }
            assert_eq!(event.state.project, moved.project);
        }
        assert!(Instant::now() < deadline && cursor(h) + BLOCK < donor_off);
    }
    while cursor(h) < donor_off + BLOCK {
        let (frame, output) = render_continuous_block(h);
        for (offset, sample) in output.into_iter().enumerate() {
            let gain = if frame + (offset as u64) < donor_off {
                donor_gain * 63.0 / 127.0
            } else {
                0.0
            };
            assert_sample_level(&[sample], gain);
        }
        assert!(Instant::now() < deadline, "original release did not settle");
    }
}

fn add_lane(h: &mut ResidentHarness, applied: &mut u64, name: &str) -> (TrackId, ClipId) {
    let Created::Track(track) = h
        .run(Request::Edit(Command::AddMidiTrack { name: name.into() }))
        .created
        .unwrap()
    else {
        panic!("MIDI lane");
    };
    (*applied, _) = h.applied(*applied);
    let Created::Clip(clip) = h
        .run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(2).unwrap(),
        }))
        .created
        .unwrap()
    else {
        panic!("Arrangement clip");
    };
    (*applied, _) = h.applied(*applied);
    (track, clip)
}

fn assert_sample_level(frames: &[hex_engine::Frame], expected: f32) {
    assert!(
        frames
            .iter()
            .flatten()
            .all(|sample| (*sample - expected).abs() < 1e-5)
    );
}

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

fn cursor(h: &ResidentHarness) -> u64 {
    status(h).performance_frame.load(Acquire)
}

fn render_continuous_block(h: &ResidentHarness) -> (u64, [hex_engine::Frame; BLOCK as usize]) {
    let frame = cursor(h);
    let arrangement = status(h).position.load(Acquire);
    let pcm = render_replay_block(h);
    assert_eq!(cursor(h), frame + BLOCK);
    assert_eq!(status(h).position.load(Acquire), arrangement + BLOCK);
    (frame, pcm)
}
