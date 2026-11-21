//! Both resident dry tracks still owe the hosted Return's old bridge delay.
use super::*;
use hex_session::service::{GraphEditOperation, ResidentGraphSubmission, ViewState};

const FADE: usize = 240;

fn sibling_sample(frame: usize) -> f32 {
    if frame >= FRAMES as usize {
        0.0
    } else if frame % 173 < 91 {
        -0.0625
    } else {
        0.1875
    }
}

fn sibling_wav(files: &Files) -> PathBuf {
    let path = files.wav(48_000, FRAMES);
    let mut bytes = fs::read(&path).unwrap();
    for (frame, sample) in bytes[44..].chunks_exact_mut(2).enumerate() {
        sample.copy_from_slice(&((sibling_sample(frame) * 32768.0) as i16).to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    path
}

fn both_dry_at(frame: usize, latency: usize) -> f32 {
    frame
        .checked_sub(latency)
        .map_or(0.0, |source| dry_sample(source) + sibling_sample(source))
}

fn assert_both_dry(h: &ResidentHarness, latency: usize) {
    let (start, pcm) = render_rolling(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        assert_eq!(frame, [both_dry_at(start + offset, latency); 2]);
    }
}

fn worker_identity(h: &ResidentHarness) -> Option<(u64, hex_audio::plugin_effects::EffectBinding)> {
    h.plugin_progress.lock().unwrap().source_bindings[0]
        .map(|(device, binding, _completed)| (device, binding))
}

fn assert_bed_with_wet(start: usize, pcm: &[hex_engine::Frame; BLOCK]) -> f32 {
    let scale = (pcm[0][0] - both_dry_at(start, BRIDGE)) / dry_at(start, BRIDGE);
    assert!(scale.is_finite() && scale > 0.0);
    for (offset, &frame) in pcm.iter().enumerate() {
        let at = start + offset;
        assert_eq!(
            frame,
            [both_dry_at(at, BRIDGE) + dry_at(at, BRIDGE) * scale; 2]
        );
    }
    scale
}

fn await_terminal(
    h: &mut ResidentHarness,
    original: &ViewState,
    exact: ResidentGraphSubmission,
    cancelling: bool,
) -> Event {
    let deadline = Instant::now() + Duration::from_millis(600);
    let predecessor = status(h).recording_source();
    let mut required_restart = false;
    loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            if let Some(offer) = event.device_insert_confirmation.as_ref() {
                assert_eq!(offer.operation(), GraphEditOperation::RemoveTrack);
                assert_eq!(offer.submission(), exact);
                assert_eq!(event.state.revision, original.revision);
                assert_eq!(event.state.project, original.project);
                assert!(offer.cancel());
                required_restart = true;
            }
            if event.request_complete {
                let completion = event.device_insert_complete.unwrap();
                assert_eq!(completion.submission, exact);
                if required_restart {
                    assert_eq!(completion.outcome, DeviceInsertOutcome::Cancelled);
                    assert_eq!(event.state.revision, original.revision);
                    assert_eq!(event.state.project, original.project);
                    assert_eq!(status(h).recording_source(), predecessor);
                    let (start, pcm) = render_rolling(h);
                    assert_bed_with_wet(start, &pcm);
                    panic!(
                        "removing one hosted Return with two aligned Resident tracks required a timing restart"
                    );
                }
                return event;
            }
            assert!(event.error.is_none(), "{:?}", event.error);
            assert_eq!(event.state.project, original.project);
        }
        if cancelling || required_restart {
            render_rolling(h);
        } else {
            assert_both_dry(h, BRIDGE);
        }
        assert!(Instant::now() < deadline, "exact two-track Return terminal");
        std::thread::yield_now();
    }
}

struct RollingTwoDryTracks {
    harness: ResidentHarness,
    original: Arc<ViewState>,
    bus: ReturnId,
    device: DeviceId,
    predecessor: AppliedAudioSource,
    original_worker: Option<(u64, EffectBinding)>,
}

fn rolling_two_dry_tracks(files: &Files, sibling_files: &Files) -> RollingTwoDryTracks {
    let directory = files.0.canonicalize().unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection(), "Fx", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: BRIDGE as u32,
        }),
        48_000,
        directory.join("plugin-catalog.json"),
        8 * u64::from(FRAMES),
    );
    assert!(h.next().error.is_none());
    let (empty, _) = h.applied(0);
    let first = h.run(Request::Import(timed_wav(files)));
    let track = first.state.project.tracks()[0].id();
    let (first_revision, _) = h.applied(empty);
    let bed = h.run(Request::Import(sibling_wav(sibling_files)));
    assert_eq!(bed.state.project.tracks().len(), 2);
    let (bed_revision, _) = h.applied(first_revision);
    let created = h.run(Request::Edit(Command::AddReturnTrack {
        name: "wet".into(),
    }));
    let Some(Created::ReturnTrack(bus)) = created.created else {
        panic!("Return bus");
    };
    let (bus_revision, _) = h.applied(bed_revision);
    let sent = h.run(Request::Edit(Command::SetReturnSend {
        track,
        return_track: bus,
        gain: 0.5,
    }));
    let (send_revision, _) = h.applied(bus_revision);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &sent.state,
            PluginEffectDestination::Return(bus),
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    assert!(inserted.plugin_effect_complete[0].accepted);
    h.applied(send_revision);
    let Some(Created::Device(device)) = inserted.created else {
        panic!("fixture VST3 on Return");
    };
    let original = &inserted.state;
    let residency = original.canonical_audio_source.as_ref().unwrap();
    assert_eq!(residency.required_sources().count(), 2);
    assert!(
        residency
            .required_sources()
            .all(|(_, source)| matches!(source, hex_session::AudioSourceBinding::Resident(_)))
    );
    let observation = h.submitted_observation().unwrap();
    let effect_type = original.project.device(device).unwrap().state().type_id();
    assert_eq!(
        observation.foreign_processor(device.value(), effect_type),
        Some(true)
    );
    assert_eq!(
        observation
            .processor_latency(device.value(), effect_type)
            .unwrap()
            .stage_frames,
        BRIDGE as u32
    );
    assert_eq!(
        observation.main_latency().unwrap().arrival_frames,
        BRIDGE as u32
    );
    wait(|| worker_identity(&h).is_some());
    let original_worker = worker_identity(&h);
    let predecessor = status(&h).recording_source().unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..3 {
        render(&h);
    }
    let (start, pcm) = render_rolling(&h);
    assert_bed_with_wet(start, &pcm);
    RollingTwoDryTracks {
        harness: h,
        original: inserted.state,
        bus,
        device,
        predecessor,
        original_worker,
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_whole_vst3_return_removal_keeps_two_dry_tracks_aligned() {
    let _native = native_fixture();
    let files = Files::new();
    let sibling_files = Files::new();
    let RollingTwoDryTracks {
        harness: mut h,
        original,
        bus,
        predecessor,
        original_worker,
        ..
    } = rolling_two_dry_tracks(&files, &sibling_files);
    let original = &original;
    for cancel in [true, false] {
        let (ready, resume, _busy) = h.probe_next_native_publication();
        let (submitted, submit_resume) = h.probe_next_submitted_device_insert();
        h.service
            .submit(Request::Edit(Command::RemoveReturnTrack(bus)))
            .unwrap();
        await_preparation(&mut h, &ready);
        let (start, pcm) = render_rolling(&h);
        let wet_scale = assert_bed_with_wet(start, &pcm) + 0.0625;
        resume.send(()).unwrap();
        let exact = await_preparation(&mut h, &submitted);
        assert_eq!(exact.output_identity, predecessor.output_identity);
        assert_eq!(exact.context.editor_revision, original.revision + 1);
        submit_resume.send(()).unwrap();
        let pending = h.next();
        assert!(!pending.request_complete && pending.error.is_none());
        assert_eq!(pending.state.project, original.project);
        assert!(h.submitted.as_ref().is_some_and(|(revision, metadata)| {
            *revision == exact.audio_revision && metadata.context == exact.context
        }));
        assert_eq!(
            h.submitted_observation()
                .unwrap()
                .main_latency()
                .unwrap()
                .arrival_frames,
            0
        );
        let (start, pcm) = render_rolling(&h);
        assert_eq!(status(&h).recording_source(), Some(predecessor));
        assert_eq!(worker_identity(&h), original_worker);
        if pcm.iter().enumerate().all(|(offset, &sample)| {
            let at = start + offset;
            sample == [both_dry_at(at, BRIDGE) + dry_at(at, BRIDGE) * wet_scale; 2]
        }) {
            await_terminal(&mut h, original, exact, true);
            panic!("unchanged wet block without the expected typed restart offer");
        }
        for (offset, frame) in pcm.into_iter().enumerate() {
            let at = start + offset;
            let fade = FADE.saturating_sub(offset + 1) as f32 / FADE as f32;
            let expected = both_dry_at(at, BRIDGE) + dry_at(at, BRIDGE) * (wet_scale * fade);
            assert!(
                frame
                    .into_iter()
                    .all(|sample| (sample - expected).abs() < 2e-6)
            );
        }
        if cancel {
            wait(|| {
                match h
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .cancel_fresh_runtime_publication(exact.audio_revision)
                {
                    FreshRuntimeCancellationResult::Busy => false,
                    result => {
                        assert_eq!(result, FreshRuntimeCancellationResult::Cancelled);
                        true
                    }
                }
            });
        }
        let event = await_terminal(&mut h, original, exact, cancel);
        if cancel {
            assert_eq!(
                event.device_insert_complete.unwrap().outcome,
                DeviceInsertOutcome::Cancelled
            );
            assert_eq!(event.state.project, original.project);
            assert_eq!(status(&h).recording_source(), Some(predecessor));
            render_rolling(&h);
            let (start, pcm) = render_rolling(&h);
            assert_bed_with_wet(start, &pcm);
            assert_eq!(worker_identity(&h), original_worker);
        } else {
            assert_eq!(
                event.device_insert_complete.unwrap().outcome,
                DeviceInsertOutcome::Installed
            );
            assert!(event.state.project.return_tracks().is_empty());
            assert!(event.state.project.device(device).is_none());
            assert_eq!(event.state.revision, original.revision + 1);
            let installed = status(&h).recording_source().unwrap();
            assert_eq!(installed.audio_revision, exact.audio_revision);
            assert_eq!(installed.context, exact.context);
            assert_eq!(installed.output_identity, predecessor.output_identity);
            assert_eq!(event.state.project.tracks().len(), 2);
            for before in original.project.tracks() {
                let after = event.state.project.track(before.id()).unwrap();
                assert_eq!(after.clips(), before.clips());
                assert_eq!(after.source_id(), before.source_id());
            }
        }
    }
    wait(|| {
        assert_both_dry(&h, BRIDGE);
        h.pump();
        h.plugin_records.load(Acquire) == 0
    });
    assert!(worker_identity(&h).is_none());
    h.audio.as_ref().unwrap().control.stop();
    render(&h);
    assert!(!status(&h).playing.load(Acquire));
    assert_eq!(status(&h).position.load(Acquire), 0);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_both_dry(&h, 0);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_return_power_off_preserves_the_wet_source_on_unsupported_bed() {
    let _native = native_fixture();
    let files = Files::new();
    let sibling_files = Files::new();
    let RollingTwoDryTracks {
        harness: mut h,
        original,
        device,
        predecessor,
        original_worker,
        ..
    } = rolling_two_dry_tracks(&files, &sibling_files);
    h.enable_confirmed_device_inserts();
    let assert_original = |h: &ResidentHarness, event: &Event| {
        assert!(event.device_insert_confirmation.is_none());
        assert_eq!(event.state.project, original.project);
        assert_eq!(event.state.revision, original.revision);
        assert_eq!(event.state.can_undo, original.can_undo);
        assert_eq!(event.state.can_redo, original.can_redo);
        assert_eq!(status(h).recording_source(), Some(predecessor));
        assert_eq!(worker_identity(h), original_worker);
        assert!(status(h).playing.load(Acquire));
    };

    let (submitted, resume) = h.probe_next_submitted_device_insert();
    h.service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device,
            bypassed: true,
        }))
        .unwrap();
    let exact = await_preparation(&mut h, &submitted);
    assert_eq!(exact.output_identity, predecessor.output_identity);
    assert_eq!(exact.context.editor_revision, original.revision + 1);
    resume.send(()).unwrap();
    let pending = h.next();
    assert!(!pending.request_complete && pending.error.is_none());
    assert_original(&h, &pending);

    let deadline = Instant::now() + Duration::from_millis(350);
    let terminal = 'terminal: loop {
        let (start, pcm) = render_rolling(&h);
        assert_bed_with_wet(start, &pcm);
        h.pump();
        while let Ok(event) = h.service.try_event() {
            assert_original(&h, &event);
            if event.request_complete {
                break 'terminal event;
            }
        }
        assert!(
            Instant::now() < deadline,
            "missing typed Return power refusal"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let receipt = terminal.device_insert_complete.unwrap();
    assert_eq!(receipt.submission, exact);
    assert_eq!(
        receipt.outcome,
        DeviceInsertOutcome::Refused(
            hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation
        )
    );
    assert_eq!(
        terminal.error.as_deref(),
        Some("running return power off cannot keep this route aligned")
    );
    assert_eq!(h.plugin_records.load(Acquire), 1);
    let (start, pcm) = render_rolling(&h);
    assert_bed_with_wet(start, &pcm);
}
