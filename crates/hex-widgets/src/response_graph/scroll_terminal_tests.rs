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
    accent_text: 0x202020,
    numeric_fill: 0x31515a,
    hover_text: 0xe0e0e0,
    focus_marker: 0xaaaaaa,
    dial_rail: 0,
    hover: 0x404040,
    selection: 0x3350a0,
    invalid: 0xff554c,
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
                        analyzer_peak: None,
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
            // Blur notifications compare active painted focus paths. An inactive
            // test window has no such path even though FocusHandle changes.
            window.activate_window();
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
        if matches!(action, DuringEnd::Blur) {
            assert_eq!(
                f.events
                    .borrow()
                    .iter()
                    .filter(|event| {
                        event.phase == ResponseGraphScrollPhase::Cancel && event.gesture_id == old
                    })
                    .count(),
                1,
                "existing blur handler must cancel the original gesture exactly once"
            );
        }
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

#[gpui::test]
fn four_review_unfocused_fractional_end_retires_metadata_before_unphased_input(
    cx: &mut TestAppContext,
) {
    let (handle, f) = fixture(cx);
    let mut v = VisualTestContext::from_window(handle, cx);
    v.run_until_parked();
    let inside = v.debug_bounds("terminal-graph").unwrap().center();
    let other = v.debug_bounds("terminal-other").unwrap().center();
    v.update(|window, app| assert!(!f.graph.read(app).focus.is_focused(window)));
    send(&mut v, inside, TouchPhase::Started, 0.25, false);
    assert!(f.events.borrow().is_empty());
    assert!(
        f.graph
            .read_with(cx, |g, _| g.scroll.gesture.is_some_and(|s| !s.begun))
    );
    send(&mut v, other, TouchPhase::Ended, 0.0, false);
    eprintln!(
        "fractional outside Ended: active={} phased={}",
        f.graph.read_with(cx, |g, _| g.scroll.active()),
        f.graph.read_with(cx, |g, _| g.scroll.phased)
    );
    assert!(
        !f.graph.read_with(cx, |g, _| g.scroll.active()),
        "unbegun token must retire without taking focus"
    );
    assert!(!f.graph.read_with(cx, |g, _| g.scroll.phased));
    assert!(f.events.borrow().is_empty());
    send(&mut v, inside, TouchPhase::Moved, 1.0, false);
    cx.executor().advance_clock(Duration::from_millis(300));
    v.run_until_parked();
    assert_eq!(commits(&f), 1);
    assert!(!f.graph.read_with(cx, |g, _| g.scroll.active()));
}

#[gpui::test]
fn four_review_mac_cancelled_terminal_cancels_inside_and_outside_once(cx: &mut TestAppContext) {
    for outside in [false, true] {
        for units in [0.0, 0.25, 1.0] {
            let cx = &mut cx.new_app();
            let (handle, f) = fixture(cx);
            let mut v = VisualTestContext::from_window(handle, cx);
            v.run_until_parked();
            let inside = v.debug_bounds("terminal-graph").unwrap().center();
            let other = v.debug_bounds("terminal-other").unwrap().center();
            send(&mut v, inside, TouchPhase::Started, units, false);
            // Explicit adapter output, not a claim of native Mac execution.
            send(
                &mut v,
                if outside { other } else { inside },
                TouchPhase::Cancelled,
                8.0,
                false,
            );
            cx.executor().advance_clock(Duration::from_secs(1));
            v.run_until_parked();
            assert!(
                !f.graph
                    .read_with(cx, |g, _| g.scroll.active() || g.scroll.phased)
            );
            assert_eq!(commits(&f), 0);
            assert_eq!(
                f.events
                    .borrow()
                    .iter()
                    .filter(|e| e.phase == ResponseGraphScrollPhase::Cancel)
                    .count(),
                usize::from(units == 1.0)
            );
            if outside {
                assert!(matches!(
                    f.outside.borrow().as_slice(),
                    [TouchPhase::Cancelled]
                ));
                let parent = f.parent.borrow();
                if units == 0.0 {
                    // A zero start deliberately bubbles before the terminal.
                    assert!(matches!(
                        parent.as_slice(),
                        [TouchPhase::Started, TouchPhase::Cancelled]
                    ));
                } else {
                    assert!(matches!(parent.as_slice(), [TouchPhase::Cancelled]));
                }
            }
        }
    }
}

#[gpui::test]
fn four_review_physical_ticks_ignore_content_preference_and_invert_once(cx: &mut TestAppContext) {
    for inverted in [false, true] {
        for (ticks, events, lines_per_tick, expected) in [
            (0.25, 4, 3.0, 1),
            (-1.0, 1, 7.0, -1),
            (30.0, 1, 3.0, 16),
            (-30.0, 1, 0.0, -16),
            (1.0, 1, 0.0, 1),
        ] {
            let cx = &mut cx.new_app();
            let (handle, f) = fixture(cx);
            let mut v = VisualTestContext::from_window(handle, cx);
            v.run_until_parked();
            let inside = v.debug_bounds("terminal-graph").unwrap().center();
            for _ in 0..events {
                let native_ticks = if inverted { -ticks } else { ticks };
                let event = ScrollWheelEvent {
                    position: inside,
                    delta: gpui::ScrollDelta::Lines(point(0.0, native_ticks * lines_per_tick)),
                    wheel_ticks: Some(point(0.0, native_ticks)),
                    direction_inverted_from_device: inverted,
                    touch_phase: TouchPhase::Moved,
                    ..Default::default()
                };
                // Opt-in helpers must never rewrite raw content-scroll input.
                assert!(
                    matches!(event.delta, gpui::ScrollDelta::Lines(d) if d.y == native_ticks * lines_per_tick)
                );
                assert_eq!(event.physical_wheel_ticks().unwrap().y, ticks);
                assert_eq!(
                    f32::from(event.physical_delta().pixel_delta(px(24.0)).y),
                    ticks * lines_per_tick * 24.0
                );
                v.simulate_mouse_move(inside, None, Modifiers::default());
                v.simulate_event(event);
                v.run_until_parked();
            }
            cx.executor().advance_clock(Duration::from_millis(300));
            v.run_until_parked();
            assert_eq!(commits(&f), 1);
            assert_eq!(f.events.borrow().last().unwrap().steps, expected);
        }
    }
}
