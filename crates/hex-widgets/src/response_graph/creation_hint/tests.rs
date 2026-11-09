use super::*;
use gpui::{AppContext, Render, TestAppContext, VisualTestContext, size};
use std::{cell::RefCell, rc::Rc};

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
const RISE: ResponseGraphCreationHint = ResponseGraphCreationHint {
    id: "rise",
    points: &[[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]],
};
const FALL: ResponseGraphCreationHint = ResponseGraphCreationHint {
    id: "fall",
    points: &[[0.0, 1.0], [0.5, 0.75], [1.0, 0.0]],
};
fn resolve(position: [f32; 2]) -> ResponseGraphCreationHint {
    if position[0] < 0.5 { RISE } else { FALL }
}
fn invalid(_: [f32; 2]) -> ResponseGraphCreationHint {
    ResponseGraphCreationHint {
        id: "invalid",
        points: &[[0.0, 0.0], [1.0, f32::NAN]],
    }
}
struct Host {
    graph: Entity<ResponseGraph>,
    other: FocusHandle,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .child(
                div()
                    .w(px(200.0))
                    .h(px(120.0))
                    .flex_shrink_0()
                    .child(self.graph.clone()),
            )
            .child(
                div()
                    .id("hint-other")
                    .debug_selector(|| "hint-other".into())
                    .w(px(100.0))
                    .h(px(120.0))
                    .track_focus(&self.other)
                    .on_mouse_down(MouseButton::Left, {
                        let other = self.other.clone();
                        move |_, window, _| other.focus(window)
                    }),
            )
    }
}
struct Fixture {
    graph: Entity<ResponseGraph>,
    visual: VisualTestContext,
    events: Rc<RefCell<Vec<ResponseGraphEvent>>>,
    _subscription: Subscription,
}
fn fixture(cx: &mut TestAppContext, hints: bool) -> Fixture {
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let graph = cx
        .update_window(handle, |_, window, cx| {
            let graph = cx.new(|cx| {
                let mut graph = ResponseGraph::new(cx, "hint-graph", STYLE);
                graph.set_editable(true, cx);
                graph.set_creation_enabled(true);
                graph.set_creation_drag_enabled(true);
                graph
                    .set_model(
                        ResponseGraphModel {
                            primary: vec![[0.0, 0.5], [1.0, 0.5]],
                            secondary: vec![[0.0, 0.5], [1.0, 0.5]],
                            analyzer: None,
                            input_analyzer: None,
                            analyzer_peak: None,
                            nodes: vec![ResponseGraphNode {
                                id: 3,
                                position: [0.5, 0.5],
                                enabled: false,
                                selected: false,
                            }],
                        },
                        STYLE,
                        cx,
                    )
                    .unwrap();
                if hints {
                    graph.set_creation_hint_resolver(Some(resolve), cx);
                }
                graph
            });
            window.replace_root(cx, |_, cx| Host {
                graph: graph.clone(),
                other: cx.focus_handle(),
            });
            window.activate_window();
            graph
        })
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let subscription = graph.update(cx, |_, cx| {
        cx.subscribe(&graph, move |_, _, event: &ResponseGraphEvent, _| {
            captured.borrow_mut().push(*event)
        })
    });
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(360.0), px(180.0)));
    visual.run_until_parked();
    Fixture {
        graph,
        visual,
        events,
        _subscription: subscription,
    }
}
fn position(f: &Fixture, x: f32, y: f32) -> Point<Pixels> {
    let plot = f
        .graph
        .read_with(&f.visual, |graph, _| graph.plot_bounds().unwrap());
    point(
        plot.left() + plot.size.width * x,
        plot.top() + plot.size.height * (1.0 - y),
    )
}
fn hover(f: &mut Fixture, point: Point<Pixels>) {
    f.visual
        .simulate_mouse_move(point, None, Modifiers::default());
    f.visual.run_until_parked();
}
#[track_caller]
fn assert_hidden(f: &mut Fixture) {
    let state = f.graph.read_with(&f.visual, |graph, _| {
        (
            graph
                .creation_hover
                .map(|hover| (hover.pointer, hover.hint.id)),
            graph.press.is_some(),
            graph.drag.is_some(),
            graph.editable,
            graph.creation_enabled,
        )
    });
    let focused = f
        .visual
        .update(|window, cx| f.graph.read(cx).focus.is_focused(window));
    for selector in [
        "hint-graph-creation-hint-rise",
        "hint-graph-creation-hint-fall",
        "hint-graph-creation-hint-invalid",
    ] {
        assert!(
            f.visual.debug_bounds(selector).is_none(),
            "unexpected visible hint {selector} after terminal at {}; (hover,press,drag,editable,creation)={state:?}, focused={focused}",
            std::panic::Location::caller()
        );
    }
    assert!(
        f.graph
            .read_with(&f.visual, |graph, _| graph.creation_hover.is_none())
    );
}

#[gpui::test]
fn unset_hint_preserves_existing_blank_creation_and_pointer_cursor(cx: &mut TestAppContext) {
    let mut f = fixture(cx, false);
    let blank = position(&f, 0.2, 0.8);
    hover(&mut f, blank);
    assert_hidden(&mut f);
    assert_eq!(
        f.visual.rendered_cursor_style(),
        Some(CursorStyle::PointingHand)
    );
    f.visual.simulate_click(blank, Modifiers::default());
    assert_eq!(
        f.events
            .borrow()
            .iter()
            .filter(|event| event.phase == ResponseGraphPhase::Create)
            .count(),
        1
    );
}

#[gpui::test]
fn hints_follow_blank_resolution_and_node_hits_own_both_hover_and_press(cx: &mut TestAppContext) {
    let mut f = fixture(cx, true);
    for (x, id) in [(0.2, "rise"), (0.8, "fall")] {
        let blank = position(&f, x, 0.8);
        hover(&mut f, blank);
        let selector = if id == "rise" {
            "hint-graph-creation-hint-rise"
        } else {
            "hint-graph-creation-hint-fall"
        };
        let icon = f
            .visual
            .debug_bounds(selector)
            .expect("actual icon is mounted beside pointer");
        assert_eq!(icon.size, size(px(18.0), px(14.0)));
        assert!(f.visual.rendered_background(selector).is_some());
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(CursorStyle::Crosshair)
        );
    }
    let node = position(&f, 0.5, 0.5);
    hover(&mut f, point(node.x + px(7.0), node.y));
    assert_hidden(&mut f);
    assert_eq!(
        f.visual.rendered_cursor_style(),
        Some(CursorStyle::PointingHand)
    );
    f.visual
        .simulate_click(point(node.x + px(7.0), node.y), Modifiers::default());
    let events = f.events.borrow();
    assert!(
        events
            .iter()
            .any(|event| event.phase == ResponseGraphPhase::Select && event.node_id == 3)
    );
    assert!(
        events
            .iter()
            .all(|event| event.phase != ResponseGraphPhase::Create),
        "disabled visible nodes still own their full radius"
    );
}

#[gpui::test]
fn press_capture_leave_cancel_blur_and_disable_retire_hints_without_intercepting_creation(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, true);
    let blank = position(&f, 0.2, 0.8);
    hover(&mut f, blank);
    f.visual
        .simulate_mouse_down(blank, MouseButton::Left, Modifiers::default());
    f.visual.run_until_parked();
    assert_hidden(&mut f);
    assert!(
        f.events
            .borrow()
            .iter()
            .any(|event| event.phase == ResponseGraphPhase::Create)
    );
    let dragged = point(blank.x + px(20.0), blank.y);
    f.visual
        .simulate_mouse_move(dragged, MouseButton::Left, Modifiers::default());
    f.visual.run_until_parked();
    assert_hidden(&mut f);
    f.graph.update(&mut f.visual, |graph, cx| graph.cancel(cx));
    f.visual
        .simulate_mouse_up(dragged, MouseButton::Left, Modifiers::default());
    f.visual.run_until_parked();
    hover(&mut f, blank);
    f.graph.update(&mut f.visual, |graph, cx| graph.cancel(cx));
    f.visual.run_until_parked();
    assert_hidden(&mut f);
    hover(&mut f, blank);
    let outside = f.visual.debug_bounds("hint-other").unwrap().center();
    hover(&mut f, outside);
    assert_hidden(&mut f);
    // Focused hover also retires through the real blur subscription.
    f.visual
        .update(|window, cx| f.graph.read(cx).focus.focus(window));
    f.visual.run_until_parked();
    hover(&mut f, blank);
    f.visual.update(|window, _| window.blur());
    f.visual.run_until_parked();
    assert_hidden(&mut f);
    hover(&mut f, blank);
    f.graph
        .update(&mut f.visual, |graph, cx| graph.set_editable(false, cx));
    f.visual.run_until_parked();
    assert_hidden(&mut f);
    f.graph.update(&mut f.visual, |graph, cx| {
        graph.set_editable(true, cx);
        graph.set_creation_enabled(false);
        cx.notify();
    });
    hover(&mut f, blank);
    assert_hidden(&mut f);
}

#[gpui::test]
fn model_redraws_preserve_hover_but_new_node_resolver_and_axis_changes_revalidate_it(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, true);
    let blank = position(&f, 0.2, 0.8);
    hover(&mut f, blank);
    f.graph.update(&mut f.visual, |graph, cx| {
        let mut model = graph.model.clone();
        model.analyzer = Some(vec![[0.0, 0.2], [1.0, 0.7]]);
        graph.set_model(model, STYLE, cx).unwrap();
        graph.set_creation_hint_resolver(Some(resolve), cx);
    });
    f.visual.run_until_parked();
    assert!(
        f.visual
            .debug_bounds("hint-graph-creation-hint-rise")
            .is_some(),
        "analyzer redraw must not blink a stationary hint"
    );
    f.graph.update(&mut f.visual, |graph, cx| {
        let mut model = graph.model.clone();
        model.nodes[0].position = [0.2, 0.8];
        graph.set_model(model, STYLE, cx).unwrap();
    });
    f.visual.run_until_parked();
    assert_hidden(&mut f);
    let elsewhere = position(&f, 0.8, 0.8);
    hover(&mut f, elsewhere);
    f.graph.update(&mut f.visual, |graph, cx| {
        graph.set_creation_hint_resolver(None, cx)
    });
    f.visual.run_until_parked();
    assert_hidden(&mut f);
    f.visual.simulate_click(elsewhere, Modifiers::default());
    assert!(
        f.events
            .borrow()
            .iter()
            .any(|event| event.phase == ResponseGraphPhase::Create),
        "hint opt-out preserves legacy creation"
    );
    f.graph.update(&mut f.visual, |graph, cx| {
        graph.cancel(cx);
        graph.set_creation_hint_resolver(Some(resolve), cx);
        graph
            .set_axes(Some(ResponseGraphAxes::default()), cx)
            .unwrap();
    });
    let margin = f.graph.read_with(&f.visual, |graph, _| {
        graph.bounds.unwrap().origin + point(px(2.0), px(2.0))
    });
    hover(&mut f, margin);
    assert_hidden(&mut f);
}

#[gpui::test]
fn invalid_paths_are_omitted_and_wheel_capture_cannot_reveal_a_hint(cx: &mut TestAppContext) {
    let mut f = fixture(cx, true);
    f.graph.update(&mut f.visual, |graph, cx| {
        graph.set_creation_hint_resolver(Some(invalid), cx)
    });
    let blank = position(&f, 0.2, 0.8);
    hover(&mut f, blank);
    assert_hidden(&mut f);
    assert_eq!(
        f.visual.rendered_cursor_style(),
        Some(CursorStyle::PointingHand)
    );
    f.graph.update(&mut f.visual, |graph, cx| {
        graph.set_creation_hint_resolver(Some(resolve), cx);
        graph.set_scroll_nodes(vec![3], cx).unwrap();
    });
    let node = position(&f, 0.5, 0.5);
    f.visual.simulate_event(gpui::ScrollWheelEvent {
        position: node,
        delta: gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        touch_phase: gpui::TouchPhase::Started,
        ..Default::default()
    });
    hover(&mut f, blank);
    assert_hidden(&mut f);
}

#[test]
fn hint_validation_bounds_static_paths_and_debug_identifiers() {
    assert!(RISE.valid());
    for hint in [
        ResponseGraphCreationHint { id: "", ..RISE },
        ResponseGraphCreationHint {
            id: "bad/id",
            ..RISE
        },
        ResponseGraphCreationHint {
            id: "a",
            points: &[[0.0, 0.0]],
        },
        ResponseGraphCreationHint {
            id: "a",
            points: &[[0.0, 0.0]; 17],
        },
        ResponseGraphCreationHint {
            id: "a",
            points: &[[0.0, 0.0], [1.0, 1.01]],
        },
        invalid([0.0, 0.0]),
    ] {
        assert!(!hint.valid());
    }
}
