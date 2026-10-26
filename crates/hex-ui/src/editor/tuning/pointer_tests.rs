//! Actual pitch-line input, independent from worker/canonical terminal custody.
use super::*;
use gpui::{AppContext, CursorStyle, Render, TestAppContext, VisualTestContext, point};
use std::{cell::Cell, rc::Rc, sync::Arc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let down = self.actions.clone();
        let up = self.actions.clone();
        let wheel = self.actions.clone();
        let editor = self.editor.clone();
        div().size_full().child(editor).child(
            div()
                .absolute()
                .left(px(100.))
                .top_0()
                .w(px(80.))
                .h(px(20.))
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
    start: gpui::Point<gpui::Pixels>,
}
fn open(cx: &mut TestAppContext) -> Fixture {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let actions = Rc::new(Cell::new(0));
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
        editor.update(cx, |v, cx| {
            v.tuning.open = true;
            v.tuning.focus.focus(window);
            v.sync_tuning(cx);
            cx.notify();
        });
    })
    .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.), px(800.)));
    visual.run_until_parked();
    let bounds = editor.read_with(&visual, |v, _| v.tuning.pitch_bounds.unwrap());
    // Degree three is away from both clamped endpoints and the reference degree.
    let start = point(bounds.left() + bounds.size.width * 0.25, bounds.center().y);
    Fixture {
        visual,
        editor,
        actions,
        start,
    }
}
#[gpui::test]
fn pitch_pointer_isolates_input_and_releases_before_canonical_ack(cx: &mut TestAppContext) {
    let Fixture {
        mut visual,
        editor,
        actions,
        start,
    } = open(cx);
    let outside = point(px(140.), px(10.));
    visual.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
    assert!(editor.read_with(&visual, |v, _| v.tuning.drag.is_some()));
    visual.simulate_mouse_down(outside, MouseButton::Right, gpui::Modifiers::default());
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: outside,
        delta: gpui::ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });
    assert_eq!(
        actions.get(),
        0,
        "held pitch input must isolate other targets before motion"
    );
    visual.simulate_mouse_move(outside, MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::ResizeLeftRight)
    );
    visual.simulate_mouse_up(outside, MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Right, gpui::Modifiers::default());
    visual.run_until_parked();
    assert_eq!(actions.get(), 0);
    assert!(!visual.update(|w, _| w.is_pointer_dragging()));
    assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
    let terminal = editor.read_with(&visual, |v, _| {
        assert!(v.tuning.drag.is_none());
        assert!(v.tuning.client.has_pending_terminal());
        v.tuning.client.snapshot().unwrap()
    });
    // Pending canonical custody must neither retain the old pointer nor accept
    // a new pitch edit that the already-finishing client cannot consume.
    visual.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
    assert!(!visual.update(|w, _| w.is_pointer_dragging()));
    editor.read_with(&visual, |v, _| {
        assert!(v.tuning.drag.is_none());
        assert_eq!(v.tuning.client.snapshot(), Some(terminal));
    });
    visual.simulate_mouse_up(start, MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_down(outside, MouseButton::Right, gpui::Modifiers::default());
    assert_eq!(
        actions.get(),
        1,
        "unrelated input resumes while the worker terminal is pending"
    );
}
#[gpui::test]
fn pitch_capture_loss_cancels_without_late_commit_or_new_source_edit(cx: &mut TestAppContext) {
    for loss in [
        "button",
        "focus",
        "focus-before-paint",
        "generation",
        "close",
        "escape",
    ] {
        let Fixture {
            mut visual,
            editor,
            actions,
            start,
        } = open(cx);
        let moved = start + point(px(12.), px(0.));
        let outside = point(px(140.), px(10.));
        visual.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        visual.simulate_mouse_move(moved, MouseButton::Left, gpui::Modifiers::default());
        let snapshot = editor.read_with(&visual, |v, _| v.tuning.client.snapshot().unwrap());
        match loss {
            "button" => visual.simulate_mouse_move(moved, None, gpui::Modifiers::default()),
            "focus" => visual.update(|w, cx| editor.update(cx, |v, _| v.focus.focus(w))),
            "focus-before-paint" => visual.update(|w, cx| {
                // Keep both operations in one update: simulator helpers may
                // otherwise complete a repaint between focus and mouse-up.
                editor.update(cx, |v, _| v.focus.focus(w));
                w.dispatch_input(
                    gpui::PlatformInput::MouseUp(MouseUpEvent {
                        position: outside,
                        button: MouseButton::Left,
                        modifiers: gpui::Modifiers::default(),
                        click_count: 1,
                    }),
                    cx,
                );
            }),
            "generation" => editor.update(&mut visual, |v, cx| {
                v.project_generation += 1;
                cx.notify();
            }),
            "close" => visual.update(|w, cx| editor.update(cx, |v, cx| v.close_tuning(w, cx))),
            "escape" => visual.simulate_keystrokes("escape"),
            _ => unreachable!(),
        }
        visual.run_until_parked();
        if loss != "focus-before-paint" {
            visual.simulate_mouse_up(outside, MouseButton::Left, gpui::Modifiers::default());
        }
        visual.run_until_parked();
        assert_eq!(
            actions.get(),
            0,
            "lost {loss} release must not activate another target"
        );
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        editor.read_with(&visual, |v, _| {
            assert!(v.tuning.drag.is_none(), "loss={loss}");
            if loss == "generation" {
                assert!(v.tuning.client.snapshot().is_none());
            } else {
                assert!(v.tuning.client.has_pending_terminal());
                assert_eq!(
                    v.tuning.client.snapshot().unwrap().sequence,
                    snapshot.sequence,
                    "cancel must not increment the commit sequence"
                );
            }
        });
    }
}

#[gpui::test]
fn created_pitch_node_owns_pointer_and_keeps_admitted_terminal(cx: &mut TestAppContext) {
    let Fixture {
        mut visual,
        editor,
        actions,
        ..
    } = open(cx);
    let bounds = editor.read_with(&visual, |v, _| v.tuning.pitch_bounds.unwrap());
    let start = point(
        bounds.left() + bounds.size.width * (3.5 / 12.),
        bounds.center().y,
    );
    visual.simulate_event(MouseDownEvent {
        position: start,
        button: MouseButton::Left,
        click_count: 2,
        ..Default::default()
    });
    editor.read_with(&visual, |v, _| {
        assert!(v.tuning.drag.is_some());
        assert_eq!(v.tuning.value.scale().count(), 13);
        assert_eq!(
            v.state.as_ref().unwrap().project.tuning().scale().count(),
            12
        );
    });
    let outside = point(px(140.), px(10.));
    visual.simulate_mouse_down(outside, MouseButton::Right, gpui::Modifiers::default());
    assert_eq!(actions.get(), 0);
    visual.run_until_parked();
    visual.simulate_mouse_up(outside, MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Right, gpui::Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.tuning.drag.is_none());
        assert!(v.tuning.client.has_pending_terminal());
        assert_eq!(
            v.tuning.client.snapshot().unwrap().tuning.scale().count(),
            13
        );
    });
    assert!(!visual.update(|w, _| w.is_pointer_dragging()));
    assert_eq!(actions.get(), 0);
}

/// Complete CPU input/repaint turns; protocol reset/analysis are outside scope.
#[gpui::test]
#[ignore = "manual optimized tuning pointer profile; no native acceptance"]
fn profile_tuning_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_TUNING_POINTER_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    let Fixture {
        mut visual,
        editor,
        start,
        ..
    } = open(cx);
    let mut timings = Vec::with_capacity(samples);
    for round in 0..warmup + samples {
        editor.update(&mut visual, |v, cx| {
            v.tuning.client.reset();
            v.tuning.value = v.state.as_ref().unwrap().project.tuning();
            v.tuning.drag = None;
            // Freeze asynchronous analysis and exclude worker/terminal setup;
            // local gesture changes and actual overlay paint remain timed.
            v.tuning.analysis_pending = true;
            v.tuning.analysis_queued = None;
            v.tuning.last_variations = Instant::now();
            cx.notify();
        });
        visual.run_until_parked();
        visual.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        visual.run_until_parked();
        assert!(editor.read_with(&visual, |v, _| v.tuning.drag.is_some()));
        let clock = Instant::now();
        let mut at = start;
        for step in 0..8 {
            at = start + point(px(((round + step) % 7) as f32 - 3.), px(0.));
            visual.simulate_mouse_move(at, MouseButton::Left, gpui::Modifiers::default());
            visual.run_until_parked();
        }
        let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
        visual.simulate_mouse_up(at, MouseButton::Left, gpui::Modifiers::default());
        visual.run_until_parked();
        if round >= warmup {
            timings.push(elapsed);
        }
    }
    timings.sort_by(f64::total_cmp);
    let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
    let scale = visual.update(|w, _| w.scale_factor());
    println!(
        "TUNING_POINTER_PROFILE size=1280x800 scale={scale} warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
        percentile(50),
        percentile(95),
        percentile(99),
        percentile(100)
    );
}
