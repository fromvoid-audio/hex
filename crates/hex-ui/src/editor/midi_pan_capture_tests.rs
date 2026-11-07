//! Both piano-pan buttons retain navigation while owning only held input.
use super::*;
use crate::editor::{midi_canvas::creation_tests::fixture, test_environment::Environment};
use gpui::{
    CursorStyle, Entity, Modifiers, MouseMoveEvent, MouseUpEvent, Render, TestAppContext,
    VisualTestContext, div, point, prelude::*, px, size,
};
use hex_project::BeatTime;
use std::{cell::Cell, rc::Rc, sync::Arc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let editor = self.editor.clone();
        let down = self.actions.clone();
        let left = self.actions.clone();
        let middle = self.actions.clone();
        let wheel = self.actions.clone();
        div().size_full().child(editor).child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w_full()
                .h(px(24.))
                .cursor(CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                .on_mouse_up(MouseButton::Left, move |_, _, _| left.set(left.get() + 1))
                .on_mouse_up(MouseButton::Middle, move |_, _, _| {
                    middle.set(middle.get() + 1)
                })
                .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
        )
    }
}
struct Fixture {
    visual: VisualTestContext,
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
    _environment: Arc<Environment>,
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
}
fn open(cx: &mut TestAppContext, button: MouseButton) -> Fixture {
    let (mut visual, editor, environment, _) = fixture(cx);
    editor.update(&mut visual, |v, cx| {
        v.midi_pan.enabled = button == MouseButton::Left;
        cx.notify();
    });
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    visual.simulate_resize(size(px(1280.), px(800.)));
    visual.run_until_parked();
    let b = visual.debug_bounds("midi-note-roll").unwrap();
    let from = b.center();
    let to = from - point(b.size.width / 8., b.size.height / 8.);
    Fixture {
        visual,
        editor,
        actions,
        _environment: environment,
        from,
        to,
    }
}
fn viewport(editor: &Entity<EditorView>, visual: &VisualTestContext) -> (i128, BeatTime, u8, u8) {
    editor.read_with(visual, |v, _| {
        (
            v.piano_beat_start,
            v.piano_beat_span,
            v.piano_pitch_low,
            v.piano_pitch_rows,
        )
    })
}
#[gpui::test]
fn piano_pan_can_begin_and_repaint_far_beyond_source_without_edit(cx: &mut TestAppContext) {
    let Fixture {
        mut visual,
        editor,
        from,
        to,
        ..
    } = open(cx, MouseButton::Middle);
    let start = BeatTime::from_quarters(1024).unwrap();
    let before = editor.update(&mut visual, |v, cx| {
        v.piano_beat_start = i128::from(start.raw());
        cx.notify();
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(from, MouseButton::Middle, Modifiers::default());
    assert!(editor.read_with(&visual, |v, _| v.midi_pan.gesture.is_some()));
    visual.simulate_mouse_move(to, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Middle, Modifiers::default());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert!(viewport(&editor, &visual).0 > i128::from(start.raw()));
    }
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_pan.gesture.is_none());
        assert!(!v.edit_pending());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}
#[gpui::test]
fn piano_pan_owns_both_buttons_and_finishes_outside_without_edit(cx: &mut TestAppContext) {
    for button in [MouseButton::Middle, MouseButton::Left] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            actions,
            from,
            to,
            ..
        } = open(cx, button);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_down(from, button, Modifiers::default());
        assert!(editor.read_with(&visual, |v, _| v.midi_pan.gesture.is_some()));
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: outside,
            delta: gpui::ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "{button:?}: pan input is exclusive before motion"
        );
        visual.simulate_mouse_move(to, button, Modifiers::default());
        visual.run_until_parked();
        let moved = viewport(&editor, &visual);
        assert!(moved.0 > 0);
        let final_at = point(to.x - px(40.), outside.y);
        visual.simulate_mouse_move(final_at, button, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::ClosedHand)
        );
        let final_view = viewport(&editor, &visual);
        visual.simulate_mouse_up(final_at, button, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
        }
        assert_eq!(actions.get(), 0);
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        assert_eq!(viewport(&editor, &visual), final_view);
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_pan.gesture.is_none());
            assert!(!v.edit_pending());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(actions.get(), 1);
    }
}
#[gpui::test]
fn piano_pan_loss_keeps_last_navigation_and_blocks_late_release(cx: &mut TestAppContext) {
    for button in [MouseButton::Middle, MouseButton::Left] {
        for loss in [
            "button",
            "focus",
            "invalid",
            "escape",
            "generation",
            "hidden",
            "bounds",
            "window",
        ] {
            let cx = &mut cx.new_app();
            let Fixture {
                mut visual,
                editor,
                actions,
                from,
                to,
                ..
            } = open(cx, button);
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            visual.simulate_mouse_down(from, button, Modifiers::default());
            visual.simulate_mouse_move(to, button, Modifiers::default());
            visual.run_until_parked();
            let held = viewport(&editor, &visual);
            let outside = point(px(640.), px(10.));
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "escape" => visual.simulate_keystrokes("escape"),
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
                "hidden" => editor.update(&mut visual, |v, cx| {
                    v.clip_view_open = false;
                    cx.notify();
                }),
                "bounds" => visual.simulate_resize(size(px(1000.), px(700.))),
                "window" => visual.deactivate_window(),
                "focus" | "invalid" => visual.update(|w, cx| {
                    if loss == "focus" {
                        editor.update(cx, |v, cx| v.search_browser(w, cx));
                    } else {
                        w.dispatch_input(
                            gpui::PlatformInput::MouseMove(MouseMoveEvent {
                                position: point(to.x, px(f32::NAN)),
                                pressed_button: Some(button),
                                modifiers: Modifiers::default(),
                            }),
                            cx,
                        );
                    }
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(MouseUpEvent {
                            position: from,
                            button,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        }),
                        cx,
                    );
                }),
                _ => unreachable!(),
            }
            visual.run_until_parked();
            if !matches!(loss, "focus" | "invalid") {
                visual.simulate_mouse_up(outside, button, Modifiers::default());
            }
            visual.run_until_parked();
            assert_eq!(actions.get(), 0, "{button:?}/{loss}: trailing release");
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_pan.gesture.is_none(), "{button:?}/{loss}");
                assert!(!v.edit_pending());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
            if loss != "generation" {
                assert_eq!(
                    viewport(&editor, &visual),
                    held,
                    "{button:?}/{loss}: cancelled pan retains navigation"
                );
            }
        }
    }
}
#[gpui::test]
#[ignore = "opt-in both-button piano pan dispatch/repaint CPU probe"]
fn piano_pan_dispatch_repaint_latency(cx: &mut TestAppContext) {
    for button in [MouseButton::Middle, MouseButton::Left] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            from,
            to,
            ..
        } = open(cx, button);
        visual.simulate_mouse_down(from, button, Modifiers::default());
        let mut samples = Vec::new();
        for n in 0..144 {
            let at = from + (to - from) * (1. + (n % 8) as f32 / 8.);
            let start = std::time::Instant::now();
            visual.simulate_mouse_move(at, button, Modifiers::default());
            visual.run_until_parked();
            if n >= 16 {
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        visual.simulate_mouse_up(to, button, Modifiers::default());
        println!(
            "piano_pan_dispatch_us button={button:?} p50={} p95={} max={} window=1280x800 warmups=16 samples=128 tracks=1 clips=1 notes=0",
            samples[64], samples[121], samples[127]
        );
    }
}
