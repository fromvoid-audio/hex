//! Signed Resident removal of the whole wet bus and its exact worker.
mod power_off;
mod two_dry_tracks;

use super::*;
use hex_audio::plugin_effects::EffectBinding;
use hex_audio::prepared::{AppliedAudioSource, FreshRuntimeCancellationResult};
use hex_project::{DeviceId, ReturnId, TrackId};
use hex_session::service::DeviceInsertOutcome;

const BLOCK: usize = 256;
const BRIDGE: usize = 2 * BLOCK;
const FRAMES: u32 = 131_072;

fn dry_sample(frame: usize) -> f32 {
    if frame >= FRAMES as usize {
        0.0
    } else if frame % 127 < 63 {
        0.25
    } else {
        0.125
    }
}

fn timed_wav(files: &Files) -> PathBuf {
    let path = files.wav(48_000, FRAMES);
    let mut bytes = fs::read(&path).unwrap();
    for (frame, sample) in bytes[44..].chunks_exact_mut(2).enumerate() {
        let source = (dry_sample(frame) * 32768.0) as i16;
        sample.copy_from_slice(&source.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    path
}

fn dry_at(frame: usize, latency: usize) -> f32 {
    frame.checked_sub(latency).map_or(0.0, dry_sample)
}

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

fn render(h: &ResidentHarness) -> [hex_engine::Frame; BLOCK] {
    let pcm = h.render_plugin_block();
    state::edits::workers_idle(h);
    pcm
}

fn render_rolling(h: &ResidentHarness) -> (usize, [hex_engine::Frame; BLOCK]) {
    let start = status(h).position.load(Acquire) as usize;
    let pcm = render(h);
    assert!(status(h).playing.load(Acquire));
    assert_eq!(status(h).position.load(Acquire), (start + BLOCK) as u64);
    (start, pcm)
}

fn assert_aligned_wet(h: &ResidentHarness) -> f32 {
    let (start, pcm) = render_rolling(h);
    let scale = pcm[0][0] / dry_at(start, BRIDGE);
    assert!(scale.is_finite() && scale > 1.0);
    for (offset, frame) in pcm.into_iter().enumerate() {
        assert_eq!(frame, [dry_at(start + offset, BRIDGE) * scale; 2]);
    }
    scale
}

fn assert_aligned_dry(h: &ResidentHarness, latency: usize) {
    let (start, pcm) = render_rolling(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        assert_eq!(
            frame,
            [dry_at(start + offset, latency); 2],
            "dry frame {}",
            start + offset
        );
    }
}

fn await_preparation<T>(h: &mut ResidentHarness, receiver: &mpsc::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(value) = receiver.try_recv() {
            return value;
        }
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event.request_complete && event.error.is_none(),
                "whole Return removal stopped before publication: {:?}",
                event.error
            );
        }
        assert!(Instant::now() < deadline, "actual whole Return preparation");
        std::thread::yield_now();
    }
}

struct RollingHostedReturn {
    harness: ResidentHarness,
    inserted: Event,
    track: TrackId,
    bus: ReturnId,
    device: DeviceId,
    predecessor: AppliedAudioSource,
    binding: EffectBinding,
}

fn rolling_hosted_return(files: &Files) -> RollingHostedReturn {
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
        u64::from(FRAMES),
    );
    assert!(h.next().error.is_none());
    let (empty, _) = h.applied(0);
    let imported = h.run(Request::Import(timed_wav(files)));
    let track = imported.state.project.tracks()[0].id();
    let (dry, _) = h.applied(empty);
    let created = h.run(Request::Edit(Command::AddReturnTrack {
        name: "wet".into(),
    }));
    let Some(Created::ReturnTrack(bus)) = created.created else {
        panic!("Return bus");
    };
    let (bus_revision, _) = h.applied(dry);
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
    let incumbent = &inserted.state;
    let (_, binding, _) = {
        wait(|| h.plugin_progress.lock().unwrap().source_bindings[0].is_some());
        h.plugin_progress.lock().unwrap().source_bindings[0].unwrap()
    };
    let effect_type = incumbent.project.device(device).unwrap().state().type_id();
    let observation = h.submitted_observation().unwrap();
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
    let predecessor = status(&h).recording_source().unwrap();
    assert_eq!(
        observation.main_latency().unwrap().arrival_frames,
        BRIDGE as u32
    );
    let canonical = incumbent.canonical_audio_source.as_ref().unwrap();
    assert!(canonical.required_sources().next().is_some());
    assert!(
        canonical
            .required_sources()
            .all(|(_, source)| matches!(source, hex_session::AudioSourceBinding::Resident(_)))
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..3 {
        render(&h);
    }
    let prior = assert_aligned_wet(&h);
    assert_eq!(assert_aligned_wet(&h), prior + 0.0625);
    assert_eq!(status(&h).recording_source(), Some(predecessor));
    RollingHostedReturn {
        harness: h,
        inserted,
        track,
        bus,
        device,
        predecessor,
        binding,
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_whole_vst3_return_removal_keeps_the_dry_track_and_returns_the_worker() {
    let _native = native_fixture();
    let files = Files::new();
    let RollingHostedReturn {
        harness: mut h,
        inserted,
        track,
        bus,
        device,
        predecessor,
        binding,
    } = rolling_hosted_return(&files);
    let incumbent = &inserted.state;

    let assert_predecessor = |h: &ResidentHarness, event: &Event| {
        assert_eq!(event.state.project, incumbent.project);
        assert_eq!(event.state.revision, incumbent.revision);
        assert_eq!(event.state.can_undo, incumbent.can_undo);
        assert_eq!(event.state.can_redo, incumbent.can_redo);
        assert_eq!(status(h).recording_source(), Some(predecessor));
        let (live, accepted, _) = h.plugin_progress.lock().unwrap().source_bindings[0].unwrap();
        assert_eq!((live, accepted), (device.value(), binding));
        assert!(status(h).playing.load(Acquire));
    };
    let mut installed = None;
    for cancel in [true, false] {
        let (ready, resume, _busy) = h.probe_next_native_publication();
        let (submitted, submit_resume) = h.probe_next_submitted_device_insert();
        h.service
            .submit(Request::Edit(Command::RemoveReturnTrack(bus)))
            .unwrap();
        await_preparation(&mut h, &ready);
        assert_predecessor(&h, &inserted);
        assert_aligned_wet(&h);
        resume.send(()).unwrap();
        let first = await_preparation(&mut h, &submitted);
        assert_eq!(first.output_identity, predecessor.output_identity);
        assert_eq!(first.context.editor_revision, incumbent.revision + 1);
        submit_resume.send(()).unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.request_complete);
        assert_predecessor(&h, &pending);
        assert!(h.submitted.as_ref().is_some_and(|(revision, metadata)| {
            *revision == first.audio_revision && metadata.context == first.context
        }));
        assert_eq!(
            h.submitted_observation()
                .unwrap()
                .main_latency()
                .unwrap()
                .arrival_frames,
            0
        );

        let (start, fade) = render_rolling(&h);
        assert!(fade[0][0] > dry_at(start, BRIDGE));
        assert_eq!(fade[BLOCK - 1], [dry_at(start + BLOCK - 1, BRIDGE); 2]);
        assert_predecessor(&h, &pending);
        if cancel {
            wait(|| {
                match h
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .cancel_fresh_runtime_publication(first.audio_revision)
                {
                    FreshRuntimeCancellationResult::Busy => false,
                    result => {
                        assert_eq!(result, FreshRuntimeCancellationResult::Cancelled);
                        true
                    }
                }
            });
        }

        let deadline = Instant::now() + Duration::from_secs(3);
        let terminal = 'receipt: loop {
            h.pump();
            while let Ok(event) = h.service.try_event() {
                assert!(
                    event.device_insert_confirmation.is_none(),
                    "the bounded pass must fit without interruption consent"
                );
                if event.request_complete {
                    break 'receipt event;
                }
                assert!(event.error.is_none(), "{:?}", event.error);
                assert_eq!(event.state.project, incumbent.project);
                assert_eq!(event.state.revision, incumbent.revision);
            }
            if cancel {
                render_rolling(&h);
            } else {
                assert_aligned_dry(&h, BRIDGE);
            }
            assert!(Instant::now() < deadline, "actual whole Return terminal");
            std::thread::sleep(Duration::from_millis(1));
        };
        let completion = terminal.device_insert_complete.unwrap();
        assert_eq!(completion.submission, first);
        if cancel {
            assert_eq!(completion.outcome, DeviceInsertOutcome::Cancelled);
            assert_predecessor(&h, &terminal);
            render_rolling(&h);
            assert_aligned_wet(&h);
        } else {
            assert_eq!(
                completion.outcome,
                DeviceInsertOutcome::Installed,
                "{:?}",
                terminal.error
            );
            assert!(terminal.error.is_none(), "{:?}", terminal.error);
            installed = Some(terminal);
        }
    }
    let removed = installed.unwrap();
    let completion = removed.device_insert_complete.unwrap();
    let applied = status(&h).recording_source().unwrap();
    assert_eq!(completion.submission.context, applied.context);
    assert_eq!(completion.submission.audio_revision, applied.audio_revision);
    assert_eq!(applied.output_identity, predecessor.output_identity);
    assert_ne!(applied, predecessor);
    assert_eq!(removed.state.revision, incumbent.revision + 1);
    assert!(removed.state.project.return_tracks().is_empty());
    assert!(removed.state.project.device(device).is_none());
    let before = incumbent.project.track(track).unwrap();
    let after = removed.state.project.track(track).unwrap();
    assert_eq!(after.clips(), before.clips());
    assert_eq!(after.source_id(), before.source_id());
    wait(|| {
        assert_aligned_dry(&h, BRIDGE);
        h.pump();
        h.plugin_records.load(Acquire) == 0
    });
    assert!(h.plugin_progress.lock().unwrap().source_bindings[0].is_none());
    assert_aligned_dry(&h, BRIDGE);
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
    assert_aligned_dry(&h, 0);
}
