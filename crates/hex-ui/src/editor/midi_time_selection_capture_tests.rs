//! Actual source-local selection input on the three shared surfaces.
use super::*;
use crate::editor::{midi_canvas::creation_tests::fixture, test_environment::Environment};
use gpui::{CursorStyle, Entity, Modifiers, Render, TestAppContext, VisualTestContext};
use std::{cell::Cell, rc::Rc, sync::Arc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut gpui::Window, _: &mut ViewContext<Self>) -> impl IntoElement {
        let editor = self.editor.clone();
        let down = self.actions.clone();
        let up = self.actions.clone();
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
                .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
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
fn open(cx: &mut TestAppContext, selector: &'static str) -> Fixture {
    let (mut visual, editor, environment, _) = fixture(cx);
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    visual.simulate_resize(size(px(1280.), px(800.)));
    visual.run_until_parked();
    let b = visual.debug_bounds(selector).unwrap();
    let at = |f| {
        point(
            b.origin.x + px(1.) + (b.size.width - px(2.)) * f,
            b.top() + px(2.),
        )
    };
    Fixture {
        visual,
        editor,
        actions,
        _environment: environment,
        from: at(0.25),
        to: at(0.5),
    }
}
const SURFACES: [&str; 3] = ["midi-source-ruler", "midi-note-roll", "midi-velocity-lane"];
#[gpui::test]
fn time_selection_owns_three_surfaces_and_releases_input_after_range(cx: &mut TestAppContext) {
    for surface in SURFACES {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            actions,
            from,
            to,
            ..
        } = open(cx, surface);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        assert!(
            editor.read_with(&visual, |v, _| v.midi_time_selection.gesture.is_some()),
            "{surface}"
        );
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: gpui::ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "{surface}: input is exclusive before movement"
        );
        visual.simulate_mouse_move(
            point(to.x, outside.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::Crosshair),
            "{surface}"
        );
        visual.simulate_mouse_up(
            point(to.x, outside.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
        }
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_time_selection.gesture.is_none());
            assert_eq!(
                v.midi_time_selection_bounds(),
                Some((
                    hex_project::BeatTime::ONE.raw(),
                    2 * hex_project::BeatTime::ONE.raw()
                )),
                "{surface}"
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(!v.edit_pending());
        });
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        assert_eq!(actions.get(), 0, "{surface}: no trailing release");
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(actions.get(), 1);
    }
}
#[gpui::test]
fn time_selection_loss_preserves_prior_range_without_stale_release(cx: &mut TestAppContext) {
    for surface in SURFACES {
        for loss in [
            "button",
            "focus",
            "invalid",
            "escape",
            "generation",
            "viewport",
            "hidden",
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
            } = open(cx, surface);
            visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let prior = editor.read_with(&visual, |v, _| {
                (
                    v.midi_time_selection_bounds(),
                    v.midi_marker,
                    v.state.as_ref().unwrap().project.snapshot(),
                )
            });
            visual.simulate_mouse_down(to, MouseButton::Left, Modifiers::default());
            let outside = point(px(640.), px(10.));
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "escape" => visual.simulate_keystrokes("escape"),
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
                "viewport" => editor.update(&mut visual, |v, cx| {
                    v.piano_beat_start = i128::from(hex_project::BeatTime::ONE.raw());
                    cx.notify();
                }),
                "hidden" => editor.update(&mut visual, |v, cx| {
                    v.clip_view_open = false;
                    cx.notify();
                }),
                "window" => visual.deactivate_window(),
                "focus" | "invalid" => visual.update(|w, cx| {
                    if loss == "focus" {
                        editor.update(cx, |v, cx| v.search_browser(w, cx));
                    } else {
                        w.dispatch_input(
                            gpui::PlatformInput::MouseMove(MouseMoveEvent {
                                position: point(to.x, px(f32::NAN)),
                                pressed_button: Some(MouseButton::Left),
                                modifiers: Modifiers::default(),
                            }),
                            cx,
                        );
                    }
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(MouseUpEvent {
                            position: to,
                            button: MouseButton::Left,
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
                visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
            }
            visual.run_until_parked();
            assert_eq!(actions.get(), 0, "{surface}/{loss}: trailing release");
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_time_selection.gesture.is_none(), "{surface}/{loss}");
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), prior.2);
                assert!(!v.edit_pending());
                if loss != "generation" {
                    assert_eq!(v.midi_time_selection_bounds(), prior.0, "{surface}/{loss}");
                }
                if loss == "generation" {
                    assert_eq!(
                        v.midi_marker,
                        Some(0),
                        "new project resets insertion cursor"
                    );
                } else {
                    assert_eq!(v.midi_marker, prior.1, "{surface}/{loss}");
                }
            });
        }
    }
}

#[gpui::test]
#[ignore = "opt-in three-surface selection dispatch and repaint probe"]
fn time_selection_dispatch_repaint_latency(cx: &mut TestAppContext) {
    for surface in SURFACES {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            from,
            to,
            ..
        } = open(cx, surface);
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        let mut samples = Vec::new();
        for n in 0..144 {
            let at = point(from.x + (to.x - from.x) * (1. + (n % 8) as f32 / 8.), to.y);
            let start = std::time::Instant::now();
            visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            if n >= 16 {
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        println!(
            "midi_time_dispatch_us surface={surface} p50={} p95={} max={} window=1280x800 warmups=16 samples=128 tracks=1 clips=1 notes=0",
            samples[64], samples[121], samples[127]
        );
    }
}
