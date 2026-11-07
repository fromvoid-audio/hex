//! Input ownership through the actual editor; source/receipt custody is unchanged.
use super::*;
use gpui::{CursorStyle, Render, ScrollDelta, ScrollWheelEvent, div};
use std::{cell::Cell, rc::Rc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
    x: gpui::Pixels,
}
impl Render for Host {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        let down = self.actions.clone();
        let up = self.actions.clone();
        let wheel = self.actions.clone();
        let editor = self.editor.clone();
        div().size_full().child(editor).child(
            div()
                .absolute()
                .left(self.x - px(40.))
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
    _files: Arc<crate::editor::test_environment::Environment>,
    visual: VisualTestContext,
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
    down: gpui::Point<gpui::Pixels>,
    outside: gpui::Point<gpui::Pixels>,
    cursor: CursorStyle,
}
fn fixture(cx: &mut TestAppContext, kind: &str) -> Fixture {
    let (files, mut visual, editor, clip) = open(cx);
    if kind == "move" {
        edit(
            &mut visual,
            &editor,
            clip,
            AudioEventEdit::Split { at: 48_000 },
        );
    }
    if kind == "range" {
        select(&mut visual, 0.2, 0.4);
    }
    if kind == "fade" {
        focus_events(&mut visual);
        fade_key(&mut visual, true);
    }
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let event = visual.debug_bounds("audio-event-1").unwrap();
    let (down, cursor) = match kind {
        "start" => (
            visual
                .debug_bounds("audio-play-start-handle")
                .unwrap()
                .center(),
            CursorStyle::ResizeLeftRight,
        ),
        "select" => (
            point(
                bounds.left() + bounds.size.width * 0.25,
                bounds.top() + px(10.),
            ),
            CursorStyle::Arrow,
        ),
        "move" => (
            point(event.center().x, event.top() + px(8.)),
            CursorStyle::OpenHand,
        ),
        "range" => (
            visual
                .debug_bounds("audio-event-range-handle")
                .unwrap()
                .center(),
            CursorStyle::OpenHand,
        ),
        "leading" => (
            point(event.left() + px(2.), event.center().y),
            CursorStyle::ResizeLeftBracket,
        ),
        "trailing" => (
            point(event.right() - px(2.), event.center().y),
            CursorStyle::ResizeRightBracket,
        ),
        "gain" => (
            visual.debug_bounds("audio-event-gain-1").unwrap().center(),
            CursorStyle::ResizeUpDown,
        ),
        "fade" => (
            visual
                .debug_bounds("audio-event-fade-out-top-1")
                .unwrap()
                .center(),
            CursorStyle::ResizeLeftRight,
        ),
        _ => unreachable!(),
    };
    let outside = point(bounds.center().x, px(10.));
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
            x: outside.x,
        });
    });
    visual.run_until_parked();
    Fixture {
        _files: files,
        visual,
        editor,
        actions,
        down,
        outside,
        cursor,
    }
}
#[gpui::test]
fn held_event_gestures_isolate_other_input_and_finish_outside(cx: &mut TestAppContext) {
    for kind in [
        "select", "move", "range", "leading", "trailing", "fade", "gain", "start",
    ] {
        let Fixture {
            _files,
            mut visual,
            editor,
            actions,
            down,
            outside,
            cursor,
        } = fixture(cx, kind);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            let gesture = v
                .audio_event_editor
                .gesture
                .as_ref()
                .expect("real gesture must start");
            assert!(
                matches!(
                    (kind, gesture.kind),
                    ("start", Kind::PlayStart { .. })
                        | ("select", Kind::Select)
                        | ("move", Kind::Move(_) | Kind::FreeMove { .. })
                        | ("range", Kind::RangeMove { .. } | Kind::FreeRangeMove { .. })
                        | ("leading", Kind::Leading(_))
                        | ("trailing", Kind::Trailing(_))
                        | ("fade", Kind::Fade(_))
                        | ("gain", Kind::Gain { .. })
                ),
                "wrong fixture kind={kind}"
            );
        });
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "kind={kind} must isolate input before motion"
        );
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(cursor), "kind={kind}");
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(actions.get(), 0, "kind={kind}");
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        editor.read_with(&visual, |v, _| {
            assert!(v.audio_event_editor.gesture.is_none());
            if kind != "select" {
                assert!(
                    if kind == "start" {
                        v.detail_clip_id()
                            .and_then(|id| v.pending_clip_loop_audio(id))
                            .is_some()
                    } else {
                        v.audio_event_editor.pending.is_some()
                    },
                    "kind={kind} must hand off one frozen edit"
                );
            }
        });
        for _ in 0..2 {
            repaint(&mut visual, &editor);
        }
        editor.read_with(&visual, |v, _| {
            if kind != "select" {
                assert!(if kind == "start" {
                    v.detail_clip_id()
                        .and_then(|id| v.pending_clip_loop_audio(id))
                        .is_some()
                } else {
                    v.audio_event_editor.pending.is_some()
                });
            }
        });
        assert!(
            !visual.update(|w, _| w.is_pointer_dragging()),
            "pending receipt must not own input"
        );
    }
}
#[gpui::test]
fn lost_event_pointers_never_commit_or_activate_late_release(cx: &mut TestAppContext) {
    for kind in ["leading", "fade", "gain", "start"] {
        for loss in ["button", "focus", "generation", "hide", "escape", "invalid"] {
            let Fixture {
                _files,
                mut visual,
                editor,
                actions,
                down,
                outside,
                ..
            } = fixture(cx, kind);
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(
                point(down.x + (outside.x - down.x) * 0.25, down.y),
                MouseButton::Left,
                Modifiers::default(),
            );
            assert!(editor.read_with(&visual, |v, _| v.audio_event_editor.gesture.is_some()));
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "invalid" => visual.simulate_mouse_move(
                    point(px(f32::NAN), outside.y),
                    MouseButton::Left,
                    Modifiers::default(),
                ),
                "focus" => visual.update(|w, cx| {
                    editor.update(cx, |v, _| v.focus.focus(w));
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
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
                "hide" => editor.update(&mut visual, |v, cx| {
                    v.clip_view_open = false;
                    cx.notify();
                }),
                "escape" => visual.simulate_keystrokes("escape"),
                _ => unreachable!(),
            }
            // The focus case dispatches both operations in one update so a
            // simulator repaint cannot hide the input-turn ordering boundary.
            visual.run_until_parked();
            if loss != "focus" {
                visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
            }
            visual.run_until_parked();
            assert_eq!(actions.get(), 0, "kind={kind} loss={loss}");
            editor.read_with(&visual, |v, _| {
                assert!(v.audio_event_editor.gesture.is_none());
                assert!(v.audio_event_editor.pending.is_none());
                assert!(
                    v.detail_clip_id()
                        .and_then(|id| v.pending_clip_loop_audio(id))
                        .is_none()
                );
                assert!(v.pending_requests.is_empty());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}

/// Actual editor input/paint; cancellation and worker processing are not timed.
#[gpui::test]
#[ignore = "manual optimized audio-event pointer profile; no native acceptance"]
fn profile_audio_event_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_EVENT_POINTER_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for kind in ["select", "fade"] {
        let Fixture {
            _files,
            mut visual,
            editor,
            down,
            ..
        } = fixture(cx, kind);
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            editor.update(&mut visual, |v, cx| {
                assert_eq!(v.worker_requests_pending, 0);
                assert!(v.pending_requests.is_empty());
                assert!(v.audio_event_editor.pending.is_none());
                assert!(v.audio_event_editor.gesture.is_none());
                cx.notify();
            });
            visual.run_until_parked();
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(editor.read_with(&visual, |v, _| v.audio_event_editor.gesture.is_some()));
            let clock = std::time::Instant::now();
            let mut at = down;
            for step in 0..8 {
                at = down + point(px(-(((round + step) % 7) as f32) * 2.), px(0.));
                visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
            }
            let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
            // Cancel after measurement, before release can submit an edit.
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = visual.update(|w, _| w.scale_factor());
        println!(
            "EVENT_POINTER_PROFILE kind={kind} size=1280x800 scale={scale} warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
