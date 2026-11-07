//! Active reorder input; click candidates and pending receipts keep their owners.
use super::*;
use gpui::{CursorStyle, Render, ScrollDelta, ScrollWheelEvent, div, prelude::*};
use std::{cell::Cell, rc::Rc};
struct Host {
    editor: gpui::Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
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
    editor: gpui::Entity<EditorView>,
    ids: Vec<TrackId>,
    actions: Rc<Cell<usize>>,
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
}
fn open(cx: &mut TestAppContext, count: usize) -> Fixture {
    let (mut visual, editor, ids) = live_tracks(cx, count, false);
    let from = bounds(&mut visual, ids[0]).center();
    let row = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[2].value()).into_boxed_str(),
        ))
        .unwrap();
    let to = point(from.x, row.top() + px(1.));
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    visual.run_until_parked();
    Fixture {
        visual,
        editor,
        ids,
        actions,
        from,
        to,
    }
}
fn activate(
    visual: &mut VisualTestContext,
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
) {
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    assert!(
        !visual.update(|w, _| w.is_pointer_dragging()),
        "name press stays a normal click candidate"
    );
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_some());
}
#[gpui::test]
fn active_reorder_isolates_input_cursor_and_outside_release(cx: &mut TestAppContext) {
    let Fixture {
        mut visual,
        editor,
        actions,
        from,
        to,
        ..
    } = open(cx, 4);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    activate(&mut visual, from, to);
    let outside = point(px(640.), px(10.));
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    visual.simulate_event(ScrollWheelEvent {
        position: outside,
        delta: ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });
    assert_eq!(
        actions.get(),
        0,
        "active track reorder must isolate unrelated input"
    );
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::ClosedHand)
    );
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(actions.get(), 0);
    assert!(!visual.update(|w, _| w.is_pointer_dragging()));
    assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
    editor.read_with(&visual, |v, _| {
        assert!(!v.track_reorder_active());
        assert!(!v.edit_pending());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    assert_eq!(actions.get(), 1);
}
#[gpui::test]
fn reorder_receipt_keeps_painted_gap_without_retaining_input(cx: &mut TestAppContext) {
    let Fixture {
        mut visual,
        editor,
        ids,
        actions,
        from,
        to,
    } = open(cx, 4);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    activate(&mut visual, from, to);
    let gap = visual.debug_bounds("track-insertion-line").unwrap();
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let terminal = editor.update(&mut visual, |v, _| next(&v.service));
    assert!(terminal.request_complete && terminal.error.is_none());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("track-insertion-line"), Some(gap));
        editor.read_with(&visual, |v, _| {
            assert!(v.track_reorder_active());
            assert_eq!(v.worker_requests_pending, 1);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
    let outside = point(px(640.), px(10.));
    visual.simulate_mouse_move(outside, None, Modifiers::default());
    visual.run_until_parked();
    assert!(!visual.update(|w, _| w.is_pointer_dragging()));
    assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    assert_eq!(actions.get(), 1);
    visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());
    editor.read_with(&visual, |v, _| {
        assert!(!v.track_reorder_active());
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .iter()
                .map(|t| t.id())
                .collect::<Vec<_>>(),
            vec![ids[1], ids[0], ids[2], ids[3]]
        );
    });
}
#[derive(Clone, Copy, Debug)]
enum Loss {
    Button,
    Focus,
    Invalid,
    Escape,
    Generation,
    Mode,
    Window,
}

#[gpui::test]
fn active_reorder_button_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_reorder_loss(cx, Loss::Button);
}

#[gpui::test]
fn active_reorder_focus_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_reorder_loss(cx, Loss::Focus);
}

#[gpui::test]
fn active_reorder_invalid_input_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_reorder_loss(cx, Loss::Invalid);
}

#[gpui::test]
fn active_reorder_escape_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_reorder_loss(cx, Loss::Escape);
}

#[gpui::test]
fn active_reorder_generation_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_reorder_loss(cx, Loss::Generation);
}

#[gpui::test]
fn active_reorder_mode_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_reorder_loss(cx, Loss::Mode);
}

#[gpui::test]
fn active_reorder_window_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_reorder_loss(cx, Loss::Window);
}

fn assert_reorder_loss(cx: &mut TestAppContext, loss: Loss) {
    let cx = &mut cx.new_app();
    let Fixture {
        mut visual,
        editor,
        actions,
        from,
        to,
        ..
    } = open(cx, 4);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    activate(&mut visual, from, to);
    let outside = point(px(640.), px(10.));
    match loss {
        Loss::Button => visual.simulate_mouse_move(outside, None, Modifiers::default()),
        Loss::Escape => visual.simulate_keystrokes("escape"),
        Loss::Generation => editor.update(&mut visual, |v, cx| {
            v.project_generation += 1;
            cx.notify();
        }),
        Loss::Mode => editor.update(&mut visual, |v, cx| {
            v.mode = super::super::EditorMode::Session;
            cx.notify();
        }),
        Loss::Window => visual.deactivate_window(),
        Loss::Focus => visual.update(|w, cx| {
            editor.update(cx, |v, cx| v.search_browser(w, cx));
            w.dispatch_input(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    position: to,
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
        }),
        Loss::Invalid => visual.update(|w, cx| {
            w.dispatch_input(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: point(px(f32::NAN), px(f32::NAN)),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            w.dispatch_input(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    position: to,
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
        }),
    }
    visual.run_until_parked();
    if !matches!(loss, Loss::Focus | Loss::Invalid) {
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    }
    visual.run_until_parked();
    assert_eq!(actions.get(), 0, "loss={loss:?}");
    assert!(visual.debug_bounds("track-insertion-line").is_none());
    editor.read_with(&visual, |v, _| {
        assert!(!v.track_reorder_active(), "loss={loss:?}");
        assert!(!v.edit_pending(), "loss={loss:?}");
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn active_reorder_keeps_its_header_list_wheel_scroll(cx: &mut TestAppContext) {
    let Fixture {
        mut visual,
        editor,
        from,
        to,
        ..
    } = open(cx, 20);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    activate(&mut visual, from, to);
    let offset = editor.read_with(&visual, |v, _| v.track_scroll.offset());
    visual.simulate_event(ScrollWheelEvent {
        position: to,
        delta: ScrollDelta::Lines(point(0., -3.)),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(
            v.track_scroll.offset().y < offset.y,
            "active reorder retains gesture-owned list scrolling"
        );
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.worker_requests_pending, 0);
    });
    assert!(visual.debug_bounds("track-insertion-line").is_some());
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::ClosedHand)
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());
}
