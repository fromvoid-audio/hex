//! Actual retained ADSR controls, original-source PCM and saved/reopened state.
use super::*;
use gpui::{Modifiers, MouseButton, VisualTestContext};
use hex_engine::SamplerParameter::{
    self, AttackMs, DecayMs, Gain, LoopEnabled, ReleaseMs, SustainPercent, Transpose,
};

pub(super) fn exercise(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    window: gpui::AnyWindowHandle,
    end: Instant,
    track: hex_project::TrackId,
    saved: &std::path::Path,
) {
    let initial_submissions = h.observation.lock().unwrap().resident_submitted.len();
    let initial = hex_storage::load(saved).unwrap();
    let original = initial.track(track).unwrap().instrument().unwrap();
    let device = original.id();
    let selector = |parameter: SamplerParameter| -> &'static str {
        Box::leak(format!("sampler-{}-{}-value", parameter.key(), device.value()).into_boxed_str())
    };
    let track_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let track_center = visual.debug_bounds(track_selector).unwrap().center();
    visual.simulate_click(track_center, Modifiers::default());
    visual.run_until_parked();
    let frame_selector =
        Box::leak(format!("instrument-device-{}", device.value()).into_boxed_str());
    let frame = visual.debug_bounds(frame_selector).unwrap();
    let grid = visual.debug_bounds("sampler-controls").unwrap();
    for parameter in [
        Gain,
        Transpose,
        SamplerParameter::StartFrame,
        SamplerParameter::EndFrame,
        LoopEnabled,
        SamplerParameter::LoopStartFrame,
        SamplerParameter::LoopEndFrame,
        SamplerParameter::LoopCrossfadeMs,
        AttackMs,
        DecayMs,
        SustainPercent,
        ReleaseMs,
    ] {
        let field = visual.debug_bounds(selector(parameter)).unwrap();
        assert!(
            field.left() >= grid.left()
                && field.right() <= grid.right()
                && field.top() >= grid.top()
                && field.bottom() <= grid.bottom(),
            "{parameter:?} clipped: {field:?} in {grid:?}"
        );
        assert!(field.right() <= frame.right() && field.bottom() <= frame.bottom());
    }
    let pulse = |visual: &mut VisualTestContext| {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let mut slot = h.backend.lock().unwrap();
        if let Some(backend) = slot.as_mut() {
            if backend.shutdown_requested() {
                slot.take();
            } else {
                render(backend, &mut [[0.0; 2]; 64]);
            }
        }
        drop(slot);
        std::thread::sleep(Duration::from_millis(1));
    };
    let input = |visual: &mut VisualTestContext, parameter, text: &str| {
        let field = visual.debug_bounds(selector(parameter)).unwrap();
        let rail = visual.debug_bounds("device-chain-scroll").unwrap();
        let delta = if field.left() < rail.left() {
            rail.left() - field.left() + gpui::px(4.0)
        } else if field.right() > rail.right() {
            rail.right() - field.right() - gpui::px(4.0)
        } else {
            gpui::px(0.0)
        };
        // The wide sampler intentionally scrolls in a narrow native window.
        // Reach the actual input instead of clicking its offscreen layout bounds.
        if delta != gpui::px(0.0) {
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: rail.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(delta, gpui::px(0.0))),
                ..Default::default()
            });
            visual.run_until_parked();
        }
        let field = visual.debug_bounds(selector(parameter)).unwrap();
        assert!(
            field.left() >= rail.left() && field.right() <= rail.right(),
            "{parameter:?} must be reachable before text entry: {field:?} in {rail:?}"
        );
        let center = field.center();
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(text);
        visual.simulate_keystrokes("enter");
    };
    // Invalid drafts and cancelled local previews never reach canonical/audio.
    let before = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rendered_current(h)
        .unwrap();
    for (parameter, draft) in [
        (AttackMs, "nan"),
        (DecayMs, "60000.1"),
        (SustainPercent, "100.1"),
        (ReleaseMs, "-0.1"),
    ] {
        input(visual, parameter, draft);
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
                .rendered_current(h),
            Some(before)
        );
        visual.simulate_keystrokes("escape");
    }
    assert_eq!(
        h.observation.lock().unwrap().resident_submitted.len(),
        initial_submissions
    );
    let mut expected = original.state().clone();
    let mut attack = false;
    let mut decay = false;
    let mut sustain = false;
    let mut release = false;
    for (parameter, value) in [
        (Gain, 0.5),
        (Transpose, -12.0),
        (LoopEnabled, 1.0),
        (AttackMs, 0.125),
        (DecayMs, 0.125),
        (SustainPercent, 50.0),
        (ReleaseMs, 0.125),
    ] {
        let old = h
            .backend
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .rendered_current(h)
            .unwrap();
        input(visual, parameter, &value.to_string());
        loop {
            pulse(visual);
            if h.backend
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|b| b.status())
                .and_then(|s| s.rendered_current(h))
                .is_some_and(|a| {
                    a.context.project_generation == old.context.project_generation
                        && a.context.editor_revision == old.context.editor_revision + 1
                        && a.output_identity == old.output_identity
                        && a.audio_revision > old.audio_revision
                })
            {
                break;
            }
            assert!(
                Instant::now() < end,
                "{parameter:?}: {:?}",
                cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            );
        }
        for _ in 0..8 {
            pulse(visual);
        }
        expected =
            hex_engine::patch_sampler_parameter_on_control(parameter, &expected, value).unwrap();
        visual.simulate_keystrokes("cmd-s");
        loop {
            pulse(visual);
            let stored = hex_storage::load(saved).unwrap();
            // Disk publication precedes the worker's completion event. Await
            // that acknowledgment before attempting the next control gesture.
            let ready = cx
                .update(|cx| hex_ui::sampler_midi_test_ready(window, cx))
                .unwrap();
            if stored.device(device).unwrap().state() == &expected && ready {
                assert_eq!(stored.tuning(), tuning(880.0));
                assert_eq!(
                    stored.device(device).unwrap().state().asset_references(),
                    original.state().asset_references()
                );
                break;
            }
            assert!(Instant::now() < end, "ADSR actual save did not finish");
        }
        if !parameter.is_envelope() {
            continue;
        }
        match parameter {
            AttackMs => attack = true,
            DecayMs => decay = true,
            SustainPercent => sustain = true,
            ReleaseMs => release = true,
            _ => {}
        }
        // Reconstruct every meaningful ADSR state through real New/Open and a
        // fresh native CURRENT before measuring it, not a metadata-only reload.
        let before = h
            .backend
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .rendered_current(h)
            .unwrap();
        visual.simulate_keystrokes("cmd-n");
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-n");
        while visual.debug_bounds(track_selector).is_some() {
            pulse(visual);
            assert!(Instant::now() < end);
        }
        for _ in 0..8 {
            pulse(visual);
        }
        visual.simulate_keystrokes("cmd-o");
        while !cx.did_prompt_for_paths() {
            pulse(visual);
            assert!(Instant::now() < end);
        }
        cx.simulate_paths_selection(Some(vec![saved.to_path_buf()]));
        loop {
            pulse(visual);
            if h.backend
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|b| b.status())
                .and_then(|s| s.rendered_current(h))
                .is_some_and(|a| {
                    a.context.project_generation == before.context.project_generation + 2
                        && a.output_identity != before.output_identity
                })
            {
                break;
            }
            assert!(Instant::now() < end, "ADSR Open did not reach CURRENT");
        }
        for _ in 0..8 {
            pulse(visual);
        }
        let track_center = visual.debug_bounds(track_selector).unwrap().center();
        visual.simulate_click(track_center, Modifiers::default());
        let expected_at = |i: usize| {
            // Literal original8k/output16k, A880, -12st whole-source loop.
            let raw = [
                [0.125, -0.25],
                [0.3125, -0.125],
                [0.5, 0.0],
                [0.1875, 0.125],
                [-0.125, 0.25],
                [-0.0625, 0.125],
                [0.0, 0.0],
                [0.0625, -0.125],
            ][i % 8];
            let envelope = if attack && i < 2 {
                [0.0, 0.5][i]
            } else if decay && i < 4 {
                if sustain { [1.0, 0.75][i - 2] } else { 1.0 }
            } else if sustain {
                0.5
            } else {
                1.0
            };
            [raw[0] * 0.5 * envelope, raw[1] * 0.5 * envelope]
        };
        let before_keyboard = h
            .backend
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .rendered_current(h)
            .unwrap();
        let toggle = visual.debug_bounds("computer midi keyboard").unwrap();
        visual.simulate_click(toggle.center(), Modifiers::default());
        visual.simulate_keystrokes("v v");
        visual.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("a").unwrap(),
            is_held: false,
        });
        let mut output = [[0.0; 2]; 64];
        loop {
            cx.background_executor
                .advance_clock(Duration::from_millis(16));
            visual.run_until_parked();
            let mut slot = h.backend.lock().unwrap();
            let backend = slot.as_mut().unwrap();
            super::envelope::render(backend, &mut output);
            if backend
                .status()
                .unwrap()
                .rendered_current(h)
                .is_some_and(|current| {
                    current.output_identity == before_keyboard.output_identity
                        && current.audio_revision > before_keyboard.audio_revision
                })
            {
                break;
            }
            drop(slot);
            assert!(Instant::now() < end, "actual keyboard graph did not apply");
            std::thread::sleep(Duration::from_millis(1));
        }
        for (i, sample) in output.iter().enumerate() {
            assert_eq!(*sample, expected_at(i), "keyboard {parameter:?} frame{i}");
        }
        visual.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("a").unwrap(),
        });
        visual.run_until_parked();
        render(h.backend.lock().unwrap().as_mut().unwrap(), &mut output);
        // Direct keyup tail before toggle cancellation or Stop: the release
        // (authored two frames once ReleaseMs is set, else zero) is raised to
        // the prepared floor, from the held level (gain .5, sustain .5 once
        // set) over the looping source from phase 0.
        let raw = [
            [0.125, -0.25],
            [0.3125, -0.125],
            [0.5, 0.0],
            [0.1875, 0.125],
            [-0.125, 0.25],
            [-0.0625, 0.125],
            [0.0, 0.0],
            [0.0625, -0.125],
        ];
        let held = if sustain { 0.25 } else { 0.5 };
        let authored = if release { 2 } else { 0 };
        for (i, sample) in output.iter().enumerate() {
            let level = held * super::floored_release_level(i, authored);
            let expected = [raw[i % 8][0] * level, raw[i % 8][1] * level];
            for c in 0..2 {
                assert!(
                    (sample[c] - expected[c]).abs() < 1e-6,
                    "keyup tail {parameter:?} {i}: {sample:?} != {expected:?}"
                );
            }
        }
        visual.simulate_keystrokes("m");
        for _ in 0..4 {
            pulse(visual);
        }
        visual.simulate_keystrokes("space");
        visual.run_until_parked();
        render(h.backend.lock().unwrap().as_mut().unwrap(), &mut output);
        for (i, sample) in output.iter().enumerate() {
            assert_eq!(
                *sample,
                expected_at(i),
                "arrangement {parameter:?} frame{i}"
            );
        }
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        render(h.backend.lock().unwrap().as_mut().unwrap(), &mut output);
        assert_eq!(output, [[0.0; 2]; 64]);
        for _ in 0..4 {
            pulse(visual);
        }
    }
}

pub(super) fn render(
    backend: &mut hex_audio::worker_fixture::HeadlessBackend,
    output: &mut [[f32; 2]],
) {
    let measured = allocation_counter::measure(|| backend.render(output).unwrap());
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

pub(super) fn trigger_release(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    track: hex_project::TrackId,
    saved: &std::path::Path,
) {
    let initial_submissions = h.observation.lock().unwrap().resident_submitted.len();
    let stored = hex_storage::load(saved).unwrap();
    let device = stored.track(track).unwrap().instrument().unwrap();
    assert_eq!(
        hex_engine::sampler_parameter_value_on_control(ReleaseMs, device.state()).unwrap(),
        0.125
    );
    assert!(!hex_engine::sampler_parameter_active_on_control(ReleaseMs, device.state()).unwrap());
    let target = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let center = visual.debug_bounds(target).unwrap().center();
    visual.simulate_click(center, Modifiers::default());
    visual.run_until_parked();
    let field =
        Box::leak(format!("sampler-release-{}-value", device.id().value()).into_boxed_str());
    let center = visual
        .debug_bounds(field)
        .expect("stored release remains rendered in trigger mode")
        .center();
    let before = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rendered_current(h)
        .unwrap();
    visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        gpui::point(center.x, center.y - gpui::px(10.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
    for _ in 0..12 {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        render(
            h.backend.lock().unwrap().as_mut().unwrap(),
            &mut [[0.0; 2]; 64],
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        h.backend
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .rendered_current(h),
        Some(before)
    );
    assert_eq!(
        h.observation.lock().unwrap().resident_submitted.len(),
        initial_submissions
    );
    visual.simulate_keystrokes("cmd-s");
    for _ in 0..12 {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        render(
            h.backend.lock().unwrap().as_mut().unwrap(),
            &mut [[0.0; 2]; 64],
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        hex_storage::load(saved)
            .unwrap()
            .device(device.id())
            .unwrap()
            .state(),
        device.state()
    );
}
