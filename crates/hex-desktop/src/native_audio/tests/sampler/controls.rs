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
    end: Instant,
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
    let initial = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_audio_source();
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
                .applied_audio_source(),
            initial,
            "unconfirmed or invalid local numeric preview must not publish/edit"
        );
        assert!(h.observation.lock().unwrap().gain_cells.is_empty());
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
            .applied_audio_source()
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
                "{parameter:?} {value} did not reach fresh CURRENT: {:?}",
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
            if hex_engine::sampler_parameter_value_on_control(parameter, state).unwrap() == value {
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
                .applied_audio_source();
            let cells = h.observation.lock().unwrap().gain_cells.len();
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
                    .applied_audio_source(),
                before
            );
            assert_eq!(h.observation.lock().unwrap().gain_cells.len(), cells);
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
        for (i, frame) in output.iter().enumerate() {
            for (a, e) in frame.iter().zip(expected_at(i)) {
                assert!(
                    (*a - e * gain as f32).abs() < 1e-6,
                    "gain {gain} pitch {pitch} frame{i}: {frame:?}"
                );
            }
        }
        drop(slot);
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
            assert_eq!(
                released, [[0.0; 2]; 64],
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
        for (i, frame) in arranged.iter().enumerate() {
            for (a, e) in frame.iter().zip(expected_at(i)) {
                assert!(
                    (*a - e * gain as f32).abs() < 1e-6,
                    "arrangement gain {gain} pitch {pitch} frame{i}: {frame:?}"
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
            assert_eq!(observation.gain_cells.len(), index + 1);
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
