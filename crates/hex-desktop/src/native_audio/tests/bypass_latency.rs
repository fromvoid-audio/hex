//! Resident worker publication to a manually pumped callback; no physical device.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{Command, Created};

#[test]
fn recovery_enabled_bypass_reaches_pumped_callback_and_reports_terminal_timing() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_recovery(2_000_000, files.0.join("untitled.hex-journal"));
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 1_048_576)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(6.0)
    .unwrap();
    let inserted = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state,
    }));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("utility device")
    };
    (applied, _) = h.applied(applied);
    for index in 0..19 {
        h.run(Request::Edit(Command::AddTrack {
            name: format!("empty {index}"),
        }));
        (applied, _) = h.applied(applied);
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    let enabled = h.render()[63][0];
    assert!(enabled > 0.4);
    let mut audio_times = Vec::new();
    let mut terminal_times = Vec::new();
    for iteration in 0..22 {
        let bypassed = iteration % 2 == 0;
        let started = Instant::now();
        h.service
            .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
            .unwrap();
        let deadline = started + Duration::from_secs(2);
        let mut audio_time = None;
        let mut terminal_time = None;
        loop {
            h.pump();
            if audio_time.is_none() {
                let output = h.render();
                let revision = h
                    .audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .applied_revision
                    .load(std::sync::atomic::Ordering::Acquire);
                if revision > applied {
                    let expected = if bypassed { 0.25 } else { enabled };
                    assert!(
                        (output[63][0] - expected).abs() < 1e-4,
                        "iteration={iteration}, output={}, expected={expected}",
                        output[63][0]
                    );
                    audio_time = Some(started.elapsed().as_micros());
                    applied = revision;
                }
            }
            if let Ok(event) = h.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                assert_eq!(
                    event.state.project.device(device).unwrap().bypassed(),
                    bypassed
                );
                terminal_time = Some(started.elapsed().as_micros());
            }
            if let (Some(audio), Some(terminal)) = (audio_time, terminal_time) {
                if iteration >= 2 {
                    audio_times.push(audio);
                    terminal_times.push(terminal);
                }
                break;
            }
            assert!(
                Instant::now() < deadline,
                "bypass did not reach callback and terminal"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    audio_times.sort_unstable();
    terminal_times.sort_unstable();
    eprintln!(
        "recovery-enabled 20-track bypass, 48 kHz/64 frames, 2 warmups, 20 samples, manually pumped callback: audio p50/p95/max={} / {} / {} us; terminal p50/p95/max={} / {} / {} us",
        (audio_times[9] + audio_times[10]) / 2,
        audio_times[18],
        audio_times[19],
        (terminal_times[9] + terminal_times[10]) / 2,
        terminal_times[18],
        terminal_times[19]
    );
}

#[test]
fn utility_bypass_publication_reaches_next_pumped_callback() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(6.0)
    .unwrap();
    let inserted = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state,
    }));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("utility device")
    };
    (applied, _) = h.applied(applied);
    for index in 0..19 {
        h.run(Request::Edit(Command::AddTrack {
            name: format!("empty {index}"),
        }));
        (applied, _) = h.applied(applied);
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    let enabled = h.render()[63][0];
    assert!(enabled > 0.4, "non-unity Utility output: {enabled}");
    let mut times = Vec::new();
    for iteration in 0..22 {
        let bypassed = iteration % 2 == 0;
        let started = Instant::now();
        let event = h.run(Request::Edit(Command::SetDeviceBypass { device, bypassed }));
        let deadline = Instant::now() + Duration::from_millis(500);
        let (next, metadata) = loop {
            h.pump();
            if let Some((next, metadata)) = h.submitted.filter(|(next, _)| *next > applied) {
                break (next, metadata);
            }
            assert!(
                Instant::now() < deadline,
                "bypass publication was not staged"
            );
            std::thread::yield_now();
        };
        let output = h.render();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            next
        );
        assert_eq!(metadata.context.editor_revision, event.state.revision);
        assert_eq!(
            event.state.project.device(device).unwrap().bypassed(),
            bypassed
        );
        let expected = if bypassed { 0.25 } else { enabled };
        assert!(
            (output[63][0] - expected).abs() < 1e-4,
            "bypassed={bypassed} output={:?}",
            output[63]
        );
        if iteration >= 2 {
            times.push(started.elapsed().as_micros());
        }
        applied = next;
    }
    times.sort_unstable();
    eprintln!(
        "utility bypass worker-to-callback: 20 tracks, one audio and one +6 dB Utility, 48 kHz, 64-frame manually pumped blocks, 2 warmups, 20 samples, no recovery or physical device; p50_us={} p95_us={} max_us={}",
        (times[9] + times[10]) / 2,
        times[18],
        times[19]
    );
}
