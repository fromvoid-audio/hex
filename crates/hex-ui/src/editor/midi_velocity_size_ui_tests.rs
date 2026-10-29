//! View-only lane sizing through the actual handle and completed layout.
use super::*;
fn resize_point(visual: &mut VisualTestContext) -> gpui::Point<gpui::Pixels> {
    visual
        .debug_bounds("midi-velocity-resize")
        .unwrap()
        .center()
}
#[gpui::test]
fn lane_size_owns_input_uses_final_release_and_never_edits_project(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = setup(cx);
    let actions = host(&mut visual, &editor, false);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for (delta, expected) in [(-20., 84.), (-1000., 128.), (1000., 40.)] {
        editor.update(&mut visual, |v, cx| {
            v.midi_velocity.height = 64.;
            cx.notify();
        });
        repaint(&editor, &mut visual);
        let down = resize_point(&mut visual);
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(actions.get(), 0, "sizing owns input before motion");
        visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::ResizeUpDown)
        );
        // Commit uses this release even when it differs from the last motion.
        visual.simulate_mouse_up(
            down + point(px(0.), px(delta)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        repaint(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_velocity.height, expected);
            assert!(v.midi_velocity.resize.is_none());
            assert!(!v.edit_pending());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        assert_eq!(actions.get(), 0);
        assert_eq!(
            f32::from(
                visual
                    .debug_bounds("midi-velocity-lane")
                    .unwrap()
                    .size
                    .height
            ),
            expected
        );
    }
    let at = handle(&mut visual, 0.5, 37);
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        at - point(px(0.), px(6.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}
#[gpui::test]
fn lane_size_loss_keeps_last_height_without_leaking_release(cx: &mut TestAppContext) {
    for loss in [
        "button",
        "escape",
        "focus",
        "invalid",
        "source",
        "generation",
        "hidden",
        "window",
    ] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, _clip) = setup(cx);
        let actions = host(&mut visual, &editor, false);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let down = resize_point(&mut visual);
        let end = down - point(px(0.), px(20.));
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        assert_eq!(
            editor.read_with(&visual, |v, _| v.midi_velocity.height),
            84.
        );
        match loss {
            "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
            "escape" => visual.simulate_keystrokes("escape"),
            "focus" | "invalid" => visual.update(|w, cx| {
                if loss == "focus" {
                    editor.update(cx, |v, _| v.focus.focus(w));
                } else {
                    w.dispatch_input(
                        gpui::PlatformInput::MouseMove(MouseMoveEvent {
                            position: point(px(f32::NAN), end.y),
                            pressed_button: Some(MouseButton::Left),
                            ..Default::default()
                        }),
                        cx,
                    );
                }
                w.dispatch_input(
                    gpui::PlatformInput::MouseUp(MouseUpEvent {
                        position: outside,
                        button: MouseButton::Left,
                        ..Default::default()
                    }),
                    cx,
                );
            }),
            "source" => editor.update(&mut visual, |v, cx| {
                v.selected_clip = None;
                cx.notify();
            }),
            "generation" => editor.update(&mut visual, |v, cx| {
                v.project_generation += 1;
                cx.notify();
            }),
            "hidden" => editor.update(&mut visual, |v, cx| {
                v.midi_velocity.open = false;
                cx.notify();
            }),
            "window" => visual.deactivate_window(),
            _ => unreachable!(),
        }
        repaint(&editor, &mut visual);
        if loss != "focus" && loss != "invalid" {
            visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        }
        assert_eq!(actions.get(), 0, "{loss}: trailing release");
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_velocity.resize.is_none(), "{loss}");
            assert_eq!(v.midi_velocity.height, 84., "{loss}: keep last height");
            assert!(!v.edit_pending());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}

#[gpui::test]
#[ignore = "opt-in velocity lane sizing dispatch and repaint CPU probe"]
fn lane_sizing_dispatch_repaint_latency(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = setup(cx);
    let _actions = host(&mut visual, &editor, false);
    let down = resize_point(&mut visual);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    let mut samples = Vec::new();
    for n in 0..144 {
        let start = Instant::now();
        visual.simulate_mouse_move(
            down - point(px(0.), px(16. + (n % 8) as f32)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        repaint(&editor, &mut visual);
        if n >= 16 {
            samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
        }
    }
    visual.simulate_mouse_up(
        down - point(px(0.), px(16.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert_eq!(handles(&mut visual).len(), 3);
    samples.sort_by(f64::total_cmp);
    println!(
        "velocity_size_repaint_us p50={} p95={} max={} handles=3 tracks=1 clips=1 notes=3 warmups=16 samples=128",
        samples[64], samples[121], samples[127]
    );
}
