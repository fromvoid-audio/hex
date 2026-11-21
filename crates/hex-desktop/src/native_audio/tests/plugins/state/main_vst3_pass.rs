//! A signed Main effect and its passive pass compensation have different lifetimes.
use super::*;
use hex_audio::prepared::FreshRuntimeCancellationResult;
use hex_session::service::DeviceInsertOutcome;

const BLOCK: usize = 256;
const BRIDGE: usize = 2 * BLOCK;
const IMPULSE: usize = 192;
const SPACING: usize = 4 * BLOCK;
const SOURCE_FRAMES: u32 = 131_072;
const SIGNAL: f32 = 0.25;
const FADE: usize = 240;

fn source_sample(frame: usize) -> f32 {
    if frame >= SOURCE_FRAMES as usize {
        0.0
    } else if frame % SPACING == IMPULSE {
        SIGNAL
    } else if frame >= 128 {
        0.03125
    } else {
        0.0
    }
}

fn impulse_wav(files: &Files) -> PathBuf {
    // Files::wav writes 16-bit mono PCM immediately after its 44-byte RIFF header.
    let wave = files.wav(48_000, SOURCE_FRAMES);
    let mut bytes = fs::read(&wave).unwrap();
    for (index, frame) in bytes[44..].chunks_exact_mut(2).enumerate() {
        let sample = (source_sample(index) * 32768.0) as i16;
        frame.copy_from_slice(&sample.to_le_bytes());
    }
    fs::write(&wave, bytes).unwrap();
    wave
}

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

fn render_playing(h: &ResidentHarness) -> (usize, [hex_engine::Frame; BLOCK]) {
    let start = usize::try_from(status(h).position.load(Acquire)).unwrap();
    let pcm = h.render_plugin_block();
    edits::workers_idle(h);
    assert_eq!(
        status(h).position.load(Acquire),
        (start + BLOCK) as u64,
        "the live pass must advance by the entire quantum"
    );
    assert!(status(h).playing.load(Acquire));
    (start, pcm)
}

fn assert_dry_timing(h: &ResidentHarness, latency: usize) {
    let (start, pcm) = render_playing(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        let at = start + offset;
        let expected = at.checked_sub(latency).map_or(0.0, source_sample);
        assert_eq!(frame, [expected; 2], "latency={latency} frame={at}");
    }
}

fn signed_output(at: usize) -> (f32, f32) {
    at.checked_sub(BRIDGE).map_or((0.0, 0.0), |origin| {
        let dry = source_sample(origin);
        (dry, dry * (0.5 + (origin / BLOCK) as f32 * 0.125))
    })
}

fn assert_wet_timing(h: &ResidentHarness) {
    let (start, pcm) = render_playing(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        let (_, expected) = signed_output(start + offset);
        assert_eq!(
            frame,
            [expected; 2],
            "signed bridge frame={}",
            start + offset
        );
    }
}

fn assert_aligned_fade(h: &ResidentHarness, toward_dry: bool) {
    let (start, pcm) = render_playing(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        let (dry, wet) = signed_output(start + offset);
        let progress = (offset + 1).min(FADE) as f32 / FADE as f32;
        let dry_gain = if toward_dry { progress } else { 1.0 - progress };
        let expected = dry * dry_gain + wet * (1.0 - dry_gain);
        assert!(
            frame.into_iter().all(|lane| (lane - expected).abs() < 1e-6),
            "aligned fade at {}: {frame:?} vs {expected}",
            start + offset
        );
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_main_vst3_removal_retires_a_worker_but_keeps_pass_latency() {
    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    let selected = selection();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selected, "Fx", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: BRIDGE as u32,
        }),
        48_000,
        directory.join("plugin-catalog.json"),
        u64::from(SOURCE_FRAMES),
    );
    assert!(h.next().error.is_none());
    let (empty, _) = h.applied(0);
    let imported = h.run(Request::Import(impulse_wav(&files)));
    let source = imported.state.canonical_audio_source.as_ref().unwrap();
    assert!(source.required_sources().next().is_some());
    assert!(
        source
            .required_sources()
            .all(|(_, source)| { matches!(source, hex_session::AudioSourceBinding::Resident(_)) })
    );
    let (dry, _) = h.applied(empty);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    assert!(inserted.plugin_effect_complete[0].accepted);
    h.applied(dry);
    let Some(Created::Device(device)) = inserted.created else {
        panic!("fixture VST3");
    };
    let predecessor = status(&h).recording_source().unwrap();
    let observation = h.submitted_observation().unwrap();
    let kind = inserted
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .type_id();
    assert_eq!(
        observation.foreign_processor(device.value(), kind),
        Some(true)
    );
    assert_eq!(
        observation
            .processor_latency(device.value(), kind)
            .unwrap()
            .stage_frames,
        BRIDGE as u32
    );
    wait(|| h.plugin_progress.lock().unwrap().source_bindings[0].is_some());
    let (original_device, binding, _) =
        h.plugin_progress.lock().unwrap().source_bindings[0].unwrap();
    assert_eq!(original_device, device.value());
    h.enable_confirmed_device_inserts();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        assert_wet_timing(&h);
    }
    assert_eq!(status(&h).recording_source(), Some(predecessor));

    for cancel in [true, false] {
        let (submitted, resume) = h.probe_next_submitted_device_insert();
        h.service
            .submit(Request::Edit(Command::RemoveDevice(device)))
            .unwrap();
        let exact = submitted.recv_timeout(Duration::from_secs(1)).unwrap();
        resume.send(()).unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.request_complete);
        assert_eq!(pending.state.revision, inserted.state.revision);
        assert_eq!(pending.state.project, inserted.state.project);
        assert_eq!(exact.output_identity, predecessor.output_identity);
        assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
        assert_aligned_fade(&h, true);
        assert_eq!(status(&h).recording_source(), Some(predecessor));
        if !cancel {
            break;
        }
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
        assert_aligned_fade(&h, false);
        let cancelled = h.next();
        assert_eq!(
            cancelled.device_insert_complete.unwrap().outcome,
            DeviceInsertOutcome::Cancelled
        );
        assert_eq!(cancelled.state.project, inserted.state.project);
        assert_eq!(status(&h).recording_source(), Some(predecessor));
        assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
        assert_wet_timing(&h);
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    let removed = loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(
                event.device_insert_confirmation.is_none(),
                "Main removal required a timing restart after anchoring the live 512-frame impulse"
            );
            if event.request_complete {
                break event;
            }
            assert_eq!(event.state.project, inserted.state.project);
        }
        if status(&h).recording_source() == Some(predecessor) {
            render_playing(&h);
        }
        assert!(Instant::now() < deadline, "Main removal terminal timed out");
        std::thread::sleep(Duration::from_millis(1));
    };
    let completion = removed.device_insert_complete.unwrap();
    assert_eq!(completion.outcome, DeviceInsertOutcome::Installed);
    assert!(removed.state.project.device(device).is_none());
    assert_eq!(removed.state.revision, inserted.state.revision + 1);
    let applied = status(&h).recording_source().unwrap();
    assert_ne!(applied, predecessor);
    assert_eq!(applied.output_identity, predecessor.output_identity);
    assert_eq!(applied.context, completion.submission.context);
    assert_eq!(applied.audio_revision, completion.submission.audio_revision);
    let deadline = Instant::now() + Duration::from_secs(1);
    while h.plugin_records.load(Acquire) != 0 {
        assert_dry_timing(&h, BRIDGE);
        h.pump();
        assert!(
            Instant::now() < deadline,
            "the original worker did not retire"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(h.plugin_progress.lock().unwrap().binding.is_none());
    for _ in 0..4 {
        assert_dry_timing(&h, BRIDGE);
    }

    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();
    assert!(!status(&h).playing.load(Acquire));
    assert_eq!(status(&h).position.load(Acquire), 0);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        assert_dry_timing(&h, 0);
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_main_power_off_retires_the_worker_but_keeps_pass_latency() {
    let _native = native_fixture();
    let files = Files::new();
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
        u64::from(SOURCE_FRAMES),
    );
    assert!(h.next().error.is_none());
    let (empty, _) = h.applied(0);
    let imported = h.run(Request::Import(impulse_wav(&files)));
    let (dry, _) = h.applied(empty);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    assert!(inserted.plugin_effect_complete[0].accepted);
    h.applied(dry);
    let Some(Created::Device(device)) = inserted.created else {
        panic!("fixture VST3");
    };
    let predecessor = status(&h).recording_source().unwrap();
    let state = inserted.state.project.device(device).unwrap().state();
    let binding = h.plugin_progress.lock().unwrap().binding.unwrap();
    assert_eq!(h.plugin_records.load(Acquire), 1);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .processor_latency(device.value(), state.type_id())
            .unwrap()
            .stage_frames,
        BRIDGE as u32
    );
    h.enable_confirmed_device_inserts();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        assert_wet_timing(&h);
    }

    // No callback runs while the first terminal/progress event is received.
    // Ordinary running bypass currently completes here and keeps its worker.
    // Genuine power Off must instead wait for one exact installed graph.
    // Hold the actual ordinary receipt reader on this original worker. The
    // same reserved retirement lease must claim it only after installation;
    // clearing the read or requiring an accidentally idle polling turn fails.
    h.plugin_progress.lock().unwrap().hold_pending_read = Some(binding);
    wait(|| h.plugin_progress.lock().unwrap().held_pending_read == Some(binding));
    h.service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device,
            bypassed: true,
        }))
        .unwrap();
    let pending = h.next();
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert!(
        !pending.request_complete,
        "running Off completed before ACK"
    );
    assert_eq!(pending.state.project, inserted.state.project);
    assert_eq!(pending.state.revision, inserted.state.revision);
    assert_eq!(status(&h).recording_source(), Some(predecessor));
    assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
    assert_eq!(h.plugin_records.load(Acquire), 1);
    assert_aligned_fade(&h, true);
    assert_eq!(status(&h).recording_source(), Some(predecessor));

    let deadline = Instant::now() + Duration::from_secs(1);
    let powered_off = loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.device_insert_confirmation.is_none());
            if event.request_complete {
                break event;
            }
            assert_eq!(event.state.project, inserted.state.project);
        }
        if status(&h).recording_source() == Some(predecessor) {
            render_playing(&h);
        }
        assert!(Instant::now() < deadline, "running Off terminal timed out");
        std::thread::yield_now();
    };
    let completion = powered_off.device_insert_complete.unwrap();
    assert_eq!(completion.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(powered_off.state.revision, inserted.state.revision + 1);
    let off_device = powered_off.state.project.device(device).unwrap();
    assert!(off_device.bypassed());
    assert_eq!(off_device.state(), state);
    let applied = status(&h).recording_source().unwrap();
    assert_ne!(applied, predecessor);
    assert_eq!(applied.output_identity, predecessor.output_identity);
    assert_eq!(applied.context, completion.submission.context);
    assert_eq!(applied.audio_revision, completion.submission.audio_revision);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .processor_status(device.value(), state.type_id()),
        Some(hex_engine::PreparedDeviceProcessing::PoweredOff)
    );
    assert!(status(&h).native_effect_source().is_none());

    let deadline = Instant::now() + Duration::from_secs(1);
    while h.plugin_records.load(Acquire) != 0 {
        assert_dry_timing(&h, BRIDGE);
        h.pump();
        assert!(
            Instant::now() < deadline,
            "powered-off worker stayed warm: frame={} records={} progress={:?}",
            status(&h).position.load(Acquire),
            h.plugin_records.load(Acquire),
            h.plugin_progress.lock().unwrap(),
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(h.plugin_progress.lock().unwrap().binding.is_none());
    let processed = h.plugin_completed.load(Acquire);
    for _ in 0..4 {
        assert_dry_timing(&h, BRIDGE);
    }
    assert_eq!(h.plugin_completed.load(Acquire), processed);

    // The next Play belongs to a fresh pass and can use the reduced latency.
    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();
    assert!(!status(&h).playing.load(Acquire));
    assert_eq!(status(&h).position.load(Acquire), 0);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        assert_dry_timing(&h, 0);
    }
}
