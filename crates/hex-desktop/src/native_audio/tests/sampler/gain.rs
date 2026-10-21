//! Actual shared numeric widget -> Session -> sealed fresh native CURRENT.
use super::*;
use gpui::{Modifiers, MouseButton, VisualTestContext};
pub(super) fn exercise(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    window: gpui::AnyWindowHandle,
    end: Instant,
    track: hex_project::TrackId,
    saved: &std::path::Path,
) {
    let stored = hex_storage::load(saved).unwrap();
    let original = stored.track(track).unwrap().instrument().unwrap();
    let device = original.id();
    let selector = Box::leak(format!("sampler-gain-{}-value", device.value()).into_boxed_str());
    let target_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    // The fixture reopens through the actual menu; select its sampler track.
    let bounds = visual
        .debug_bounds(target_selector)
        .expect("actual sampler track header");
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let pulse = |visual: &mut VisualTestContext| {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let mut slot = h.backend.lock().unwrap();
        if let Some(backend) = slot.as_mut() {
            if backend.shutdown_requested() {
                slot.take();
            } else {
                let measured =
                    allocation_counter::measure(|| backend.render(&mut [[0.0; 2]; 64]).unwrap());
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
            }
        }
        drop(slot);
        std::thread::sleep(Duration::from_millis(1));
    };
    let initial = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_audio_source();
    for draft in ["2.5", "nan"] {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(draft);
        if draft == "nan" {
            visual.simulate_keystrokes("enter");
        }
        for _ in 0..4 {
            pulse(visual);
        }
        assert_eq!(
            h.backend
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source(),
            initial,
            "unconfirmed or invalid local numeric preview must not publish/edit"
        );
        assert!(h.observation.lock().unwrap().gain_cells.is_empty());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
    }
    for gain in [0.5, 0.0] {
        let old = h
            .backend
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .unwrap();
        let bounds = visual
            .debug_bounds(selector)
            .expect("actual sampler numeric gain field");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(&gain.to_string());
        visual.simulate_keystrokes("enter");
        loop {
            pulse(visual);
            if h.backend
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|b| b.status())
                .and_then(|s| s.applied_audio_source())
                .is_some_and(|s| {
                    s.context.project_generation == old.context.project_generation
                        && s.context.editor_revision == old.context.editor_revision + 1
                        && s.output_identity != old.output_identity
                })
            {
                break;
            }
            assert!(
                Instant::now() < end,
                "gain did not reach fresh CURRENT: {:?}",
                cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            );
        }
        for _ in 0..12 {
            pulse(visual);
        }
        visual.simulate_keystrokes("cmd-s");
        loop {
            pulse(visual);
            let project = hex_storage::load(saved).unwrap();
            let state = project.device(device).unwrap().state();
            if hex_engine::sampler_gain_on_control(state).unwrap() == gain {
                assert_eq!(
                    state.asset_references(),
                    original.state().asset_references()
                );
                assert_eq!(state.input_gain_db(), original.state().input_gain_db());
                assert_eq!(state.output_gain_db(), original.state().output_gain_db());
                assert_eq!(
                    hex_engine::patch_sampler_gain_on_control(state, 1.0).unwrap(),
                    *original.state()
                );
                break;
            }
            assert!(
                Instant::now() < end,
                "actual SaveCurrent did not persist gain: {:?}",
                cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            );
        }
        if gain == 0.5 {
            let before = h
                .backend
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source()
                .unwrap();
            visual.simulate_keystrokes("cmd-n");
            visual.run_until_parked();
            visual.simulate_keystrokes("cmd-n");
            while visual.debug_bounds(target_selector).is_some() {
                pulse(visual);
                assert!(Instant::now() < end, "gain New did not complete");
            }
            for _ in 0..8 {
                pulse(visual);
            }
            visual.simulate_keystrokes("cmd-o");
            while !cx.did_prompt_for_paths() {
                pulse(visual);
                assert!(Instant::now() < end, "gain reopen picker did not open");
            }
            cx.simulate_paths_selection(Some(vec![saved.to_path_buf()]));
            loop {
                pulse(visual);
                if h.backend
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|b| b.status())
                    .and_then(|s| s.applied_audio_source())
                    .is_some_and(|a| {
                        a.context.project_generation == before.context.project_generation + 2
                            && a.output_identity != before.output_identity
                    })
                {
                    break;
                }
                assert!(
                    Instant::now() < end,
                    "gain reopen did not reach actual CURRENT"
                );
            }
            for _ in 0..8 {
                pulse(visual);
            }
            let bounds = visual.debug_bounds(target_selector).unwrap();
            visual.simulate_click(bounds.center(), Modifiers::default());
        }
        // Capture first key render without pumping away this short source.
        let toggle = visual.debug_bounds("computer midi keyboard").unwrap();
        visual.simulate_click(toggle.center(), Modifiers::default());
        visual.simulate_keystrokes("v v");
        visual.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("a").unwrap(),
            is_held: false,
        });
        loop {
            cx.background_executor
                .advance_clock(Duration::from_millis(16));
            visual.run_until_parked();
            if cx
                .update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
                .unwrap()
                .contains("ready=true")
            {
                break;
            }
            assert!(
                Instant::now() < end,
                "gain keyboard activation did not complete"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut output = [[0.0; 2]; 64];
        let mut slot = h.backend.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        let measured = allocation_counter::measure(|| backend.render(&mut output).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let expected = [
            [0.125, -0.25],
            [0.3125, -0.125],
            [0.5, 0.0],
            [0.1875, 0.125],
            [-0.125, 0.25],
            [-0.0625, 0.125],
            [0.0, 0.0],
        ];
        for (i, frame) in output.iter().enumerate() {
            for (a, e) in frame
                .iter()
                .zip(expected.get(i).copied().unwrap_or([0.0; 2]))
            {
                assert!(
                    (*a - e * gain as f32).abs() < 1e-6,
                    "gain {gain} frame{i}: {frame:?}"
                );
            }
        }
        drop(slot);
        visual.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("a").unwrap(),
        });
        visual.simulate_keystrokes("m");
        for _ in 0..4 {
            pulse(visual);
        }
        visual.simulate_keystrokes("space");
        visual.run_until_parked();
        let mut slot = h.backend.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        let mut arranged = [[0.0; 2]; 64];
        let measured = allocation_counter::measure(|| backend.render(&mut arranged).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(
            backend
                .status()
                .unwrap()
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
        );
        for (i, frame) in arranged.iter().enumerate() {
            for (a, e) in frame
                .iter()
                .zip(expected.get(i).copied().unwrap_or([0.0; 2]))
            {
                assert!(
                    (*a - e * gain as f32).abs() < 1e-6,
                    "arrangement gain {gain} frame{i}: {frame:?}"
                );
            }
        }
        drop(slot);
        visual.simulate_keystrokes("escape");
        for _ in 0..4 {
            pulse(visual);
        }
        {
            let observation = h.observation.lock().unwrap();
            assert!(
                observation
                    .gain_cells
                    .iter()
                    .all(|(before, after)| before == after)
            );
            assert_eq!(
                observation.gain_cells.len(),
                if gain == 0.5 { 1 } else { 2 }
            );
        }
        let slot = h.backend.lock().unwrap();
        let status = slot.as_ref().unwrap().status().unwrap();
        assert!(!status.playing.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            status.position.load(std::sync::atomic::Ordering::Acquire),
            0
        );
    }
}

fn current(
    limit: u64,
) -> (
    Files,
    Harness,
    hex_project::TrackId,
    hex_project::DeviceId,
    Arc<hex_session::service::ViewState>,
) {
    let files = Files::new();
    let mut h = Harness::resident_sampler(limit);
    let (track, asset) = authored(&mut h, &files, true, false, false);
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let assigned = h.run(Request::AssignSampler { track, asset });
    let device = assigned
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    h.open_sampler(old);
    h.applied(h.context.unwrap());
    (files, h, track, device, assigned.state)
}
fn edit(
    state: &hex_session::service::ViewState,
    track: hex_project::TrackId,
    device: hex_project::DeviceId,
) -> hex_session::service::SamplerGainEdit {
    hex_session::service::SamplerGainEdit {
        request_id: 1,
        project_generation: state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        revision: state.revision,
        track,
        device,
        gain: 0.5,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
fn unchanged(before: &hex_session::service::ViewState, after: &hex_session::service::Event) {
    assert_eq!(after.state.revision, before.revision);
    assert_eq!(after.state.project.snapshot(), before.project.snapshot());
    assert_eq!(
        (after.state.can_undo, after.state.can_redo),
        (before.can_undo, before.can_redo)
    );
    assert_eq!(after.state.document_path, before.document_path);
}
fn original_pcm(h: &mut Harness) {
    let identity = h.audio.as_ref().unwrap().control.output_identity();
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let pcm = h.render();
    assert_eq!(pcm[0], [0.125, -0.25]);
    assert_eq!(pcm[2], [0.5, 0.0]);
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        identity
    );
}
#[test]
fn sampler_gain_stale_invalid_cancelled_and_tight_budget_refusals_preserve_actual_current() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(8);
    let identity = h.audio.as_ref().unwrap().control.output_identity();
    for case in 0..6 {
        let mut gain = edit(&before, track, device);
        match case {
            0 => gain.gain = f64::NAN,
            1 => gain.gain = 4.01,
            2 => gain.gain = -0.1,
            3 => gain.revision += 1,
            4 => gain.project_generation += 1,
            _ => gain
                .cancel
                .store(true, std::sync::atomic::Ordering::Release),
        }
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::SetSamplerGain(gain))
            .unwrap();
        let refused = h.next();
        assert!(refused.error.is_some());
        unchanged(&before, &refused);
        assert_eq!(refused.sampler_gain_complete, Some(1));
    }
    original_pcm(&mut h);
    let initial = h.run(Request::SetSamplerGain(edit(&before, track, device)));
    assert!(initial.sampler_gain_complete.is_none());
    unchanged(&before, &initial);
    h.render();
    let refused = h.next();
    assert!(
        refused.error.as_ref().unwrap().contains("frame limit"),
        "{:?}",
        refused.error
    );
    assert!(
        !refused.request_complete,
        "worker continuation is not another UI acknowledgement"
    );
    unchanged(&before, &refused);
    assert_eq!(refused.sampler_gain_complete, Some(1));
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        identity
    );
    assert_eq!(h.observation.lock().unwrap().native_hold.frames, 4);
    original_pcm(&mut h);
}
#[test]
fn sampler_gain_fence_observes_earlier_play_and_refuses_later_play_without_replay() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(12);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.run(Request::SetSamplerGain(edit(&before, track, device)));
    assert_eq!(h.render()[0], [0.125, -0.25]);
    let refused = h.next();
    assert!(refused.error.as_ref().unwrap().contains("stop playback"));
    unchanged(&before, &refused);
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    let gain = edit(&before, track, device);
    let cancel = Arc::clone(&gain.cancel);
    h.run(Request::SetSamplerGain(gain));
    assert!(
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .is_err()
    );
    cancel.store(true, std::sync::atomic::Ordering::Release);
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let cancelled = h.next();
    assert!(cancelled.error.is_some());
    unchanged(&before, &cancelled);
    for _ in 0..3 {
        assert_eq!(h.render(), [[0.0; 2]; 64]);
    }
    original_pcm(&mut h);
}

#[test]
fn sampler_gain_postcommit_open_failure_keeps_authored_gain_save_export_and_explicit_retry() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, device, before) = current(12);
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.observation.lock().unwrap().fail_gain_open = true;
    h.run(Request::SetSamplerGain(edit(&before, track, device)));
    h.render();
    let accepted = h.next();
    assert!(accepted.error.is_none());
    assert_eq!(accepted.state.revision, before.revision + 1);
    assert_eq!(
        hex_engine::sampler_gain_on_control(accepted.state.project.device(device).unwrap().state())
            .unwrap(),
        0.5
    );
    let context = h.context.unwrap();
    let end = Instant::now() + h.wait;
    loop {
        match h.events.try_recv() {
            Ok(EditorAudioEvent::RelinquishClient { .. }) => {
                h.audio = None;
                h.backend.lock().unwrap().take();
            }
            Ok(EditorAudioEvent::Unavailable { error, .. }) => {
                assert!(error.contains("fixture hardware open failed"));
                h.audio = None;
                h.backend.lock().unwrap().take();
                break;
            }
            Ok(EditorAudioEvent::Opened { .. }) => {
                panic!("failed hardware open must not report an opened/applied output")
            }
            _ => {}
        }
        assert!(
            Instant::now() < end,
            "postcommit hardware failure was not reported"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let saved = files.0.join("gain-after-open-failure.hex");
    h.run(Request::SaveNew(saved.clone()));
    assert_eq!(
        hex_engine::sampler_gain_on_control(
            hex_storage::load(&saved)
                .unwrap()
                .device(device)
                .unwrap()
                .state()
        )
        .unwrap(),
        0.5
    );
    let path = files.0.join("gain-after-open-failure.wav");
    h.run(Request::ExportRange {
        path: path.clone(),
        start: 0,
        frames: Some(8),
        signal: hex_session::service::ExportSignal::Main,
        overwrite: false,
        control: hex_export_control(),
    });
    loop {
        if let Ok(event) = h.service.as_ref().unwrap().try_export_event() {
            assert!(event.result.is_ok(), "{:?}", event.result);
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    let bytes = fs::read(path).unwrap();
    assert_eq!(i16::from_le_bytes([bytes[44], bytes[45]]), 2048);
    assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), -4096);
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::RestartAudio {
            project_generation: context.project_generation,
            request_id: 77,
        })
        .unwrap();
    let retry = h.next();
    assert!(retry.error.is_none(), "{:?}", retry.error);
    loop {
        match h.events.try_recv() {
            Ok(EditorAudioEvent::Opened { audio, .. }) => h.audio = Some(audio),
            Ok(EditorAudioEvent::RelinquishClient { .. }) => {
                h.audio = None;
                h.backend.lock().unwrap().take();
            }
            Ok(EditorAudioEvent::Unavailable { error, .. }) => panic!("retry unavailable: {error}"),
            _ => {}
        }
        let mut slot = h.backend.lock().unwrap();
        if let Some(backend) = slot.as_mut() {
            if backend.shutdown_requested() {
                slot.take();
            } else {
                let measured =
                    allocation_counter::measure(|| backend.render(&mut [[0.0; 2]; 64]).unwrap());
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
            }
        }
        drop(slot);
        if h.audio
            .as_ref()
            .and_then(|a| a.status())
            .and_then(|s| s.applied_audio_source())
            .is_some_and(|s| s.context == context && s.output_identity != old)
        {
            break;
        }
        assert!(
            Instant::now() < end,
            "explicit retry did not reach actual CURRENT"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render()[0], [0.0625, -0.125]);
}

#[test]
fn sampler_gain_lost_callback_timeout_preserves_earlier_play_and_allows_new_teardown() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(12);
    let old_output = h.audio.as_ref().unwrap().control.output_identity();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.run(Request::SetSamplerGain(edit(&before, track, device)));
    h.observation.lock().unwrap().expire_gain_fence = true;
    let timeout = h.next();
    assert!(timeout.error.as_ref().unwrap().contains("timed out"));
    unchanged(&before, &timeout);
    assert!(
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .is_err()
    );
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .control
            .reserve_sampler_keyboard_epoch()
            .is_err()
    );
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::SetSamplerGain(edit(&before, track, device)))
        .unwrap();
    let busy = h.next();
    assert!(busy.error.as_ref().unwrap().contains("busy"));
    unchanged(&before, &busy);
    // The callback returns only now: the Play admitted before the timed-out
    // fence is still honored, not reclassified as a raced new command.
    assert_eq!(h.render()[0], [0.125, -0.25]);
    let end = Instant::now() + h.wait;
    loop {
        if h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .is_ok()
        {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio.as_ref().unwrap().control.stop();
    for _ in 0..3 {
        assert_eq!(h.render(), [[0.0; 2]; 64]);
    }
    // A second callback loss may be abandoned through actual New/output
    // teardown; the old scalar fence is never treated as a source receipt.
    h.run(Request::SetSamplerGain(edit(&before, track, device)));
    h.observation.lock().unwrap().expire_gain_fence = true;
    let timeout = h.next();
    assert!(timeout.error.as_ref().unwrap().contains("timed out"));
    let new = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(new.project_replaced);
    assert!(new.state.project.tracks().is_empty());
    h.open();
    // The empty New project uses the ordinary path, which deliberately has no
    // constructor AppliedAudioSource. A genuinely opened different output
    // proves the old exclusive lease was retired, not a fabricated sampler ACK.
    assert_ne!(
        h.audio.as_ref().unwrap().control.output_identity(),
        old_output
    );
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    assert_eq!(h.observation.lock().unwrap().native_hold.frames, 0);
}

#[test]
fn sampler_gain_cancel_after_real_sealing_and_fenced_late_keyboard_preserve_old_current() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(12);
    let output = h.audio.as_ref().unwrap().control.output_identity();
    let gain = edit(&before, track, device);
    h.observation.lock().unwrap().cancel_gain_after_seal = Some(Arc::clone(&gain.cancel));
    h.run(Request::SetSamplerGain(gain));
    h.render();
    let cancelled = h.next();
    assert!(cancelled.error.as_ref().unwrap().contains("cancelled"));
    unchanged(&before, &cancelled);
    assert!(
        h.observation.lock().unwrap().gain_prepared.is_some(),
        "must reach actual sealed cell before cancellation"
    );
    assert!(
        h.observation.lock().unwrap().gain_cells.is_empty(),
        "cancelled cell must never enter output drain/adoption"
    );
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
    original_pcm(&mut h);
    // Reserve before the fence but deliver activation after it began. This
    // exact old output/target must still refuse the late keyboard activation.
    let (identity, epoch) = h
        .audio
        .as_ref()
        .unwrap()
        .control
        .reserve_sampler_keyboard_epoch()
        .unwrap();
    let input = Arc::new(
        hex_engine::computer_keyboard::ComputerKeyboardInput::with_native_sampler_activation(
            identity, epoch,
        )
        .unwrap(),
    );
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let gain = edit(&before, track, device);
    let cancel = Arc::clone(&gain.cancel);
    h.run(Request::SetSamplerGain(gain));
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::PrepareComputerKeyboard {
            project_generation: before
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            revision: before.revision,
            request_id: 99,
            track,
            input: Arc::clone(&input),
        })
        .unwrap();
    let late = h.next();
    assert!(late.error.is_some());
    assert_eq!(late.computer_keyboard_complete, Some(99));
    assert!(input.is_cancelled());
    cancel.store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let cancelled = h.next();
    assert!(cancelled.error.is_some());
    unchanged(&before, &cancelled);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    original_pcm(&mut h);
}
