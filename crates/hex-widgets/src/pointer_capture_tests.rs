//! Numeric input ownership, tested through completed scenes and real widget events.
use crate::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
};
use gpui::{
    Context, CursorStyle, Entity, IntoElement, Modifiers, MouseButton, Render, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, Window, div, point,
    prelude::*, px, size,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct Root {
    control: Entity<NumericControl>,
    show: bool,
    actions: Rc<Cell<usize>>,
    keys: Rc<Cell<usize>>,
}
impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let keys = self.keys.clone();
        let stray_drag = self.actions.clone();
        let down = self.actions.clone();
        let up = self.actions.clone();
        let right_up = self.actions.clone();
        let scroll = self.actions.clone();
        div()
            .size_full()
            .id("capture-container")
            .on_drag((), move |_, _, _, cx| {
                stray_drag.set(stray_drag.get() + 1);
                cx.new(|_| gpui::Empty)
            })
            .debug_selector(|| "capture-surface".into())
            .bg(gpui::rgb(0x111111))
            .hover(|style| style.bg(gpui::rgb(0xff0000)))
            .active(|style| style.bg(gpui::rgb(0x00ff00)))
            .on_key_down(move |event, _, _| {
                if event.keystroke.key == "space" {
                    keys.set(keys.get() + 1);
                }
            })
            .when(self.show, |d| {
                let mut layout = div().w(px(100.)).h(px(100.));
                d.child(gpui::AnyView::from(self.control.clone()).cached(layout.style().clone()))
            })
            .child(
                div()
                    .id("other")
                    .debug_selector(|| "other".into())
                    .absolute()
                    .left(px(200.))
                    .top(px(20.))
                    .size(px(80.))
                    .cursor(CursorStyle::Crosshair)
                    .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                    .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                    .on_mouse_up(MouseButton::Right, move |_, _, _| {
                        right_up.set(right_up.get() + 1)
                    })
                    .on_scroll_wheel(move |_, _, _| scroll.set(scroll.get() + 1)),
            )
    }
}

#[gpui::test]
fn numeric_capture_isolates_actions_scroll_and_cursor_before_motion(cx: &mut TestAppContext) {
    cx.update(crate::bind_numeric_control_keys);
    let actions = Rc::new(Cell::new(0));
    let handle = cx.add_window(|_, cx| Root {
        control: cx.new(|cx| {
            NumericControl::from_config(
                cx,
                NumericControlConfig::new(
                    "captured",
                    "volume",
                    NumericControlKind::HorizontalSlider,
                    NumericSpec::new(0., 1., 0.5, NumericScale::Linear, 2, None::<String>).unwrap(),
                    0.5,
                    crate::tests::STYLE,
                )
                .unwrap()
                .with_presentation(NumericControlPresentation::CompactSlider),
            )
        }),
        show: true,
        actions: actions.clone(),
        keys: Rc::new(Cell::new(0)),
    });
    let control = handle
        .read_with(cx, |root, _| root.control.clone())
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(&control, move |_, event: &NumericControlEvent, _| {
            observed.borrow_mut().push(event.phase)
        })
    });
    let mut v = VisualTestContext::from_window(*handle, cx);
    v.simulate_resize(size(px(400.), px(140.)));
    v.update(|window, _| window.activate_window());
    v.run_until_parked();
    let start = v.debug_bounds("captured-value").unwrap().center();
    let other = point(px(220.), px(50.));
    v.simulate_mouse_move(start, None, Modifiers::default());
    v.run_until_parked();
    assert_eq!(
        v.rendered_background("capture-surface"),
        Some(gpui::rgb(0xff0000).into())
    );
    v.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    v.run_until_parked();
    assert_eq!(
        v.rendered_background("capture-surface"),
        Some(gpui::rgb(0x111111).into()),
        "capture must retire ordinary hover before any movement"
    );
    v.simulate_keystrokes("space");
    assert_eq!(
        handle.read_with(cx, |r, _| r.keys.get()).unwrap(),
        1,
        "mouse ownership must preserve applicable keyboard shortcuts"
    );
    // A second button must not act even before the drag crosses its threshold.
    v.simulate_mouse_down(other, MouseButton::Right, Modifiers::default());
    v.simulate_event(ScrollWheelEvent {
        position: other,
        delta: ScrollDelta::Pixels(point(px(0.), px(12.))),
        touch_phase: TouchPhase::Moved,
        modifiers: Modifiers::default(),
        ..Default::default()
    });
    v.run_until_parked();
    assert_eq!(
        actions.get(),
        0,
        "unrelated input must not run while a numeric pointer is held"
    );
    assert_ne!(v.platform_cursor_style(), CursorStyle::Crosshair);
    v.simulate_mouse_move(other, Some(MouseButton::Left), Modifiers::default());
    v.run_until_parked();
    assert_eq!(v.platform_cursor_style(), CursorStyle::Hidden);
    v.simulate_mouse_up(other, MouseButton::Left, Modifiers::default());
    v.run_until_parked();
    assert_eq!(
        actions.get(),
        0,
        "release must not activate the element underneath"
    );
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|phase| **phase == NumericEventPhase::Commit)
            .count(),
        1
    );
    assert!(!control.read_with(cx, |c, _| c.is_interacting()));
    v.simulate_mouse_move(other, None, Modifiers::default());
    v.run_until_parked();
    assert_eq!(
        actions.get(),
        0,
        "an ancestor must not retain a stale drag latch after release"
    );
    v.simulate_mouse_up(other, MouseButton::Right, Modifiers::default());
    assert_eq!(
        actions.get(),
        0,
        "a secondary button's release must also stay isolated"
    );
    v.simulate_mouse_down(other, MouseButton::Right, Modifiers::default());
    assert_eq!(
        actions.get(),
        1,
        "ordinary input must return immediately after release"
    );
    v.simulate_mouse_up(other, MouseButton::Right, Modifiers::default());
    let before = actions.get();
    // The lease and framework latch reset cannot depend on a repaint between
    // input events. In particular an idle move must not start the old ancestor.
    v.update(|window, cx| {
        window.dispatch_input(
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position: start,
                button: MouseButton::Left,
                ..Default::default()
            }),
            cx,
        );
        window.dispatch_input(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: other,
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        );
        window.dispatch_input(
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position: other,
                button: MouseButton::Left,
                ..Default::default()
            }),
            cx,
        );
        window.dispatch_input(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: other,
                pressed_button: None,
                ..Default::default()
            }),
            cx,
        );
    });
    v.run_until_parked();
    assert_eq!(
        actions.get(),
        before,
        "batched capture must not leave a click/drag latch"
    );
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|p| **p == NumericEventPhase::Commit)
            .count(),
        2
    );
}

#[gpui::test]
fn numeric_capture_cancels_once_on_loss_and_swallows_the_old_release(cx: &mut TestAppContext) {
    cx.update(crate::bind_numeric_control_keys);
    for terminal in 0..8 {
        let actions = Rc::new(Cell::new(0));
        let handle = cx.add_window(|_, cx| Root {
            control: cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    NumericControlConfig::new(
                        "captured",
                        "pan",
                        NumericControlKind::Knob,
                        NumericSpec::new(0., 1., 0.5, NumericScale::Linear, 2, None::<String>)
                            .unwrap(),
                        0.5,
                        crate::tests::STYLE,
                    )
                    .unwrap(),
                )
            }),
            show: true,
            actions: actions.clone(),
            keys: Rc::new(Cell::new(0)),
        });
        let control = handle
            .read_with(cx, |root, _| root.control.clone())
            .unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(&control, move |_, event: &NumericControlEvent, _| {
                observed.borrow_mut().push(event.phase)
            })
        });
        let mut v = VisualTestContext::from_window(*handle, cx);
        v.simulate_resize(size(px(400.), px(140.)));
        v.update(|window, _| window.activate_window());
        v.run_until_parked();
        let start = v.debug_bounds("captured-knob").unwrap().center();
        let other = point(px(220.), px(50.));
        v.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        v.simulate_mouse_move(
            start - point(px(0.), px(12.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        v.run_until_parked();
        // A parent repaint must renew the held owner through cached child replay.
        v.update(|window, cx| {
            window
                .root::<Root>()
                .unwrap()
                .unwrap()
                .update(cx, |_, cx| cx.notify())
        });
        v.run_until_parked();
        assert!(control.read_with(cx, |c, _| c.is_interacting()));
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|p| **p == NumericEventPhase::Cancel)
                .count(),
            0
        );
        match terminal {
            0 => v.simulate_keystrokes("escape"),
            1 => v.simulate_mouse_move(other, None, Modifiers::default()),
            2 => v.deactivate_window(),
            3 => control.update(cx, |c, cx| c.set_enabled(false, cx)),
            4 => v.update(|window, cx| {
                window.root::<Root>().unwrap().unwrap().update(cx, |r, cx| {
                    r.show = false;
                    cx.notify();
                })
            }),
            5 => v.update(|window, cx| {
                window.replace_root(cx, |_, _| gpui::Empty);
            }),
            6 => v.update(|window, _| window.remove_window()),
            _ => v.update(|window, _| window.blur()),
        }
        v.run_until_parked();
        assert!(
            !control.read_with(cx, |c, _| c.is_interacting()),
            "terminal {terminal}"
        );
        assert_eq!(
            control.read_with(cx, |c, _| c.value()),
            0.5,
            "terminal {terminal}"
        );
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|p| **p == NumericEventPhase::Cancel)
                .count(),
            1,
            "terminal {terminal}"
        );
        assert!(!events.borrow().contains(&NumericEventPhase::Commit));
        if matches!(terminal, 5 | 6) {
            continue;
        }
        v.simulate_mouse_up(other, MouseButton::Left, Modifiers::default());
        v.run_until_parked();
        assert_eq!(
            actions.get(),
            0,
            "cancelled mouse-up leaked for terminal {terminal}"
        );
        assert_eq!(v.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        v.simulate_mouse_down(other, MouseButton::Right, Modifiers::default());
        assert_eq!(
            actions.get(),
            1,
            "normal input did not resume for terminal {terminal}"
        );
    }
}
