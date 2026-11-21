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

pub(super) const WORKERLESS_DRY_PROBE_FRAMES: usize = 514;

/// The SDK edit uses its actual 256-frame quantum. After real worker retirement,
/// the same passive graph must also accept tiny and cross-quantum callbacks.
pub(super) fn assert_workerless_dry_partition(
    h: &ResidentHarness,
    partition: usize,
    sample_at: impl Fn(usize) -> f32,
) {
    const FRAMES: usize = WORKERLESS_DRY_PROBE_FRAMES;
    let start = status(h).position.load(Acquire);
    let performance = status(h).performance_frame.load(Acquire);
    let mut pcm = [[0.0; 2]; FRAMES];
    let mut host = [0.0; 2 * FRAMES];
    let mut slot = h.callback.lock().unwrap();
    let callback = slot.as_mut().unwrap();
    let measured = allocation_counter::measure(|| {
        for chunk in pcm.chunks_mut(partition) {
            let output = &mut host[..chunk.len() * 2];
            callback.render_native_direct(chunk, output).unwrap();
            assert!(
                output
                    .chunks_exact(2)
                    .zip(chunk.iter())
                    .all(|(h, p)| h == p)
            );
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(status(h).position.load(Acquire), start + FRAMES as u64);
    assert_eq!(
        status(h).performance_frame.load(Acquire),
        performance + FRAMES as u64
    );
    assert!(status(h).playing.load(Acquire));
    for (offset, frame) in pcm.into_iter().enumerate() {
        let at = start as usize + offset;
        assert_eq!(
            frame,
            [sample_at(at); 2],
            "partition={partition} offset={offset}"
        );
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
    verify_running_main_power_cycle(MainPowerCheck::Off);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn cancelled_running_main_power_off_restores_the_worker_then_retry_installs() {
    verify_running_main_power_cycle(MainPowerCheck::CancelOff);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_main_power_on_installs_a_fresh_worker_at_its_exact_receipt() {
    verify_running_main_power_cycle(MainPowerCheck::OnAgain);
}

enum MainPowerCheck {
    Off,
    CancelOff,
    OnAgain,
}

fn verify_running_main_power_cycle(check: MainPowerCheck) {
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
    let cancelled_audio = if matches!(check, MainPowerCheck::CancelOff) {
        let (submitted, resume) = h.probe_next_submitted_device_insert();
        h.service
            .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
            .unwrap();
        let exact = submitted.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(exact.output_identity, predecessor.output_identity);
        resume.send(()).unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.request_complete);
        assert_eq!(pending.state.project, inserted.state.project);
        assert_aligned_fade(&h, true);
        assert_eq!(status(&h).recording_source(), Some(predecessor));
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
            cancelled.error.as_deref(),
            Some("device edit cancelled before installation")
        );
        let completion = cancelled.device_insert_complete.unwrap();
        assert_eq!(completion.submission, exact);
        assert_eq!(completion.outcome, DeviceInsertOutcome::Cancelled);
        assert_eq!(cancelled.state.project, inserted.state.project);
        assert_eq!(cancelled.state.revision, inserted.state.revision);
        assert_eq!(
            (cancelled.state.can_undo, cancelled.state.can_redo),
            (inserted.state.can_undo, inserted.state.can_redo)
        );
        assert_eq!(status(&h).recording_source(), Some(predecessor));
        assert_eq!(status(&h).native_effect_source(), Some(predecessor));
        let progress = *h.plugin_progress.lock().unwrap();
        assert_eq!(progress.binding, Some(binding));
        assert!(progress.accepted && !progress.retiring);
        assert_eq!(h.plugin_records.load(Acquire), 1);
        let processed = h.plugin_completed.load(Acquire);
        assert_wet_timing(&h);
        // Wet PCM waits for the actual endpoint. Its completion projection is
        // published later by the independent canonical-worker poll.
        let observed_by = Instant::now() + Duration::from_millis(250);
        loop {
            h.pump();
            let progress = *h.plugin_progress.lock().unwrap();
            if progress
                .source_bindings
                .iter()
                .flatten()
                .any(|(id, source, count)| {
                    *id == device.value()
                        && *source == binding
                        && count.is_some_and(|count| count > processed)
                })
                && h.plugin_completed.load(Acquire) > processed
            {
                assert!(progress.accepted && !progress.retiring);
                assert_eq!(progress.binding, Some(binding));
                break;
            }
            assert!(
                Instant::now() < observed_by,
                "restored worker prefix not observed: before={processed} now={} binding={binding:?} progress={progress:?}",
                h.plugin_completed.load(Acquire)
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        Some(exact.audio_revision)
    } else {
        None
    };
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
    assert!(
        cancelled_audio.is_none_or(|cancelled| completion.submission.audio_revision > cancelled)
    );
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
    for partition in [1, 257] {
        assert_workerless_dry_partition(&h, partition, |at| {
            at.checked_sub(BRIDGE).map_or(0.0, source_sample)
        });
    }
    assert_eq!(h.plugin_completed.load(Acquire), processed);

    if matches!(check, MainPowerCheck::OnAgain) {
        assert_running_main_power_on(&mut h, &powered_off, binding);
        return;
    }

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

fn assert_running_main_power_on(
    h: &mut ResidentHarness,
    powered_off: &Event,
    binding: hex_audio::plugin_effects::EffectBinding,
) {
    let off = &powered_off.state.project.main_devices()[0];
    let device = off.id();
    let applied = status(h).recording_source().unwrap();
    let (prepared, enqueue, _) = h.probe_next_native_publication();
    h.service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device,
            bypassed: false,
        }))
        .unwrap();
    // Let the existing control owner prepare the fresh worker while only the
    // original dry graph can render. The candidate is still held before enqueue.
    let preparation_deadline = Instant::now() + Duration::from_secs(2);
    while prepared.try_recv().is_err() {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                event.error.is_none() && !event.request_complete,
                "running On stopped in preparation: complete={}, status={:?}, error={:?}",
                event.request_complete,
                event.status,
                event.error
            );
            assert_eq!(event.state.project, powered_off.state.project);
        }
        assert_eq!(status(h).recording_source(), Some(applied));
        assert_dry_timing(h, BRIDGE);
        assert!(
            Instant::now() < preparation_deadline,
            "running On never reached native publication"
        );
        std::thread::sleep(Duration::from_millis(4));
    }
    assert_eq!(status(h).recording_source(), Some(applied));
    let (id, prepared_binding, accepted) = h.plugin_progress.lock().unwrap().newest_source.unwrap();
    assert_eq!(id, device.value());
    assert_ne!(prepared_binding, binding);
    assert!(!accepted);
    enqueue.send(()).unwrap();

    // No callback can have installed the candidate since it was enqueued.
    let pending = h.next();
    assert_eq!(status(h).recording_source(), Some(applied));
    assert!(status(h).native_effect_source().is_none());
    assert!(
        pending.error.is_none() && !pending.request_complete,
        "running On did not stage an exact install: complete={}, status={:?}, error={:?}",
        pending.request_complete,
        pending.status,
        pending.error
    );
    assert_eq!(pending.state.project, powered_off.state.project);
    assert_eq!(pending.state.revision, powered_off.state.revision);
    assert_eq!(pending.state.can_redo, powered_off.state.can_redo);
    assert_eq!(pending.state.can_undo, powered_off.state.can_undo);

    let deadline = Instant::now() + Duration::from_secs(2);
    let powered_on = 'receipt: loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            if let Some(confirmation) = &event.device_insert_confirmation {
                assert_eq!(
                    confirmation.operation(),
                    hex_session::service::GraphEditOperation::PowerDevice
                );
                assert_eq!(event.state.project, powered_off.state.project);
                assert_eq!(status(h).recording_source(), Some(applied));
                assert!(confirmation.proceed());
            }
            if event.request_complete {
                break 'receipt event;
            }
            assert_eq!(event.state.project, powered_off.state.project);
        }
        assert!(Instant::now() < deadline, "running On receipt timed out");
        if status(h).recording_source() == Some(applied) {
            let before = status(h).position.load(Acquire);
            h.render_plugin_block();
            let after = status(h).position.load(Acquire);
            assert!(
                (before..=before + BLOCK as u64).contains(&after),
                "main power on changed the musical position from {before} to {after}"
            );
        }
        std::thread::yield_now();
    };
    let completion = powered_on.device_insert_complete.unwrap();
    assert_eq!(completion.outcome, DeviceInsertOutcome::Installed);
    let active = status(h).recording_source().unwrap();
    assert_eq!(active.output_identity, applied.output_identity);
    assert_eq!(active.audio_revision, completion.submission.audio_revision);
    assert_eq!(active.context, completion.submission.context);
    assert_eq!(status(h).native_effect_source(), Some(active));
    let on_device = powered_on.state.project.device(device).unwrap();
    assert!(!on_device.bypassed());
    assert_eq!(on_device.state(), off.state());
    assert_eq!(powered_on.state.revision, powered_off.state.revision + 1);
    assert_eq!(h.plugin_records.load(Acquire), 1);
    let fresh = h.plugin_progress.lock().unwrap().binding.unwrap();
    assert_eq!(fresh, prepared_binding);
    for _ in 0..4 {
        render_playing(h);
    }
    assert_wet_timing(h);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn confirmed_two_track_main_power_off_retires_the_original_worker() {
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
    let wave = impulse_wav(&files);
    h.run(Request::Import(wave.clone()));
    let (first, _) = h.applied(empty);
    let imported = h.run(Request::Import(wave));
    let (dry, _) = h.applied(first);
    assert_eq!(imported.state.project.tracks().len(), 2);
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
    let [device] = inserted.state.project.main_devices() else {
        panic!("sole Main effect");
    };
    let device_id = device.id();
    let predecessor = status(&h).recording_source().unwrap();
    let binding = h.plugin_progress.lock().unwrap().binding.unwrap();
    assert_eq!(h.plugin_records.load(Acquire), 1);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .processor_latency(device_id.value(), device.state().type_id())
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
    let old_wet = |h: &ResidentHarness| {
        let (start, pcm) = render_playing(h);
        for (offset, frame) in pcm.into_iter().enumerate() {
            let (_, wet) = signed_output(start + offset);
            assert_eq!(
                frame,
                [2.0 * wet; 2],
                "two-track original wet frame={}",
                start + offset
            );
        }
        assert_eq!(status(h).recording_source(), Some(predecessor));
        assert_eq!(status(h).native_effect_source(), Some(predecessor));
        assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
        assert_eq!(h.plugin_records.load(Acquire), 1);
    };
    for _ in 0..4 {
        old_wet(&h);
    }

    let (private, enqueue, _) = h.probe_next_native_publication();
    h.service
        .submit(Request::Edit(Command::ToggleDeviceBypass {
            device: device_id,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        h.pump();
        if private.try_recv().is_ok() {
            break;
        }
        if let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.project, inserted.state.project);
            assert_eq!(event.state.revision, inserted.state.revision);
            assert!(
                !event.request_complete,
                "two-track Off completed before its required restart offer: {:?}",
                event.error
            );
        }
        old_wet(&h);
        assert!(Instant::now() < until, "two-track Off private preparation");
        std::thread::sleep(Duration::from_millis(1));
    }
    old_wet(&h);
    enqueue.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    let mut confirmed = None;
    let installed = loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            if event.request_complete {
                assert!(event.error.is_none(), "{:?}", event.error);
                break event;
            }
            assert!(event.error.is_none(), "{:?}", event.error);
            assert_eq!(event.state.project, inserted.state.project);
            assert_eq!(event.state.revision, inserted.state.revision);
            if let Some(offer) = event.device_insert_confirmation {
                assert!(confirmed.is_none(), "second restart offer for the same Off");
                assert_eq!(
                    offer.operation(),
                    hex_session::service::GraphEditOperation::PowerDevice
                );
                assert_eq!(
                    offer.submission().output_identity,
                    predecessor.output_identity
                );
                old_wet(&h);
                let (submitted, resume) = h.probe_next_submitted_device_insert();
                eprintln!("two-track off offered restart: {:?}", offer.submission());
                assert!(offer.proceed());
                let exact = submitted.recv_timeout(Duration::from_secs(1)).unwrap();
                assert_eq!(exact.context, offer.submission().context);
                assert_eq!(exact.output_identity, predecessor.output_identity);
                assert!(exact.audio_revision > offer.submission().audio_revision);
                assert_eq!(status(&h).recording_source(), Some(predecessor));
                assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
                confirmed = Some(exact);
                resume.send(()).unwrap();
            }
        }
        if status(&h).recording_source() == Some(predecessor) {
            if confirmed.is_some() {
                // Consent permits the existing 128-frame fade prefix and
                // frozen remainder before restart. This is not a live pass;
                // wait for its exact terminal before checking steady clocks.
                h.render_plugin_block();
                edits::workers_idle(&h);
            } else {
                let (start, pcm) = render_playing(&h);
                if status(&h).recording_source() == Some(predecessor) {
                    for (offset, frame) in pcm.into_iter().enumerate() {
                        assert_eq!(frame, [2.0 * signed_output(start + offset).1; 2]);
                    }
                }
            }
        }
        assert!(Instant::now() < until, "two-track Off runtime terminal");
        std::thread::sleep(Duration::from_millis(1));
    };
    let completion = installed.device_insert_complete.unwrap();
    assert_eq!(completion.outcome, DeviceInsertOutcome::Installed);
    let confirmed = confirmed.expect("two-track Off must offer and approve an exact restart");
    assert_eq!(completion.submission, confirmed);
    assert_eq!(installed.state.revision, inserted.state.revision + 1);
    let off = installed.state.project.device(device_id).unwrap();
    assert!(off.bypassed());
    assert_eq!(off.state(), device.state());
    assert_eq!(installed.state.project.main_devices().len(), 1);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .processor_status(device_id.value(), device.state().type_id()),
        Some(hex_engine::PreparedDeviceProcessing::PoweredOff)
    );
    let actual = status(&h).recording_source().unwrap();
    assert_eq!(
        (
            actual.context,
            actual.audio_revision,
            actual.output_identity
        ),
        (
            completion.submission.context,
            completion.submission.audio_revision,
            predecessor.output_identity
        )
    );
    let required_at_install = status(&h).native_effect_source();
    // The approved restart resumes this exact graph with no old Main delay.
    // Check its first full block as well as steady playback below.
    let (start, after) = render_playing(&h);
    h.pump();
    eprintln!(
        "two-track off installed: confirmed=true required_at_install={required_at_install:?} required_after={:?} frame={start} first_pcm={:?} nominal_dry={} worker={:?}",
        status(&h).native_effect_source(),
        &after[..4],
        2.0 * source_sample(start),
        h.plugin_progress.lock().unwrap()
    );
    assert!(
        required_at_install.is_none(),
        "powered-off graph retained the original required worker after restart"
    );
    for (offset, frame) in after.into_iter().enumerate() {
        assert_eq!(frame, [2.0 * source_sample(start + offset); 2]);
    }
    let retired_by = Instant::now() + Duration::from_secs(1);
    while h.plugin_records.load(Acquire) != 0 {
        render_playing(&h);
        h.pump();
        assert!(
            Instant::now() < retired_by,
            "two-track Off worker not retired: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(h.plugin_progress.lock().unwrap().binding.is_none());
    for _ in 0..4 {
        let (start, pcm) = render_playing(&h);
        for (offset, frame) in pcm.into_iter().enumerate() {
            assert_eq!(frame, [2.0 * source_sample(start + offset); 2]);
        }
    }
    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();
    assert!(!status(&h).playing.load(Acquire));
    assert_eq!(status(&h).position.load(Acquire), 0);
    assert!(status(&h).native_effect_source().is_none());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        let (start, pcm) = render_playing(&h);
        for (offset, frame) in pcm.into_iter().enumerate() {
            assert_eq!(frame, [2.0 * source_sample(start + offset); 2]);
        }
    }
}
