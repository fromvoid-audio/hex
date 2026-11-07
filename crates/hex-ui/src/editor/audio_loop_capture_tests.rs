//! Actual audio brace input; the ordinary clip-loop receipt remains its owner.
use super::*;
use gpui::{
    CursorStyle, Entity, Modifiers, Render, ScrollDelta, ScrollWheelEvent, TestAppContext,
    VisualTestContext, point,
};
use std::{cell::Cell, rc::Rc, sync::Arc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
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
    _files: Arc<crate::editor::test_environment::Environment>,
    visual: VisualTestContext,
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
    start: gpui::Point<Pixels>,
    width: Pixels,
    clip: ClipId,
}
fn open(cx: &mut TestAppContext, start_edge: bool) -> Fixture {
    let (files, handle, editor) = crate::editor::clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.), px(800.)));
    let clip = editor.update(&mut visual, |v, cx| {
        let track = &v.state.as_ref().unwrap().project.tracks()[0];
        let clip = track.clips()[0].id();
        v.selected_track = Some(track.id());
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.device_panel_active = false;
        cx.notify();
        clip
    });
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
        editor.read(cx).audio_event_focus.focus(w);
    });
    visual.run_until_parked();
    let start = visual
        .debug_bounds(if start_edge {
            "audio-loop-start"
        } else {
            "audio-loop-end"
        })
        .unwrap()
        .center();
    let width = visual.debug_bounds("audio-loop-ruler").unwrap().size.width;
    Fixture {
        _files: files,
        visual,
        editor,
        actions,
        start,
        width,
        clip,
    }
}
#[gpui::test]
fn audio_loop_start_edge_keeps_input_and_pending_receipt_separate(cx: &mut TestAppContext) {
    assert_audio_loop_receipt(cx, true);
}

#[gpui::test]
fn audio_loop_end_edge_keeps_input_and_pending_receipt_separate(cx: &mut TestAppContext) {
    assert_audio_loop_receipt(cx, false);
}

fn assert_audio_loop_receipt(cx: &mut TestAppContext, start_edge: bool) {
    let cx = &mut cx.new_app();
    let Fixture {
        mut visual,
        editor,
        actions,
        start,
        width,
        clip,
        ..
    } = open(cx, start_edge);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let outside = point(px(640.), px(10.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    assert!(editor.read_with(&visual, |v, _| v.clip_loop_held()));
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    visual.simulate_event(ScrollWheelEvent {
        position: outside,
        delta: ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });
    assert_eq!(
        actions.get(),
        0,
        "audio loop handles must isolate input before motion"
    );
    let moved = start + point(width * if start_edge { 0.125 } else { -0.125 }, px(0.));
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(if start_edge {
            CursorStyle::ResizeLeft
        } else {
            CursorStyle::ResizeRight
        })
    );
    let preview = editor.read_with(&visual, |v, _| {
        v.clip_loop.audio_gesture.as_ref().unwrap().preview
    });
    visual.simulate_mouse_up(
        point(moved.x, px(10.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    let terminal = crate::editor::clip_editor_loop_ui_tests::hold(&editor, &mut visual);
    assert!(
        terminal.request_complete && terminal.error.is_none(),
        "{:?}",
        terminal.error
    );
    visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(!v.clip_loop_held());
            let root = v.pending_clip_loop_audio(clip).unwrap().audio_event_root();
            assert_eq!((root.view_start, root.view_end), preview);
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
        "pending clip-loop edit cannot hold pointer input"
    );
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(editor.read_with(&visual, |v, _| v.pending_clip_loop_audio(clip).is_none()));
}

#[derive(Clone, Copy, Debug)]
enum Loss {
    Button,
    Focus,
    Invalid,
    Escape,
    Geometry,
    Generation,
    Hidden,
    Source,
}

#[gpui::test]
fn audio_loop_start_button_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Button);
}

#[gpui::test]
fn audio_loop_end_button_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, false, Loss::Button);
}

#[gpui::test]
fn audio_loop_focus_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Focus);
}

#[gpui::test]
fn audio_loop_invalid_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Invalid);
}

#[gpui::test]
fn audio_loop_escape_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Escape);
}

#[gpui::test]
fn audio_loop_geometry_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Geometry);
}

#[gpui::test]
fn audio_loop_generation_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Generation);
}

#[gpui::test]
fn audio_loop_hidden_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Hidden);
}

#[gpui::test]
fn audio_loop_source_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Source);
}

fn assert_audio_loop_loss(cx: &mut TestAppContext, start_edge: bool, loss: Loss) {
    let cx = &mut cx.new_app();
    let Fixture {
        mut visual,
        editor,
        actions,
        start,
        width,
        ..
    } = open(cx, start_edge);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let outside = point(px(640.), px(10.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    let moved = start + point(width * if start_edge { 0.125 } else { -0.125 }, px(0.));
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    match loss {
        Loss::Button => visual.simulate_mouse_move(outside, None, Modifiers::default()),
        Loss::Invalid => visual.simulate_mouse_move(
            point(moved.x, px(f32::NAN)),
            MouseButton::Left,
            Modifiers::default(),
        ),
        Loss::Escape => visual.simulate_keystrokes("escape"),
        Loss::Geometry => visual.simulate_resize(gpui::size(px(1100.), px(750.))),
        Loss::Generation => editor.update(&mut visual, |v, cx| {
            v.project_generation += 1;
            cx.notify();
        }),
        Loss::Hidden => editor.update(&mut visual, |v, cx| {
            v.clip_view_open = false;
            cx.notify();
        }),
        Loss::Source => editor.update(&mut visual, |v, cx| {
            v.audio_event_editor.source_view = true;
            cx.notify();
        }),
        Loss::Focus => visual.update(|w, cx| {
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
    }
    visual.run_until_parked();
    if !matches!(loss, Loss::Focus) {
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    }
    visual.run_until_parked();
    assert_eq!(actions.get(), 0, "edge={start_edge} loss={loss:?}");
    if matches!(loss, Loss::Geometry) {
        let ruler = visual.debug_bounds("audio-loop-ruler").unwrap();
        let marker = visual.debug_bounds("audio-loop-start-marker").unwrap();
        assert!(
            (marker.left() - ruler.left()).abs() < px(1.),
            "cancelled geometry must repaint canonical loop bounds"
        );
    }
    editor.read_with(&visual, |v, _| {
        assert!(!v.clip_loop_held(), "edge={start_edge} loss={loss:?}");
        assert!(!v.edit_pending(), "edge={start_edge} loss={loss:?}");
        assert!(v.pending_requests.is_empty());
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

/// Real retained-root preview; setup/cancellation and worker admission excluded.
#[gpui::test]
#[ignore = "manual optimized audio loop profile; no native acceptance"]
fn profile_audio_loop_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_AUDIO_LOOP_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for start_edge in [true, false] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            start,
            width,
            ..
        } = open(cx, start_edge);
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(editor.read_with(&visual, |v, _| v.clip_loop_held()));
            let clock = std::time::Instant::now();
            let mut at = start;
            for step in 0..8 {
                let displacement = width * (((round + step) % 5) as f32 / 16.);
                at = start
                    + point(
                        if start_edge {
                            displacement
                        } else {
                            -displacement
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
                assert!(!v.clip_loop_held());
                assert!(!v.edit_pending());
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
            "AUDIO_LOOP_PROFILE start_edge={start_edge} width=1280 height=800 scale={scale} source_frames=96000 rate=48000 source_channels=1 tracks=2 clips=1 warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
