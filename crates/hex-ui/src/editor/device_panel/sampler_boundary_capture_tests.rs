//! Actual handle input; admitted sampler receipts retain their original owner.
use super::*;
use gpui::{CursorStyle, Render, ScrollDelta, ScrollWheelEvent};
use std::{cell::Cell, rc::Rc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
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
                .cursor(CursorStyle::Crosshair)
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
    start: gpui::Point<Pixels>,
    bounds: Bounds<Pixels>,
}
fn open(cx: &mut TestAppContext, end: bool) -> Fixture {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, _, _) = fixture(cx, 1024, true);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1600.), px(700.)));
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    visual.run_until_parked();
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    let start = point(
        if end {
            bounds.right() - px(2.)
        } else {
            bounds.left() + px(1.)
        },
        bounds.top() + px(12.),
    );
    Fixture {
        visual,
        editor,
        actions,
        start,
        bounds,
    }
}
#[gpui::test]
fn sampler_handles_capture_input_and_release_without_holding_pending_edit(cx: &mut TestAppContext) {
    for end in [false, true] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            actions,
            start,
            bounds,
        } = open(cx, end);
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        assert!(editor.read_with(&visual, |v, _| {
            v.device_panel.sampler_cursor.boundary.is_some()
        }));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "sampler handles must isolate input before motion"
        );
        let moved = start + point(bounds.size.width * if end { -0.25 } else { 0.25 }, px(0.));
        visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::ResizeColumn)
        );
        let release = point(moved.x, px(10.));
        visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(actions.get(), 0);
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        editor.read_with(&visual, |v, _| {
            assert!(v.device_panel.sampler_cursor.boundary.is_none());
            assert!(v.sampler_parameter_pending());
        });
        visual.simulate_mouse_move(outside, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(actions.get(), 1, "pending edit must not own pointer input");
    }
}
#[gpui::test]
fn sampler_handles_capture_loss_never_submits_late_release(cx: &mut TestAppContext) {
    for end in [false, true] {
        for loss in [
            "button",
            "focus",
            "invalid",
            "escape",
            "generation",
            "hidden",
        ] {
            let cx = &mut cx.new_app();
            let Fixture {
                mut visual,
                editor,
                actions,
                start,
                bounds,
            } = open(cx, end);
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let outside = point(px(640.), px(10.));
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(
                start + point(bounds.size.width * if end { -0.25 } else { 0.25 }, px(0.)),
                MouseButton::Left,
                Modifiers::default(),
            );
            visual.run_until_parked();
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "invalid" => visual.simulate_mouse_move(
                    point(px(f32::NAN), px(f32::NAN)),
                    MouseButton::Left,
                    Modifiers::default(),
                ),
                "escape" => visual.simulate_keystrokes("escape"),
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
                "hidden" => editor.update(&mut visual, |v, cx| {
                    v.clip_view_open = false;
                    cx.notify();
                }),
                "focus" => visual.update(|w, cx| {
                    editor.update(cx, |v, cx| v.search_browser(w, cx));
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            position: outside,
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
            if loss != "focus" {
                visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
            }
            visual.run_until_parked();
            assert_eq!(actions.get(), 0, "end={end} loss={loss}");
            editor.read_with(&visual, |v, _| {
                assert!(
                    v.device_panel.sampler_cursor.boundary.is_none(),
                    "end={end} loss={loss}"
                );
                assert!(!v.sampler_parameter_pending(), "end={end} loss={loss}");
                assert!(v.pending_requests.is_empty());
                assert_eq!(v.worker_requests_pending, 0);
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}

/// Original-frame preview only; setup/cancellation and worker admission excluded.
#[gpui::test]
#[ignore = "manual optimized sampler handle profile; no native acceptance"]
fn profile_sampler_capture_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_SAMPLER_CAPTURE_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for end in [false, true] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            start,
            ..
        } = open(cx, end);
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(editor.read_with(&visual, |v, _| {
                v.device_panel.sampler_cursor.boundary.is_some()
            }));
            let clock = std::time::Instant::now();
            let mut at = start;
            for step in 0..8 {
                let displacement = px(((round + step) % 7) as f32 * 2.);
                at = start + point(if end { -displacement } else { displacement }, px(0.));
                visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
            }
            let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(v.device_panel.sampler_cursor.boundary.is_none());
                assert!(!v.sampler_parameter_pending());
                assert_eq!(v.worker_requests_pending, 0);
                assert!(v.pending_requests.is_empty());
            });
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = visual.update(|w, _| w.scale_factor());
        println!(
            "SAMPLER_CAPTURE_PROFILE end={end} size=1600x700 scale={scale} frames=1024 rate=44100 warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
