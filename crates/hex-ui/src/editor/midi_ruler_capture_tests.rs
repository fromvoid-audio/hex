//! Actual source-marker/brace input; clip-loop receipts retain admitted custody.
use super::*;
use crate::editor::{
    midi_canvas::creation_tests::{fixture, pump},
    test_environment::Environment,
};
use gpui::{CursorStyle, Entity, Modifiers, Render, TestAppContext, VisualTestContext};
use std::{cell::Cell, rc::Rc, sync::Arc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut ViewContext<Self>,
    ) -> impl gpui::IntoElement {
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
    _environment: Arc<Environment>,
    clip: ClipId,
    start: gpui::Point<gpui::Pixels>,
    width: gpui::Pixels,
}
fn open(cx: &mut TestAppContext, handle: Handle) -> Fixture {
    let (mut visual, editor, environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(hex_project::Command::SetMidiClipSource {
                clip,
                source_length: hex_project::BeatTime::from_quarters(4).unwrap(),
                source_start: hex_project::BeatTime::ZERO,
                loop_start: hex_project::BeatTime::ONE,
                loop_end: hex_project::BeatTime::from_quarters(3).unwrap(),
                looping: false,
                length: hex_project::BeatTime::from_quarters(4).unwrap(),
            }))
        )
    });
    pump(&editor, &mut visual);
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    visual.run_until_parked();
    let b = visual
        .debug_bounds(if handle == Handle::Start {
            "midi-source-ruler"
        } else {
            "midi-loop-ruler"
        })
        .unwrap();
    let fraction = match handle {
        Handle::Start => 0.,
        Handle::LoopStart => 0.25,
        Handle::LoopEnd => 0.75,
        Handle::LoopBody => 0.5,
    };
    let width = b.size.width - px(2.);
    let start = point(b.origin.x + px(1.) + width * fraction, b.origin.y + px(11.));
    Fixture {
        visual,
        editor,
        actions,
        _environment: environment,
        clip,
        start,
        width,
    }
}
fn cursor(handle: Handle) -> CursorStyle {
    match handle {
        Handle::Start => CursorStyle::ResizeLeftRight,
        Handle::LoopStart => CursorStyle::ResizeLeft,
        Handle::LoopEnd => CursorStyle::ResizeRight,
        Handle::LoopBody => CursorStyle::ClosedHand,
    }
}
#[gpui::test]
fn midi_source_handles_capture_input_without_holding_clip_loop_receipt(cx: &mut TestAppContext) {
    for handle in [
        Handle::Start,
        Handle::LoopStart,
        Handle::LoopEnd,
        Handle::LoopBody,
    ] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            actions,
            start,
            width,
            clip,
            ..
        } = open(cx, handle);
        let outside = point(px(640.), px(10.));
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        assert!(editor.read_with(&visual, |v, _| v.midi_ruler.held()));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: gpui::ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "MIDI source handles must isolate input before motion"
        );
        let moved = start
            + point(
                width * if handle == Handle::LoopEnd { -0.1 } else { 0.1 },
                px(0.),
            );
        visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(cursor(handle)));
        let preview = editor.read_with(&visual, |v, _| v.midi_ruler.gesture.unwrap().preview);
        visual.simulate_mouse_up(
            point(moved.x, px(10.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        let terminal = crate::editor::clip_editor_loop_ui_tests::hold(&editor, &mut visual);
        assert!(terminal.request_complete && terminal.error.is_none());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(!v.midi_ruler.held());
                assert_eq!(v.pending_clip_loop_midi(clip), Some(preview));
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
        assert_eq!(actions.get(), 0);
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        visual.simulate_mouse_move(outside, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(
            actions.get(),
            1,
            "admitted clip-loop receipt must not own pointer input"
        );
        editor.update(&mut visual, |v, cx| {
            v.receive(terminal);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(editor.read_with(&visual, |v, _| v.pending_clip_loop_midi(clip).is_none()));
    }
}
#[gpui::test]
fn midi_source_capture_loss_discards_only_held_preview_before_late_up(cx: &mut TestAppContext) {
    for handle in [Handle::Start, Handle::LoopBody] {
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
                width,
                ..
            } = open(cx, handle);
            let outside = point(px(640.), px(10.));
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            let moved = start + point(width * 0.1, px(0.));
            visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "invalid" => visual.simulate_mouse_move(
                    point(moved.x, px(f32::NAN)),
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
                        gpui::PlatformInput::MouseUp(MouseUpEvent {
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
            assert_eq!(actions.get(), 0, "handle={handle:?} loss={loss}");
            editor.read_with(&visual, |v, _| {
                assert!(!v.midi_ruler.held(), "handle={handle:?} loss={loss}");
                assert!(!v.edit_pending(), "handle={handle:?} loss={loss}");
                assert_eq!(v.worker_requests_pending, 0);
                assert!(v.pending_requests.is_empty());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}

/// Four actual source handles; worker admission/setup/rollback excluded.
#[gpui::test]
#[ignore = "manual optimized MIDI ruler profile; no native acceptance"]
fn profile_midi_ruler_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_MIDI_RULER_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for handle in [
        Handle::Start,
        Handle::LoopStart,
        Handle::LoopEnd,
        Handle::LoopBody,
    ] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            start,
            width,
            ..
        } = open(cx, handle);
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(editor.read_with(&visual, |v, _| v.midi_ruler.held()));
            let clock = std::time::Instant::now();
            let mut at = start;
            for step in 0..8 {
                let displacement = width * (((round + step) % 5) as f32 / 16.);
                at = start
                    + point(
                        if handle == Handle::LoopEnd {
                            -displacement
                        } else {
                            displacement
                        },
                        px(0.),
                    );
                visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
            }
            let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(!v.midi_ruler.held());
                assert!(!v.edit_pending());
                assert!(v.pending_requests.is_empty());
                assert_eq!(v.worker_requests_pending, 0);
            });
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let (scale, viewport) = visual.update(|w, _| (w.scale_factor(), w.viewport_size()));
        println!(
            "MIDI_RULER_PROFILE handle={handle:?} width={} height={} scale={scale} source_beats=4 clip_count=1 notes=0 warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            f32::from(viewport.width),
            f32::from(viewport.height),
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
