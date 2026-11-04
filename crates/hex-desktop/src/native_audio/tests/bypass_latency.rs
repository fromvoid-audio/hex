//! Resident worker publication to a manually pumped callback; no physical device.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{Command, Created};

#[test]
fn live_zero_tail_devices_bypass_without_a_step_in_the_actual_callback() {
    let _native = native_fixture();
    for kind in ["hex.auto-pan", "hex.gate", "hex.compressor"] {
        let files = Files::new();
        let mut h = ResidentHarness::with_frame_limit(131_072);
        h.next();
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
        (applied, _) = h.applied(applied);
        let track = imported.state.project.tracks()[0].id();
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static(kind),
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
            panic!("device")
        };
        (applied, _) = h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .unwrap();
        for _ in 0..16 {
            h.render();
        }
        let before = h.render();
        let steady_step = before
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).abs())
            .fold(0.0_f32, f32::max);
        h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
        let after = h.render();
        let first_revision = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire);
        assert!(first_revision > applied);
        assert!(
            (after[0][0] - before[63][0]).abs() <= steady_step * 2.0 + 0.01,
            "{kind}: {} -> {}",
            before[63][0],
            after[0][0]
        );
        for _ in 0..4 {
            h.render();
        }
        let dry = h.render()[63][0];
        assert!((dry - 0.25).abs() < 1e-3, "{kind}: {dry}");
        h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
        let resumed = h.render();
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire)
                > first_revision
        );
        assert!(
            (resumed[0][0] - dry).abs() < 0.01,
            "{kind}: resumed {}",
            resumed[0][0]
        );
    }
}

#[test]
fn live_eq_bypass_transition_keeps_output_continuous() {
    let _native = native_fixture();
    for owner in 0..3 {
        let files = Files::new();
        let mut h = ResidentHarness::with_frame_limit(131_072);
        h.next();
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
        (applied, _) = h.applied(applied);
        let track = imported.state.project.tracks()[0].id();
        let mut eq = hex_engine::eq::EqState {
            output_gain: 2.0,
            ..Default::default()
        };
        eq.add(
            hex_engine::eq::Node::new(hex_engine::eq::NodeId::new(1).unwrap(), 1_000.0, 6.0, 0)
                .unwrap(),
        )
        .unwrap();
        let state = eq.encode().unwrap();
        let command = match owner {
            0 => Command::InsertDevice {
                track,
                before: None,
                state,
            },
            1 => {
                let added = h.run(Request::Edit(Command::AddReturnTrack {
                    name: "return".into(),
                }));
                (applied, _) = h.applied(applied);
                let Some(Created::ReturnTrack(return_track)) = added.created else {
                    panic!("return")
                };
                h.run(Request::Edit(Command::SetReturnSend {
                    track,
                    return_track,
                    gain: 1.0,
                }));
                (applied, _) = h.applied(applied);
                Command::InsertReturnDevice {
                    return_track,
                    before: None,
                    state,
                }
            }
            _ => Command::InsertMainDevice {
                before: None,
                state,
            },
        };
        let inserted = h.run(Request::Edit(command));
        let Some(Created::Device(device)) = inserted.created else {
            panic!("EQ device")
        };
        (applied, _) = h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .unwrap();
        for _ in 0..16 {
            h.render();
        }
        let before = h.render()[63][0];
        h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
        let after = h.render();
        let revision = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire);
        assert!(revision > applied, "bypass must reach the callback");
        assert!(
            (after[0][0] - before).abs() < 0.02,
            "discontinuity: {before} -> {}",
            after[0][0]
        );
        for _ in 0..4 {
            h.render();
        }
        let dry = h.render()[63][0];
        assert!((dry - if owner == 1 { 0.5 } else { 0.25 }).abs() < 1e-3);
        h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
        let resumed = h.render();
        assert!(
            (resumed[0][0] - dry).abs() < 0.02,
            "resume discontinuity: {dry} -> {}",
            resumed[0][0]
        );
        for _ in 0..4 {
            h.render();
        }
        assert!((h.render()[63][0] - if owner == 1 { 0.75 } else { 0.5 }).abs() < 1e-3);
    }
}

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
                    let from = if bypassed { enabled } else { 0.25 };
                    let target = if bypassed { 0.25 } else { enabled };
                    let expected = from + (target - from) * (64.0 / 240.0);
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
                for _ in 0..3 {
                    h.render();
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
        let from = if bypassed { enabled } else { 0.25 };
        let target = if bypassed { 0.25 } else { enabled };
        let expected = from + (target - from) * (64.0 / 240.0);
        assert!(
            (output[63][0] - expected).abs() < 1e-4,
            "bypassed={bypassed} output={:?}",
            output[63]
        );
        if iteration >= 2 {
            times.push(started.elapsed().as_micros());
        }
        for _ in 0..3 {
            h.render();
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

#[test]
fn eq_edit_then_bypass_reaches_the_actual_callback_smoothly() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let mut eq = hex_engine::eq::EqState {
        output_gain: 2.0,
        ..Default::default()
    };
    eq.add(
        hex_engine::eq::Node::new(hex_engine::eq::NodeId::new(1).unwrap(), 1_000.0, 6.0, 0)
            .unwrap(),
    )
    .unwrap();
    let inserted = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: eq.encode().unwrap(),
    }));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("eq device")
    };
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    for _ in 0..16 {
        h.render();
    }
    h.render();
    eq.output_gain = 3.0;
    h.run(Request::Edit(Command::ReplaceDeviceState {
        device,
        state: eq.encode().unwrap(),
    }));
    (applied, _) = h.applied(applied);
    for _ in 0..4 {
        h.render();
    }
    let before = h.render()[63][0];
    let final_event = h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
    let deadline = Instant::now() + Duration::from_secs(2);
    let (next, metadata) = loop {
        h.pump();
        if let Some((next, metadata)) = h.submitted.filter(|(next, metadata)| {
            *next > applied && metadata.context.editor_revision == final_event.state.revision
        }) {
            break (next, metadata);
        }
        assert!(
            Instant::now() < deadline,
            "bypass publication was not staged"
        );
        std::thread::yield_now();
    };
    let after = h.render();
    assert_eq!(metadata.context.editor_revision, final_event.state.revision);
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
    assert!(
        (after[0][0] - before).abs() < 0.02,
        "edit then bypass jumped: {before} -> {}",
        after[0][0]
    );
    for _ in 0..4 {
        h.render();
    }
    assert!((h.render()[63][0] - 0.25).abs() < 1e-3);
}

#[test]
fn publication_observation_tracks_worker_callback_admission_and_application() {
    use hex_session::service::{ResidentPublicationReadiness, ResidentRuntimeLoop};
    use std::sync::atomic::Ordering::Acquire;

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
    .unwrap();
    h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state,
    }));
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.submitted.is_none_or(|(revision, _)| revision <= applied) {
        h.pump();
        assert!(std::time::Instant::now() < end, "publication not submitted");
        std::thread::yield_now();
    }
    let (revision, metadata) = h.submitted.unwrap();
    let audio = h.audio.as_ref().unwrap();
    let submission = ResidentRuntimeLoop {
        output_identity: audio.control.output_identity().unwrap(),
        audio_revision: revision,
        source_context: Some(metadata.context),
        section_loop: None,
    };
    let observe = |h: &ResidentHarness| {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        super::super::resident::classify_publication(
            submission.output_identity,
            status.applied_revision.load(Acquire),
            status.rejected_revision().map(|(revision, _)| revision),
            status.publication_in_flight(),
            Some(submission),
        )
    };
    assert_eq!(
        observe(&h),
        ResidentPublicationReadiness::Pending {
            submission,
            applied_revision: applied,
            publication_in_flight: true,
        }
    );
    h.render();
    assert_eq!(
        observe(&h),
        ResidentPublicationReadiness::Applied {
            submission,
            publication_in_flight: false,
        }
    );
}

#[test]
fn eq_bypass_waits_for_inflight_parameter_publication() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let mut eq = hex_engine::eq::EqState {
        output_gain: 2.0,
        ..Default::default()
    };
    let inserted = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: eq.encode().unwrap(),
    }));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("eq device")
    };
    (applied, _) = h.applied(applied);
    eq.output_gain = 3.0;
    h.run(Request::Edit(Command::ReplaceDeviceState {
        device,
        state: eq.encode().unwrap(),
    }));
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.submitted.is_none_or(|(revision, _)| revision <= applied) {
        h.pump();
        assert!(
            Instant::now() < deadline,
            "parameter publication was not staged"
        );
        std::thread::yield_now();
    }
    let parameter_revision = h.submitted.unwrap().0;
    h.service
        .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
        .unwrap();
    let wait = Instant::now() + Duration::from_millis(100);
    let mut terminal = None;
    while Instant::now() < wait {
        h.pump();
        if let Ok(event) = h.service.try_event()
            && event.request_complete
        {
            assert!(event.error.is_none(), "{:?}", event.error);
            terminal = Some(event);
            break;
        }
        std::thread::yield_now();
    }
    h.render();
    let event = terminal.unwrap_or_else(|| h.next());
    assert!(event.error.is_none(), "{:?}", event.error);
    assert!(event.state.project.device(device).unwrap().bypassed());
    let deadline = Instant::now() + Duration::from_secs(2);
    while h
        .submitted
        .is_none_or(|(revision, _)| revision <= parameter_revision)
    {
        h.pump();
        assert!(
            Instant::now() < deadline,
            "bypass publication was not staged"
        );
        std::thread::yield_now();
    }
    let bypass_revision = h.submitted.unwrap().0;
    h.service
        .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
        .unwrap();
    h.render();
    let reversed = h.next();
    assert!(reversed.error.is_none(), "{:?}", reversed.error);
    assert!(!reversed.state.project.device(device).unwrap().bypassed());
    let (next, metadata) = h.applied(bypass_revision);
    assert_eq!(metadata.context.editor_revision, reversed.state.revision);
    assert!(next > bypass_revision);
}

#[test]
fn stop_remains_effective_while_a_device_edit_waits() {
    use std::sync::atomic::Ordering::Acquire;
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let mut eq = hex_engine::eq::EqState {
        output_gain: 2.0,
        ..Default::default()
    };
    let inserted = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: eq.encode().unwrap(),
    }));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("eq device")
    };
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    for _ in 0..8 {
        h.render();
    }
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    eq.output_gain = 3.0;
    h.run(Request::Edit(Command::ReplaceDeviceState {
        device,
        state: eq.encode().unwrap(),
    }));
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.submitted.is_none_or(|(revision, _)| revision <= applied) {
        h.pump();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    h.service
        .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
        .unwrap();
    let wait = Instant::now() + Duration::from_millis(50);
    while Instant::now() < wait {
        if let Ok(event) = h.service.try_event()
            && event.request_complete
        {
            panic!("device request unexpectedly completed: {:?}", event.error);
        }
        std::thread::yield_now();
    }
    for _ in 0..32 {
        h.service
            .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
            .unwrap();
    }
    assert!(matches!(
        h.service
            .submit(Request::Edit(Command::ToggleDeviceBypass { device })),
        Err(hex_session::service::SubmitError::Full(_))
    ));
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    for _ in 0..8 {
        h.render();
    }
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
}

#[test]
fn parked_device_edit_cancels_if_document_changes_before_admission() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let mut eq = hex_engine::eq::EqState {
        output_gain: 2.0,
        ..Default::default()
    };
    let inserted = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: eq.encode().unwrap(),
    }));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("eq device")
    };
    (applied, _) = h.applied(applied);
    eq.output_gain = 3.0;
    h.run(Request::Edit(Command::ReplaceDeviceState {
        device,
        state: eq.encode().unwrap(),
    }));
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.submitted.is_none_or(|(revision, _)| revision <= applied) {
        h.pump();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    h.service
        .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
        .unwrap();
    // Ensure the worker has time to park before the callback applies the predecessor.
    let wait = Instant::now() + Duration::from_millis(50);
    while Instant::now() < wait {
        if let Ok(event) = h.service.try_event()
            && event.request_complete
        {
            panic!("early terminal: {:?}", event.error);
        }
        std::thread::yield_now();
    }
    let (entered, resume, _) = h.probe_next_native_publication();
    h.render();
    entered
        .recv_timeout(Duration::from_secs(2))
        .expect("edit reached pre-admission gate");
    h.service
        .submit(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    resume.send(()).unwrap();
    let edit = h.next();
    assert!(
        edit.error
            .as_deref()
            .is_some_and(|error| error.contains("cancel")),
        "{:?}",
        edit.error
    );
    assert!(!edit.state.project.device(device).unwrap().bypassed());
    let replacement = h.next();
    assert!(replacement.error.is_none(), "{:?}", replacement.error);
    assert!(replacement.project_replaced);
}
