//! Actual retained controls -> Session -> sealed native CURRENT and persistence.
use super::*;
use gpui::{Modifiers, MouseButton, VisualTestContext};
use hex_engine::SamplerParameter::{
    EndFrame, Gain, LoopEnabled, LoopEndFrame, LoopStartFrame, StartFrame, Transpose,
};
pub(super) fn exercise(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    window: gpui::AnyWindowHandle,
    step: Duration,
    track: hex_project::TrackId,
    saved: &std::path::Path,
) {
    let stored = hex_storage::load(saved).unwrap();
    let original = stored.track(track).unwrap().instrument().unwrap();
    let device = original.id();
    let gain_selector =
        Box::leak(format!("sampler-gain-{}-value", device.value()).into_boxed_str());
    let pitch_selector =
        Box::leak(format!("sampler-pitch-{}-value", device.value()).into_boxed_str());
    let start_selector =
        Box::leak(format!("sampler-start-{}-value", device.value()).into_boxed_str());
    let end_selector = Box::leak(format!("sampler-end-{}-value", device.value()).into_boxed_str());
    let loop_selector =
        Box::leak(format!("sampler-loop-{}-value", device.value()).into_boxed_str());
    let loop_start_selector =
        Box::leak(format!("sampler-loop-start-{}-value", device.value()).into_boxed_str());
    let loop_end_selector =
        Box::leak(format!("sampler-loop-end-{}-value", device.value()).into_boxed_str());
    let target_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    // The fixture reopens through the actual menu; select its sampler track.
    let bounds = visual
        .debug_bounds(target_selector)
        .expect("actual sampler track header");
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let frame_selector =
        Box::leak(format!("instrument-device-{}", device.value()).into_boxed_str());
    let frame = visual.debug_bounds(frame_selector).unwrap();
    let grid = visual.debug_bounds("sampler-controls").unwrap();
    for selector in [
        gain_selector as &str,
        pitch_selector,
        start_selector,
        end_selector,
        loop_selector,
        loop_start_selector,
        loop_end_selector,
    ] {
        let field = visual.debug_bounds(selector).unwrap();
        assert!(
            field.left() >= grid.left()
                && field.right() <= grid.right()
                && field.top() >= grid.top()
                && field.bottom() <= grid.bottom(),
            "field must fit compact sampler grid: {selector}"
        );
        assert!(field.bottom() <= frame.bottom());
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
    let initial_submissions = h.observation.lock().unwrap().resident_submitted.len();
    let initial = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rendered_current(h)
        .unwrap();
    for (selector, draft) in [
        (gain_selector as &str, "2.5"),
        (gain_selector, "nan"),
        (pitch_selector, "nan"),
        (start_selector, "0.5"),
        (end_selector, "5"),
        (loop_selector, "maybe"),
        (loop_start_selector, "0.5"),
        (loop_end_selector, "5"),
    ] {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(draft);
        if draft != "2.5" {
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
                .rendered_current(h),
            Some(initial),
            "unconfirmed or invalid local numeric preview must not publish/edit"
        );
        assert_eq!(
            h.observation.lock().unwrap().resident_submitted.len(),
            initial_submissions
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
    }
    let mut gain = 1.0;
    let mut pitch = 0.0;
    let mut region_start = 0.0;
    let mut region_end = 4.0;
    let mut loop_enabled = false;
    let mut loop_start = 0.0;
    let mut loop_end = 4.0;
    for (index, (parameter, value)) in [
        (Gain, 0.5),
        (Transpose, 12.0),
        (Transpose, -12.0),
        (Transpose, 0.0),
        (Transpose, 0.5),
        (Transpose, -12.0),
        (StartFrame, 1.0),
        (EndFrame, 2.0),
        (StartFrame, 0.0),
        (EndFrame, 4.0),
        (LoopStartFrame, 1.0),
        (LoopEndFrame, 3.0),
        (LoopEnabled, 1.0),
        (LoopEnabled, 0.0),
        (LoopEnabled, 1.0),
        (LoopEnabled, 0.0),
        (StartFrame, 2.0),
        (LoopStartFrame, 2.0),
        (LoopEnabled, 1.0),
        (LoopEnabled, 0.0),
        (Gain, 0.0),
    ]
    .into_iter()
    .enumerate()
    {
        // Each edit/save/new/reopen phase gets its own stuck-progress deadline.
        // One whole-fixture budget made 21 sequential steps fail under host load.
        let mut end = Instant::now() + step;
        match parameter {
            Gain => gain = value,
            Transpose => pitch = value,
            StartFrame => region_start = value,
            EndFrame => region_end = value,
            LoopEnabled => loop_enabled = value == 1.0,
            LoopStartFrame => loop_start = value,
            LoopEndFrame => loop_end = value,
            _ => panic!("not a scalar fixture control"),
        }
        let selector = match parameter {
            Gain => gain_selector as &str,
            Transpose => pitch_selector,
            StartFrame => start_selector,
            EndFrame => end_selector,
            LoopEnabled => loop_selector,
            LoopStartFrame => loop_start_selector,
            LoopEndFrame => loop_end_selector,
            _ => panic!("not a scalar fixture control"),
        };
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
        let bounds = visual
            .debug_bounds(selector)
            .expect("actual sampler numeric gain field");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        if parameter == StartFrame && value == 1.0 {
            // Whole-frame scrub must accumulate small moves on the actual field.
            for offset in 1..=24 {
                visual.simulate_mouse_move(
                    gpui::point(
                        bounds.center().x,
                        bounds.center().y - gpui::px(offset as f32),
                    ),
                    Some(MouseButton::Left),
                    Modifiers::default(),
                );
            }
            visual.simulate_mouse_up(
                gpui::point(bounds.center().x, bounds.center().y - gpui::px(24.0)),
                MouseButton::Left,
                Modifiers::default(),
            );
        } else {
            visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_keystrokes("cmd-a");
            let text = if parameter == LoopEnabled {
                if value == 1.0 {
                    "on".into()
                } else {
                    "off".into()
                }
            } else {
                value.to_string()
            };
            visual.simulate_input(&text);
            visual.simulate_keystrokes("enter");
        }
        loop {
            pulse(visual);
            if h.backend
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|b| b.status())
                .and_then(|s| s.rendered_current(h))
                .is_some_and(|s| {
                    s.context.project_generation == old.context.project_generation
                        && s.context.editor_revision == old.context.editor_revision + 1
                        && s.output_identity == old.output_identity
                        && s.audio_revision > old.audio_revision
                })
            {
                break;
            }
            assert!(
                Instant::now() < end,
                "{parameter:?} {value} did not reach fresh CURRENT: {:?}",
                cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            );
        }
        for _ in 0..12 {
            pulse(visual);
        }
        end = Instant::now() + step;
        visual.simulate_keystrokes("cmd-s");
        loop {
            pulse(visual);
            let project = hex_storage::load(saved).unwrap();
            let state = project.device(device).unwrap().state();
            let ready = cx
                .update(|cx| hex_ui::sampler_midi_test_ready(window, cx))
                .unwrap();
            if hex_engine::sampler_parameter_value_on_control(parameter, state).unwrap() == value
                && ready
            {
                assert_eq!(
                    state.asset_references(),
                    original.state().asset_references()
                );
                assert_eq!(state.input_gain_db(), original.state().input_gain_db());
                assert_eq!(state.output_gain_db(), original.state().output_gain_db());
                assert_eq!(
                    hex_engine::sampler_parameter_value_on_control(LoopEnabled, state).unwrap(),
                    if loop_enabled { 1.0 } else { 0.0 }
                );
                assert_eq!(
                    hex_engine::sampler_parameter_value_on_control(LoopStartFrame, state).unwrap(),
                    loop_start
                );
                assert_eq!(
                    hex_engine::sampler_parameter_value_on_control(LoopEndFrame, state).unwrap(),
                    loop_end
                );
                let disabled =
                    hex_engine::patch_sampler_parameter_on_control(LoopEnabled, state, 0.0)
                        .unwrap();
                let reset_loop_end =
                    hex_engine::patch_sampler_parameter_on_control(LoopEndFrame, &disabled, 4.0)
                        .unwrap();
                let reset_loop = hex_engine::patch_sampler_parameter_on_control(
                    LoopStartFrame,
                    &reset_loop_end,
                    0.0,
                )
                .unwrap();
                let restored_gain =
                    hex_engine::patch_sampler_parameter_on_control(Gain, &reset_loop, 1.0).unwrap();
                let restored =
                    hex_engine::patch_sampler_parameter_on_control(Transpose, &restored_gain, 0.0)
                        .unwrap();
                let restored =
                    hex_engine::patch_sampler_parameter_on_control(EndFrame, &restored, 4.0)
                        .unwrap();
                let restored =
                    hex_engine::patch_sampler_parameter_on_control(StartFrame, &restored, 0.0)
                        .unwrap();
                assert_eq!(
                    restored,
                    *original.state(),
                    "all other authored and host/source state must survive"
                );
                assert_eq!(project.tuning(), tuning(880.0));
                break;
            }
            assert!(
                Instant::now() < end,
                "actual SaveCurrent did not persist gain: {:?}",
                cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            );
        }
        if index <= 1
            || (parameter == Transpose && value == 0.5)
            || parameter == EndFrame
            || parameter == LoopEnabled
        {
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
            end = Instant::now() + step;
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
            end = Instant::now() + step;
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
                    .and_then(|s| s.rendered_current(h))
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
        let invalid = if parameter == StartFrame && value == 2.0 {
            Some((loop_selector as &str, "on")) // Dormant1..3 is outside trimmed2..4.
        } else if parameter == LoopEnabled && loop_enabled && loop_start == 2.0 {
            Some((loop_start_selector as &str, "1")) // Active range cannot leave current region.
        } else {
            None
        };
        if let Some((selector, text)) = invalid {
            let before = h
                .backend
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .rendered_current(h);
            let submissions = h.observation.lock().unwrap().resident_submitted.len();
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_keystrokes("cmd-a");
            visual.simulate_input(text);
            visual.simulate_keystrokes("enter");
            loop {
                pulse(visual);
                let status = cx
                    .update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
                    .unwrap();
                if status.contains("edit_pending=false") && status.contains("error=Some") {
                    assert!(
                        status.contains("loop") && status.contains("current=true"),
                        "{status}"
                    );
                    break;
                }
                assert!(
                    Instant::now() < end,
                    "invalid loop edit did not finish: {status}"
                );
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
                before
            );
            assert_eq!(
                h.observation.lock().unwrap().resident_submitted.len(),
                submissions
            );
        }
        end = Instant::now() + step;
        // Capture first key render without pumping away this short source.
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
        // Independent literal source/interpolation oracles. A880 contributes
        // exactly x2 and 8k->16k contributes x0.5; authored pitch remains once.
        let expected: &[[f32; 2]] = if region_start == 2.0 {
            &[[-0.125, 0.25], [-0.0625, 0.125], [0.0, 0.0]]
        } else if region_start == 1.0 && region_end == 2.0 {
            &[[0.5, 0.0], [0.5, 0.0]]
        } else if region_start == 0.0 && region_end == 2.0 {
            &[[0.125, -0.25], [0.3125, -0.125], [0.5, 0.0], [0.5, 0.0]]
        } else if region_start == 1.0 {
            &[
                [0.5, 0.0],
                [0.1875, 0.125],
                [-0.125, 0.25],
                [-0.0625, 0.125],
                [0.0, 0.0],
            ]
        } else if pitch == 12.0 {
            &[[0.125, -0.25], [-0.125, 0.25]]
        } else if pitch == -12.0 {
            &[
                [0.125, -0.25],
                [0.3125, -0.125],
                [0.5, 0.0],
                [0.1875, 0.125],
                [-0.125, 0.25],
                [-0.0625, 0.125],
                [0.0, 0.0],
            ]
        } else if pitch == 0.5 {
            &[
                [0.125, -0.25],
                [0.4816861, 0.007325559],
                [-0.11767444, 0.23534888],
                [0.0, 0.0],
            ]
        } else {
            &[[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]]
        };
        // Literal loop seam oracles. Off preserves endpoints but has no wrap.
        let expected_at = |i: usize| {
            if loop_enabled && loop_start == 2.0 {
                [-0.125, 0.25] // One-frame2..3 loop holds this exact source frame.
            } else if loop_enabled && i >= 2 {
                [[0.5, 0.0], [0.1875, 0.125], [-0.125, 0.25], [0.1875, 0.125]][(i - 2) % 4]
            } else {
                expected.get(i).copied().unwrap_or([0.0; 2])
            }
        };
        // Pitched-up voices are band-limited (#4089); their reconstruction is
        // owned and tested in hex-devices. Here they keep the literal duration
        // (no frames past the region), stay finite and bounded by the gained
        // source peak with kernel overshoot, and sound.
        let pitched_up = pitch > 0.0;
        let check = |frames: &[[f32; 2]], label: &str| {
            if pitched_up {
                assert!(
                    frames[..expected.len()].iter().flatten().any(|a| *a != 0.0),
                    "{label} gain {gain} pitch {pitch}: {frames:?}"
                );
            }
            for (i, frame) in frames.iter().enumerate() {
                for (a, e) in frame.iter().zip(expected_at(i)) {
                    let in_note = i < expected.len();
                    let ok = if pitched_up && in_note {
                        a.is_finite() && a.abs() <= gain as f32
                    } else {
                        (*a - e * gain as f32).abs() < 1e-6
                    };
                    assert!(ok, "{label} gain {gain} pitch {pitch} frame{i}: {frame:?}");
                }
            }
        };
        check(&output, "keyboard");
        visual.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("a").unwrap(),
        });
        visual.run_until_parked();
        if loop_enabled {
            let mut released = [[0.0; 2]; 64];
            let mut slot = h.backend.lock().unwrap();
            let measured = allocation_counter::measure(|| {
                slot.as_mut().unwrap().render(&mut released).unwrap()
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
            // The looping voice declicks over the prepared release floor and
            // then stops, without keyboard-toggle cancellation.
            for (i, frame) in released.iter().enumerate() {
                let bound = 0.5 * gain as f32 * super::floored_release_level(i, 0);
                assert!(
                    frame.iter().all(|sample| sample.abs() <= bound + 1e-6),
                    "loop key release {i} exceeds its declick: {frame:?}"
                );
            }
            assert_eq!(
                &released[super::RELEASE_FLOOR_FRAMES..],
                &[[0.0; 2]; 64 - super::RELEASE_FLOOR_FRAMES],
                "actual loop key release must stop without keyboard-toggle cancellation"
            );
        }
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
        check(&arranged, "arrangement");
        drop(slot);
        visual.simulate_keystrokes("escape");
        for _ in 0..4 {
            pulse(visual);
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
