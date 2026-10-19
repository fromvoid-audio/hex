//! Real window dispatch/adjacent panel delivery and controlled deferred races.
use super::*;
use gpui::{AppContext, Render, TestAppContext, VisualTestContext, size};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const STYLE: ControlStyle = ControlStyle {
    background: 0x202020,
    surface: 0x303030,
    raised: 0x383838,
    rail: 0x282828,
    border: 0x505050,
    text: 0xe0e0e0,
    muted: 0xaaaaaa,
    accent: 0x55ddee,
    focus_marker: 0xaaaaaa,
    dial_rail: 0,
    hover: 0x404040,
    font_size: 10.0,
    text_case: crate::TextCase::Preserve,
};
#[derive(Clone, Copy, Debug)]
enum DuringEnd {
    None,
    Cancel,
    Blur,
    Missing,
    NewToken,
}
struct Host {
    graph: Entity<ResponseGraph>,
    parent: Rc<RefCell<Vec<TouchPhase>>>,
    outside: Rc<RefCell<Vec<TouchPhase>>>,
    action: Rc<Cell<DuringEnd>>,
    outside_focus: FocusHandle,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let parent = self.parent.clone();
        let outside = self.outside.clone();
        let action = self.action.clone();
        let graph = self.graph.clone();
        let focus = self.outside_focus.clone();
        div()
            .flex()
            .flex_row()
            .size_full()
            .on_scroll_wheel(move |event, _, _| parent.borrow_mut().push(event.touch_phase))
            .child(
                div()
                    .w(px(200.0))
                    .h(px(120.0))
                    .flex_shrink_0()
                    .child(self.graph.clone()),
            )
            .child(
                div()
                    .id("terminal-other")
                    .debug_selector(|| "terminal-other".into())
                    .w(px(200.0))
                    .h(px(120.0))
                    .flex_shrink_0()
                    .track_focus(&self.outside_focus)
                    .on_scroll_wheel(move |event, window, cx| {
                        outside.borrow_mut().push(event.touch_phase);
                        if event.is_momentum || !matches!(event.touch_phase, TouchPhase::Ended) {
                            return;
                        }
                        // Race injection occurs in the actual OUTSIDE panel bubble,
                        // after graph capture has scheduled its terminal effect.
                        match action.get() {
                            DuringEnd::None => {}
                            DuringEnd::Cancel => graph.update(cx, |g, cx| g.cancel(cx)),
                            DuringEnd::Blur => window.focus(&focus),
                            DuringEnd::Missing => graph.update(cx, |g, cx| {
                                let mut model = g.model.clone();
                                model.nodes.clear();
                                g.set_model(model, STYLE, cx).unwrap();
                            }),
                            DuringEnd::NewToken => graph.update(cx, |g, cx| {
                                g.cancel(cx);
                                // Controlled handler-level replacement race, NOT a
                                // claimed second physical platform input event.
                                let position = g.plot_bounds().unwrap().center();
                                g.scroll_wheel(
                                    &ScrollWheelEvent {
                                        position,
                                        delta: gpui::ScrollDelta::Lines(point(0.0, 1.0)),
                                        touch_phase: TouchPhase::Started,
                                        ..Default::default()
                                    },
                                    window,
                                    cx,
                                );
                            }),
                        }
                    }),
            )
    }
}
struct Fixture {
    graph: Entity<ResponseGraph>,
    events: Rc<RefCell<Vec<ResponseGraphScrollEvent>>>,
    parent: Rc<RefCell<Vec<TouchPhase>>>,
    outside: Rc<RefCell<Vec<TouchPhase>>>,
    action: Rc<Cell<DuringEnd>>,
    _subscription: Subscription,
}
fn fixture(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, Fixture) {
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let parent = Rc::new(RefCell::new(Vec::new()));
    let outside = Rc::new(RefCell::new(Vec::new()));
    let action = Rc::new(Cell::new(DuringEnd::None));
    let events = Rc::new(RefCell::new(Vec::new()));
    let graph = cx
        .update_window(handle, |_, window, cx| {
            let graph = cx.new(|cx| {
                let mut g = ResponseGraph::new(cx, "terminal-graph", STYLE);
                g.set_model(
                    ResponseGraphModel {
                        primary: vec![[0.0, 0.5], [1.0, 0.5]],
                        secondary: vec![[0.0, 0.5], [1.0, 0.5]],
                        analyzer: None,
                        input_analyzer: None,
                        nodes: vec![ResponseGraphNode {
                            id: 1,
                            position: [0.5, 0.5],
                            enabled: true,
                            selected: true,
                        }],
                    },
                    STYLE,
                    cx,
                )
                .unwrap();
                g.set_editable(true, cx);
                g.set_scroll_nodes(vec![1], cx).unwrap();
                g
            });
            window.replace_root(cx, |_, cx| Host {
                graph: graph.clone(),
                parent: parent.clone(),
                outside: outside.clone(),
                action: action.clone(),
                outside_focus: cx.focus_handle(),
            });
            graph
        })
        .unwrap();
    let captured = events.clone();
    let subscription = graph.update(cx, |_, cx| {
        cx.subscribe(&graph, move |_, _, e: &ResponseGraphScrollEvent, _| {
            captured.borrow_mut().push(*e)
        })
    });
    (
        handle,
        Fixture {
            graph,
            events,
            parent,
            outside,
            action,
            _subscription: subscription,
        },
    )
}
fn send(
    v: &mut VisualTestContext,
    position: Point<Pixels>,
    phase: TouchPhase,
    units: f32,
    momentum: bool,
) {
    v.simulate_mouse_move(position, None, Modifiers::default());
    v.simulate_event(ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Lines(point(0.0, units)),
        touch_phase: phase,
        is_momentum: momentum,
        ..Default::default()
    });
    v.run_until_parked();
}
fn commits(f: &Fixture) -> usize {
    f.events
        .borrow()
        .iter()
        .filter(|e| e.phase == ResponseGraphScrollPhase::Commit)
        .count()
}

#[gpui::test]
fn terminal_observer_preserves_inside_consumption_and_outside_panel_delivery(
    cx: &mut TestAppContext,
) {
    for outside in [false, true] {
        let cx = &mut cx.new_app();
        let (handle, f) = fixture(cx);
        let mut v = VisualTestContext::from_window(handle, cx);
        v.simulate_resize(size(px(500.0), px(180.0)));
        v.run_until_parked();
        let inside = v.debug_bounds("terminal-graph").unwrap().center();
        let other = v.debug_bounds("terminal-other").unwrap().center();
        send(&mut v, inside, TouchPhase::Started, 1.0, false);
        assert!(f.graph.read_with(cx, |g, _| g.scroll.active()));
        assert!(f.parent.borrow().is_empty());
        // A nonzero terminal delta must not accidentally edit or page inside.
        send(
            &mut v,
            if outside { other } else { inside },
            TouchPhase::Ended,
            2.0,
            false,
        );
        assert_eq!(commits(&f), 1);
        assert!(!f.graph.read_with(cx, |g, _| g.scroll.active()));
        assert_eq!(f.events.borrow().last().unwrap().steps, 1);
        if outside {
            assert!(matches!(f.outside.borrow().as_slice(), [TouchPhase::Ended]));
            assert!(matches!(f.parent.borrow().as_slice(), [TouchPhase::Ended]));
        } else {
            assert!(f.outside.borrow().is_empty());
            assert!(
                f.parent.borrow().is_empty(),
                "normal inside terminal stays consumed"
            );
        }
    }
}
#[gpui::test]
fn terminal_observer_zero_start_and_momentum_keep_existing_history_laws(cx: &mut TestAppContext) {
    let (handle, f) = fixture(cx);
    let mut v = VisualTestContext::from_window(handle, cx);
    v.run_until_parked();
    let inside = v.debug_bounds("terminal-graph").unwrap().center();
    let other = v.debug_bounds("terminal-other").unwrap().center();
    send(&mut v, inside, TouchPhase::Started, 0.0, false);
    assert!(f.events.borrow().is_empty());
    assert!(f.graph.read_with(cx, |g, _| g.scroll.phased));
    send(&mut v, other, TouchPhase::Ended, 0.0, false);
    assert!(f.events.borrow().is_empty());
    assert!(!f.graph.read_with(cx, |g, _| g.scroll.phased));
    send(&mut v, inside, TouchPhase::Moved, 1.0, false);
    cx.executor().advance_clock(Duration::from_millis(300));
    v.run_until_parked();
    assert_eq!(commits(&f), 1);
    send(&mut v, inside, TouchPhase::Started, 1.0, false);
    let token = f
        .graph
        .read_with(cx, |g, _| g.scroll.gesture.unwrap().token);
    send(&mut v, other, TouchPhase::Ended, 0.0, true);
    assert_eq!(commits(&f), 1);
    assert_eq!(
        f.graph
            .read_with(cx, |g, _| g.scroll.gesture.unwrap().token),
        token
    );
    send(&mut v, other, TouchPhase::Ended, 0.0, false);
    assert_eq!(commits(&f), 2);
}
#[gpui::test]
fn terminal_observer_does_not_late_commit_cancel_blur_missing_or_new_token(
    cx: &mut TestAppContext,
) {
    for action in [
        DuringEnd::Cancel,
        DuringEnd::Missing,
        DuringEnd::NewToken,
        DuringEnd::Blur,
    ] {
        let cx = &mut cx.new_app();
        let (handle, f) = fixture(cx);
        let mut v = VisualTestContext::from_window(handle, cx);
        v.run_until_parked();
        let inside = v.debug_bounds("terminal-graph").unwrap().center();
        let other = v.debug_bounds("terminal-other").unwrap().center();
        send(&mut v, inside, TouchPhase::Started, 1.0, false);
        let old = f
            .graph
            .read_with(cx, |g, _| g.scroll.gesture.unwrap().token);
        f.action.set(action);
        send(&mut v, other, TouchPhase::Ended, 0.0, false);
        assert!(matches!(f.outside.borrow().as_slice(), [TouchPhase::Ended]));
        eprintln!(
            "deferred_terminal_invalidation action={action:?} commits={} active_token={:?}",
            commits(&f),
            f.graph
                .read_with(cx, |g, _| g.scroll.gesture.map(|s| s.token))
        );
        assert_eq!(
            commits(&f),
            0,
            "invalidated before deferred effect: {action:?}"
        );
        if matches!(action, DuringEnd::NewToken) {
            assert!(
                f.graph
                    .read_with(cx, |g, _| g.scroll.gesture.unwrap().token)
                    > old
            );
        } else {
            assert!(
                !f.graph.read_with(cx, |g, _| g.scroll.active()),
                "{action:?}"
            );
        }
        f.graph.update(cx, |g, cx| g.cancel(cx));
    }
}
