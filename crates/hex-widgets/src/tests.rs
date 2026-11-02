use crate::{
    ControlStyle, DialLayout, NumericControl, NumericControlConfig, NumericControlKind,
    NumericControlPresentation, NumericScale, NumericSpec, ResponseGraph, Selector, Toggle,
};
use gpui::{
    AppContext, Context, Empty, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    Modifiers, MouseButton, MouseDownEvent, ParentElement, Render, StatefulInteractiveElement,
    Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub(super) const STYLE: ControlStyle = ControlStyle {
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
    dial_rail: 0x000000,
    hover: 0x404040,
    selection: 0x3350a0,
    invalid: 0xff554c,
    font_size: 10.0,
    text_case: crate::TextCase::Preserve,
};

#[test]
fn numeric_control_config_rejects_values_that_would_fail_during_entity_creation() {
    let spec = NumericSpec::new(-1.0, 1.0, 0.0, NumericScale::Bipolar, 2, None::<String>)
        .expect("valid spec");
    assert!(
        NumericControlConfig::new(
            "invalid-value",
            "gain",
            NumericControlKind::HorizontalSlider,
            spec.clone(),
            f64::NAN,
            STYLE,
        )
        .is_err()
    );
    let invalid_style = ControlStyle {
        font_size: f32::NAN,
        ..STYLE
    };
    assert!(
        NumericControlConfig::new(
            "invalid-style",
            "gain",
            NumericControlKind::HorizontalSlider,
            spec,
            0.0,
            invalid_style,
        )
        .is_err()
    );
}

#[test]
fn discrete_control_configs_validate_before_entity_factories() {
    assert!(
        crate::ToggleConfig::new(
            "toggle",
            "mono",
            false,
            ControlStyle {
                font_size: f32::INFINITY,
                ..STYLE
            },
        )
        .is_err()
    );
    assert!(crate::SelectorConfig::new("empty", Vec::<String>::new(), 0, STYLE).is_err());
    assert!(crate::SelectorConfig::new("bad-index", ["one", "two"], 2, STYLE).is_err());
}

struct TestRoot {
    control: Entity<NumericControl>,
    outside_focus: FocusHandle,
}

struct DiscreteTestRoot {
    toggle: Entity<Toggle>,
    selector: Entity<Selector>,
    key_events: Rc<Cell<usize>>,
}

struct ToggleTraversalRoot {
    first: Entity<Toggle>,
    middle: Entity<Toggle>,
    last: Entity<Toggle>,
}

struct TabAvailabilityRoot {
    toggle: Entity<Toggle>,
    selector: Entity<Selector>,
    numeric: Entity<NumericControl>,
    graph: Entity<ResponseGraph>,
}

impl Render for TabAvailabilityRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.toggle.clone())
            .child(self.selector.clone())
            .child(self.numeric.clone())
            .child(self.graph.clone())
    }
}

impl Render for ToggleTraversalRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    cx.stop_propagation();
                    if event.keystroke.modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                }
            }))
            .child(self.first.clone())
            .child(self.middle.clone())
            .child(self.last.clone())
    }
}

struct ClippedSelectorRoot {
    selector: Entity<Selector>,
    underlying_clicks: Rc<Cell<usize>>,
    outside_focus: FocusHandle,
    top_offset: f32,
}

impl Render for ClippedSelectorRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let underlying_clicks = self.underlying_clicks.clone();
        let outside_focus = self.outside_focus.clone();
        div()
            .size_full()
            .relative()
            .flex()
            .child(
                div()
                    .id("selector-clipped-parent")
                    .debug_selector(|| "selector-clipped-parent".to_owned())
                    .relative()
                    .mt(px(self.top_offset))
                    .w(px(140.0))
                    .h(px(18.0))
                    .overflow_hidden()
                    .child(self.selector.clone())
                    .child(
                        div()
                            .id("selector-underlying-control")
                            .absolute()
                            .top(px(18.0))
                            .left(px(0.0))
                            .w(px(140.0))
                            .h(px(20.0))
                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                underlying_clicks.set(underlying_clicks.get() + 1);
                            }),
                    ),
            )
            .child(
                div()
                    .id("selector-outside-focus")
                    .debug_selector(|| "selector-outside-focus".to_owned())
                    .w(px(50.0))
                    .h(px(24.0))
                    .track_focus(&self.outside_focus)
                    .on_mouse_down(MouseButton::Left, move |_, window, _| {
                        outside_focus.focus(window);
                    }),
            )
    }
}

impl Render for DiscreteTestRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .on_key_down(cx.listener(|root, _, _, _| {
                root.key_events.set(root.key_events.get() + 1);
            }))
            .child(self.toggle.clone())
            .child(self.selector.clone())
    }
}

impl Render for TestRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("test-root")
            .debug_selector(|| "test-root".to_owned())
            .size_full()
            .child(div().w(px(300.0)).child(self.control.clone()))
            .child(
                div()
                    .id("outside-focus")
                    .debug_selector(|| "outside-focus".to_owned())
                    .w(px(50.0))
                    .h(px(24.0))
                    .track_focus(&self.outside_focus)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|root, _, window, _| {
                            root.outside_focus.focus(window);
                        }),
                    ),
            )
    }
}

pub(crate) fn open_numeric_control(
    cx: &mut TestAppContext,
    kind: NumericControlKind,
) -> (gpui::AnyWindowHandle, Entity<NumericControl>) {
    let spec = NumericSpec::new(
        -1.0,
        1.0,
        0.0,
        NumericScale::Bipolar,
        2,
        None::<std::sync::Arc<str>>,
    )
    .expect("numeric spec");
    open_numeric_control_with_spec(cx, kind, spec, 0.0)
}

fn open_numeric_control_with_spec(
    cx: &mut TestAppContext,
    kind: NumericControlKind,
    spec: NumericSpec,
    value: f64,
) -> (gpui::AnyWindowHandle, Entity<NumericControl>) {
    open_numeric_control_appearance(cx, kind, spec, value, kind.into())
}

fn open_numeric_control_appearance(
    cx: &mut TestAppContext,
    kind: NumericControlKind,
    spec: NumericSpec,
    value: f64,
    appearance: NumericControlPresentation,
) -> (gpui::AnyWindowHandle, Entity<NumericControl>) {
    open_numeric_control_layout(cx, kind, spec, value, appearance, DialLayout::ReadoutGap)
}

fn open_numeric_control_layout(
    cx: &mut TestAppContext,
    kind: NumericControlKind,
    spec: NumericSpec,
    value: f64,
    appearance: NumericControlPresentation,
    layout: DialLayout,
) -> (gpui::AnyWindowHandle, Entity<NumericControl>) {
    cx.update(crate::bind_numeric_control_keys);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let control = cx
        .update_window(handle, |_, window, cx| {
            let control = cx.new(|cx| {
                let config =
                    NumericControlConfig::new("test-control", "gain", kind, spec, value, STYLE)
                        .expect("numeric control")
                        .with_presentation(appearance)
                        .with_dial_layout(layout);
                NumericControl::from_config(cx, config)
            });
            let root_control = control.clone();
            window.replace_root(cx, |_, cx| TestRoot {
                control: root_control,
                outside_focus: cx.focus_handle(),
            });
            control
        })
        .expect("test window");
    (handle, control)
}

fn open_micro_dial(cx: &mut TestAppContext) -> gpui::AnyWindowHandle {
    let spec = NumericSpec::new(
        0.0,
        1.0,
        0.5,
        NumericScale::Linear,
        2,
        None::<std::sync::Arc<str>>,
    )
    .expect("valid MicroDial domain");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    cx.update_window(handle, |_, window, cx| {
        let control = cx.new(|cx| {
            let config = NumericControlConfig::new(
                "test-control",
                "amount",
                NumericControlKind::Knob,
                spec,
                0.5,
                STYLE,
            )
            .expect("valid MicroDial config")
            .with_presentation(NumericControlPresentation::MicroDial);
            NumericControl::from_config(cx, config)
        });
        window.replace_root(cx, |_, cx| TestRoot {
            control,
            outside_focus: cx.focus_handle(),
        });
    })
    .expect("MicroDial test window");
    handle
}

#[gpui::test]
fn micro_dial_keeps_compact_bounds_and_shared_numeric_gesture(cx: &mut TestAppContext) {
    let handle = open_micro_dial(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(260.0), px(100.0)));
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    let dial = visual
        .debug_bounds("test-control")
        .expect("MicroDial bounds");
    assert_eq!(dial.size.width, px(crate::MICRO_DIAL_WIDTH));
    assert_eq!(dial.size.height, px(40.0));
    let ring = visual
        .debug_bounds("test-control-knob")
        .expect("MicroDial ring");
    assert_eq!(ring.size.width, px(18.0));
    assert_eq!(ring.size.height, px(18.0));
    let readout = visual
        .debug_bounds("test-control-value")
        .expect("MicroDial readout");
    assert_eq!(readout.size.width, px(crate::MICRO_DIAL_WIDTH - 13.0));
    assert_eq!(readout.size.height, px(12.0));
    let label = visual
        .debug_bounds("test-control-label")
        .expect("full amount label");
    assert_eq!(label.size.width, px(crate::MICRO_DIAL_WIDTH - 4.0));
    assert!(label.bottom() <= ring.top());
    assert!(readout.left() > ring.left());
}

fn open_discrete_controls(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    Entity<Toggle>,
    Entity<Selector>,
    Rc<Cell<usize>>,
) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let (toggle, selector, key_events) = cx
        .update_window(handle, |_, window, cx| {
            let toggle =
                cx.new(|cx| Toggle::new(cx, "test-toggle", "mono", false, STYLE).expect("toggle"));
            let selector = cx.new(|cx| {
                Selector::new(cx, "test-selector", ["one", "two"], 0, STYLE).expect("selector")
            });
            let key_events = Rc::new(Cell::new(0));
            let root_toggle = toggle.clone();
            let root_selector = selector.clone();
            let root_key_events = key_events.clone();
            window.replace_root(cx, |_, _| DiscreteTestRoot {
                toggle: root_toggle,
                selector: root_selector,
                key_events: root_key_events,
            });
            (toggle, selector, key_events)
        })
        .expect("discrete test window");
    (handle, toggle, selector, key_events)
}

#[gpui::test]
fn tab_navigation_skips_runtime_disabled_toggle_and_restores_owner_opt_in(cx: &mut TestAppContext) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let (first, middle, last) = cx
        .update_window(handle, |_, window, cx| {
            let first = cx
                .new(|cx| Toggle::new(cx, "first-toggle", "first", false, STYLE).expect("toggle"));
            let middle = cx.new(|cx| {
                Toggle::new(cx, "middle-toggle", "middle", false, STYLE).expect("toggle")
            });
            let last =
                cx.new(|cx| Toggle::new(cx, "last-toggle", "last", false, STYLE).expect("toggle"));
            for toggle in [&first, &middle, &last] {
                toggle.update(cx, |toggle, cx| toggle.set_tab_navigation(true, cx));
            }
            window.replace_root(cx, |_, _| ToggleTraversalRoot {
                first: first.clone(),
                middle: middle.clone(),
                last: last.clone(),
            });
            (first, middle, last)
        })
        .expect("toggle traversal window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(240.0), px(100.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    let first_focus = first.read_with(cx, |toggle, app| toggle.focus_handle(app));
    visual.update(|window, _| window.focus(&first_focus));
    visual.simulate_keystrokes("space");
    assert!(first.read_with(cx, |toggle, _| toggle.checked()));

    middle.update(cx, |toggle, cx| toggle.set_enabled(false, cx));
    visual.simulate_keystrokes("tab");
    visual.simulate_keystrokes("space");
    assert!(last.read_with(cx, |toggle, _| toggle.checked()));
    assert!(!middle.read_with(cx, |toggle, _| toggle.checked()));

    visual.simulate_keystrokes("shift-tab");
    visual.simulate_keystrokes("space");
    assert!(!first.read_with(cx, |toggle, _| toggle.checked()));
    assert!(!middle.read_with(cx, |toggle, _| toggle.checked()));

    middle.update(cx, |toggle, cx| toggle.set_enabled(true, cx));
    visual.simulate_keystrokes("tab");
    visual.simulate_keystrokes("space");
    assert!(middle.read_with(cx, |toggle, _| toggle.checked()));
    visual.simulate_keystrokes("tab");
    visual.simulate_keystrokes("space");
    assert!(!last.read_with(cx, |toggle, _| toggle.checked()));

    middle.update(cx, |toggle, cx| {
        toggle.set_enabled(false, cx);
        toggle.set_tab_navigation(false, cx);
        toggle.set_enabled(true, cx);
    });
    visual.simulate_keystrokes("shift-tab");
    visual.simulate_keystrokes("space");
    assert!(first.read_with(cx, |toggle, _| toggle.checked()));
    assert!(middle.read_with(cx, |toggle, _| toggle.checked()));
}

fn is_tab_stop<T: Focusable>(entity: &Entity<T>, cx: &TestAppContext) -> bool {
    entity.read_with(cx, |control, app| control.focus_handle(app).tab_stop)
}

#[gpui::test]
fn availability_changes_recompute_all_widget_tab_stops(cx: &mut TestAppContext) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let spec = NumericSpec::new(-1.0, 1.0, 0.0, NumericScale::Bipolar, 2, None::<String>)
        .expect("numeric spec");
    let (toggle, selector, numeric, graph) = cx
        .update_window(handle, |_, window, cx| {
            let toggle = cx.new(|cx| {
                Toggle::new(cx, "availability-toggle", "toggle", false, STYLE).expect("toggle")
            });
            let selector = cx.new(|cx| {
                Selector::new(cx, "availability-selector", ["one", "two"], 0, STYLE)
                    .expect("selector")
            });
            let numeric = cx.new(|cx| {
                NumericControl::new(
                    cx,
                    "availability-numeric",
                    "value",
                    NumericControlKind::HorizontalSlider,
                    spec,
                    0.0,
                    STYLE,
                )
                .expect("numeric control")
            });
            let graph = cx.new(|cx| {
                let mut graph = ResponseGraph::new(cx, "availability-graph", STYLE);
                graph.set_editable(true, cx);
                graph
            });
            toggle.update(cx, |control, cx| control.set_tab_navigation(true, cx));
            selector.update(cx, |control, cx| control.set_tab_navigation(true, cx));
            numeric.update(cx, |control, cx| control.set_tab_navigation(true, cx));
            graph.update(cx, |control, cx| control.set_tab_navigation(true, cx));
            window.replace_root(cx, |_, _| TabAvailabilityRoot {
                toggle: toggle.clone(),
                selector: selector.clone(),
                numeric: numeric.clone(),
                graph: graph.clone(),
            });
            (toggle, selector, numeric, graph)
        })
        .expect("availability test window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    for tab_stop in [
        is_tab_stop(&toggle, cx),
        is_tab_stop(&selector, cx),
        is_tab_stop(&numeric, cx),
        is_tab_stop(&graph, cx),
    ] {
        assert!(tab_stop, "opted-in available widgets are tab stops");
    }

    toggle.update(cx, |control, cx| control.set_enabled(false, cx));
    selector.update(cx, |control, cx| control.set_enabled(false, cx));
    numeric.update(cx, |control, cx| control.set_enabled(false, cx));
    graph.update(cx, |control, cx| control.set_editable(false, cx));
    for tab_stop in [
        is_tab_stop(&toggle, cx),
        is_tab_stop(&selector, cx),
        is_tab_stop(&numeric, cx),
        is_tab_stop(&graph, cx),
    ] {
        assert!(!tab_stop, "unavailable widgets leave the tab order");
    }

    toggle.update(cx, |control, cx| control.set_enabled(true, cx));
    selector.update(cx, |control, cx| control.set_enabled(true, cx));
    numeric.update(cx, |control, cx| control.set_enabled(true, cx));
    graph.update(cx, |control, cx| control.set_editable(true, cx));
    for tab_stop in [
        is_tab_stop(&toggle, cx),
        is_tab_stop(&selector, cx),
        is_tab_stop(&numeric, cx),
        is_tab_stop(&graph, cx),
    ] {
        assert!(tab_stop, "re-enabled widgets restore their owner's opt-in");
    }

    toggle.update(cx, |control, cx| {
        control.set_enabled(false, cx);
        control.set_tab_navigation(false, cx);
        control.set_enabled(true, cx);
    });
    selector.update(cx, |control, cx| {
        control.set_enabled(false, cx);
        control.set_tab_navigation(false, cx);
        control.set_enabled(true, cx);
    });
    numeric.update(cx, |control, cx| {
        control.set_enabled(false, cx);
        control.set_tab_navigation(false, cx);
        control.set_enabled(true, cx);
    });
    graph.update(cx, |control, cx| {
        control.set_editable(false, cx);
        control.set_tab_navigation(false, cx);
        control.set_editable(true, cx);
    });
    for tab_stop in [
        is_tab_stop(&toggle, cx),
        is_tab_stop(&selector, cx),
        is_tab_stop(&numeric, cx),
        is_tab_stop(&graph, cx),
    ] {
        assert!(!tab_stop, "availability restoration respects owner opt-out");
    }
}

fn open_clipped_selector(
    cx: &mut TestAppContext,
    top_offset: f32,
    choice_count: usize,
) -> (gpui::AnyWindowHandle, Entity<Selector>, Rc<Cell<usize>>) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let (selector, underlying_clicks) = cx
        .update_window(handle, |_, window, cx| {
            let selector = cx.new(|cx| {
                let choices = (0..choice_count)
                    .map(|index| format!("choice {index}"))
                    .collect::<Vec<_>>();
                Selector::new(cx, "clipped-selector", choices, 0, STYLE).expect("selector")
            });
            let underlying_clicks = Rc::new(Cell::new(0));
            let outside_focus = cx.focus_handle();
            let root_selector = selector.clone();
            let root_clicks = underlying_clicks.clone();
            window.replace_root(cx, |_, _| ClippedSelectorRoot {
                selector: root_selector,
                underlying_clicks: root_clicks,
                outside_focus,
                top_offset,
            });
            (selector, underlying_clicks)
        })
        .expect("selector overlay test window");
    (handle, selector, underlying_clicks)
}

#[gpui::test]
fn rendered_drag_captures_outside_release_and_emits_one_complete_gesture(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let phases = Rc::new(RefCell::new(Vec::new()));
    let event_phases = phases.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            event_phases.borrow_mut().push(event.phase)
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(180.0)));
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("test-root").is_some(),
        "test root should render"
    );
    let slider = visual
        .debug_bounds("test-control-slider")
        .expect("rendered slider");
    let start = point(
        slider.origin.x + px(10.0),
        slider.origin.y + slider.size.height / 2.0,
    );
    let outside = point(
        slider.origin.x + slider.size.width + px(70.0),
        start.y - px(30.0),
    );
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    control.read_with(cx, |control, _| {
        assert!(
            control.value() > 0.4,
            "drag should update its local value outside bounds"
        );
        assert!(control.is_interacting());
    });
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    control.read_with(cx, |control, _| {
        assert!(!control.is_interacting());
        assert!(control.value() > 0.4);
    });
    assert_eq!(
        phases.borrow().as_slice(),
        [
            crate::NumericEventPhase::Begin,
            crate::NumericEventPhase::Change,
            crate::NumericEventPhase::Commit,
        ]
    );
}

#[gpui::test]
fn rectangular_value_fields_drag_vertically_without_horizontal_adjustment(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let field = visual
        .debug_bounds("test-control-value")
        .expect("rectangular field");
    let initial = control.read_with(cx, |control, _| control.value());
    let start = field.center();
    let sideways = point(start.x + px(12.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(sideways, Some(MouseButton::Left), Modifiers::default());
    control.read_with(cx, |control, _| {
        assert_eq!(
            control.value(),
            initial,
            "horizontal motion does not adjust a value field"
        )
    });
    let up = point(sideways.x, start.y - px(24.0));
    visual.simulate_mouse_move(up, Some(MouseButton::Left), Modifiers::default());
    control.read_with(cx, |control, _| {
        assert!(control.value() > initial, "up increases")
    });
    let down = point(sideways.x, start.y + px(12.0));
    visual.simulate_mouse_move(down, Some(MouseButton::Left), Modifiers::default());
    control.read_with(cx, |control, _| {
        assert!(control.value() < initial, "down decreases")
    });
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    control.read_with(cx, |control, _| assert!(!control.is_interacting()));
}

#[gpui::test]
fn shift_changes_adjustment_sensitivity_without_jumping(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let slider = visual
        .debug_bounds("test-control-slider")
        .expect("rendered slider");
    let start = point(
        slider.origin.x + px(10.0),
        slider.origin.y + slider.size.height / 2.0,
    );
    let normal_position = point(start.x, start.y - px(30.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        normal_position,
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let normal_value = control.read_with(cx, |control, _| control.value());
    assert!(normal_value > 0.49);

    visual.simulate_mouse_move(
        normal_position,
        Some(MouseButton::Left),
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    control.read_with(cx, |control, _| {
        assert_eq!(
            control.value(),
            normal_value,
            "modifier change must re-anchor"
        );
    });

    let fine_position = point(normal_position.x, normal_position.y - px(12.0));
    visual.simulate_mouse_move(
        fine_position,
        Some(MouseButton::Left),
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    control.read_with(cx, |control, _| {
        assert!((control.value() - (normal_value + 0.02)).abs() < 1.0e-6);
    });
    visual.simulate_mouse_up(fine_position, MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn invalid_value_border_paints_the_injected_style_color(cx: &mut TestAppContext) {
    const INVALID: u32 = 0x00ff7f;
    let (handle, control) = open_numeric_control(cx, NumericControlKind::Knob);
    control.update(cx, |control, cx| {
        control
            .set_external_style(
                ControlStyle {
                    invalid: INVALID,
                    ..STYLE
                },
                cx,
            )
            .expect("valid style");
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual
        .debug_bounds("test-control-value")
        .expect("numeric value field")
        .center();
    visual.simulate_click(value, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    control.read_with(cx, |control, _| assert!(control.invalid_text));
    let (border, _, _) = visual
        .rendered_border("test-control-value")
        .expect("value field border");
    assert_eq!(border, gpui::Hsla::from(gpui::rgb(INVALID)));
}

#[gpui::test]
fn rendered_knob_reset_emits_one_complete_default_gesture(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::Knob);
    let phases = Rc::new(RefCell::new(Vec::new()));
    let output = phases.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            output.borrow_mut().push((event.phase, event.value));
        })
    });
    control.update(cx, |control, cx| {
        control
            .set_external_value(0.75, cx)
            .expect("valid external value");
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let knob = visual
        .debug_bounds("test-control-knob")
        .expect("rendered knob");
    let point = point(
        knob.origin.x + knob.size.width / 2.0,
        knob.origin.y + knob.size.height / 2.0,
    );
    visual.simulate_event(MouseDownEvent {
        position: point,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    control.read_with(cx, |control, _| assert_eq!(control.value(), 0.0));
    assert_eq!(
        phases.borrow().as_slice(),
        [
            (crate::NumericEventPhase::Begin, 0.75),
            (crate::NumericEventPhase::Change, 0.0),
            (crate::NumericEventPhase::Commit, 0.0),
        ]
    );

    // Reset is also an explicit way out of a pending typed value, including
    // invalid input and resetting a value that is already at its default.
    let value_bounds = visual
        .debug_bounds("test-control-value")
        .expect("numeric value field");
    let value_point = value_bounds.center();
    for (start, pending_text) in [(0.75, "0.5"), (0.0, "invalid")] {
        control.update(cx, |control, cx| {
            control
                .set_external_value(start, cx)
                .expect("external value");
        });
        visual.simulate_click(value_point, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(pending_text);
        if pending_text == "invalid" {
            visual.simulate_keystrokes("enter");
            control.read_with(cx, |control, _| assert!(control.invalid_text));
        }
        visual.simulate_click(point, Modifiers::default());
        // The first click can commit valid text through the existing focus-blur
        // contract. Invalid text remains pending until the reset discards it.
        let reset_start = control.read_with(cx, |control, _| control.value());
        phases.borrow_mut().clear();
        visual.simulate_event(MouseDownEvent {
            position: point,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        control.read_with(cx, |control, _| {
            assert!(!control.is_editing(), "reset must end pending text editing");
            assert!(!control.invalid_text);
            assert!(
                !control.is_interacting(),
                "reset must release the old press"
            );
            assert_eq!(control.display_text(), "0.00");
            assert_eq!(control.selected, 0..0);
            assert!(control.marked.is_none());
        });
        // Continuing to hold/move after the double-click must not revive the
        // first click's press or open a second gesture.
        let moved = gpui::point(point.x, point.y - px(12.0));
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        let expected = if reset_start == 0.0 {
            Vec::new()
        } else {
            vec![
                (crate::NumericEventPhase::Begin, reset_start),
                (crate::NumericEventPhase::Change, 0.0),
                (crate::NumericEventPhase::Commit, 0.0),
            ]
        };
        assert_eq!(*phases.borrow(), expected);
        control.update(cx, |control, cx| {
            control
                .set_external_value(0.25, cx)
                .expect("external value");
            assert_eq!(control.value(), 0.25);
        });
        visual.simulate_keystrokes("up");
        control.read_with(cx, |control, _| assert!(control.value() > 0.25));
    }
}

#[gpui::test]
fn rendered_decibel_control_steps_resets_types_and_cancels_in_linear_units(
    cx: &mut TestAppContext,
) {
    let spec = NumericSpec::new(
        0.0,
        f64::from(56.234_13_f32),
        1.0,
        NumericScale::LinearAmplitudeDecibels,
        2,
        Some("db"),
    )
    .expect("dB amplitude spec");
    let (handle, control) = open_numeric_control_with_spec(cx, NumericControlKind::Knob, spec, 1.0);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let knob = visual.debug_bounds("test-control-knob").expect("knob");
    let knob_point = point(knob.center().x, knob.center().y);
    visual.simulate_click(knob_point, Modifiers::default());
    visual.simulate_keystrokes("up");
    control.read_with(cx, |control, _| {
        assert!((control.value() - 10.0_f64.powf(1.0 / 20.0)).abs() < 1.0e-12)
    });
    visual.simulate_keystrokes("shift-up");
    control.read_with(cx, |control, _| {
        assert!((control.value() - 10.0_f64.powf(1.1 / 20.0)).abs() < 1.0e-12)
    });

    visual.simulate_event(gpui::MouseDownEvent {
        position: knob_point,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    control.read_with(cx, |control, _| assert_eq!(control.value(), 1.0));

    let value_bounds = visual
        .debug_bounds("test-control-value")
        .expect("numeric value field");
    let value_point = point(value_bounds.center().x, value_bounds.center().y);
    visual.simulate_mouse_down(value_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(value_point, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("6 db");
    visual.simulate_keystrokes("enter");
    let six_db = 10.0_f64.powf(6.0 / 20.0);
    control.read_with(cx, |control, _| {
        assert!(!control.is_editing());
        assert!((control.value() - six_db).abs() < 1.0e-12);
    });

    visual.simulate_mouse_down(value_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(value_point, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("9 db");
    visual.simulate_keystrokes("escape");
    control.read_with(cx, |control, _| {
        assert!(!control.is_editing());
        assert!((control.value() - six_db).abs() < 1.0e-12);
    });
}

#[gpui::test]
fn disabling_an_active_drag_emits_cancel_and_releases_capture(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let phases = Rc::new(RefCell::new(Vec::new()));
    let output = phases.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            output.borrow_mut().push(event.phase)
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let slider = visual
        .debug_bounds("test-control-slider")
        .expect("rendered slider");
    let start = point(
        slider.origin.x + px(10.0),
        slider.origin.y + slider.size.height / 2.0,
    );
    let moved = point(start.x, start.y - px(36.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    control.read_with(cx, |control, _| assert!(control.is_interacting()));
    control.update(cx, |control, cx| control.set_enabled(false, cx));
    control.read_with(cx, |control, _| {
        assert!(!control.is_interacting());
        assert_eq!(control.value(), 0.0);
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    assert_eq!(
        phases.borrow().as_slice(),
        [
            crate::NumericEventPhase::Begin,
            crate::NumericEventPhase::Change,
            crate::NumericEventPhase::Cancel,
        ]
    );
}

#[gpui::test]
fn stationary_value_click_enters_text_edit_without_dragging(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::Knob);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual
        .debug_bounds("test-control-value")
        .expect("numeric value bounds");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    control.read_with(cx, |control, _| {
        assert!(control.is_editing());
        assert_eq!(control.value(), 0.0);
    });
    visual.simulate_input("2");
    visual.simulate_keystrokes("enter");
    control.read_with(cx, |control, _| {
        assert!(control.is_editing(), "invalid typed value stays editable");
        assert_eq!(
            control.value(),
            0.0,
            "invalid input does not change the value"
        );
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.5");
    visual.simulate_keystrokes("enter");
    control.read_with(cx, |control, _| {
        assert!(!control.is_editing());
        assert!((control.value() - 0.5).abs() < f64::EPSILON);
    });
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_input("0.25");
    let outside = visual
        .debug_bounds("outside-focus")
        .expect("focusable outside target");
    let outside = point(
        outside.origin.x + outside.size.width / 2.0,
        outside.origin.y + outside.size.height / 2.0,
    );
    visual.simulate_mouse_down(outside, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let outside_has_focus = visual.update(|window, app| {
        window
            .root::<TestRoot>()
            .flatten()
            .is_some_and(|root| root.read(app).outside_focus.is_focused(window))
    });
    assert!(outside_has_focus, "outside target should receive focus");
    control.read_with(cx, |control, _| {
        assert!(!control.is_editing());
        assert!((control.value() - 0.25).abs() < f64::EPSILON);
    });
}

#[gpui::test]
fn numeric_arrow_keys_step_idle_and_typed_values_across_presentations(cx: &mut TestAppContext) {
    for presentation in [
        NumericControlPresentation::Knob,
        NumericControlPresentation::CompactDial,
        NumericControlPresentation::MicroDial,
        NumericControlPresentation::WideMicroDial,
        NumericControlPresentation::HorizontalSlider,
        NumericControlPresentation::CompactSlider,
        NumericControlPresentation::LabeledSlider,
        NumericControlPresentation::FilledSlider,
        NumericControlPresentation::InlineValue,
        NumericControlPresentation::VerticalSlider,
    ] {
        let spec =
            NumericSpec::new(-100.0, 100.0, 0.0, NumericScale::Linear, 2, None::<&str>).unwrap();
        let (handle, control) =
            open_numeric_control_appearance(cx, presentation.kind(), spec, 0.0, presentation);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let value = visual
            .debug_bounds("test-control-value")
            .expect("numeric text input");
        visual.simulate_click(value.center(), Modifiers::default());
        visual.simulate_keystrokes("escape up");
        assert_eq!(
            control.read_with(cx, |c, _| c.value()),
            1.0,
            "{presentation:?}: normal step"
        );
        visual.simulate_keystrokes("shift-down");
        assert_eq!(
            control.read_with(cx, |c, _| c.value()),
            0.9,
            "{presentation:?}: fine step"
        );

        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let _subscription = cx.update(|app| {
            app.subscribe(&control, move |_, event, _| {
                observed.borrow_mut().push((event.phase, event.value))
            })
        });
        visual.simulate_click(value.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("2.5");
        visual.simulate_keystrokes("up shift-down");
        control.read_with(cx, |c, _| {
            assert!(c.is_editing());
            assert_eq!(
                c.display_text(),
                "3.40",
                "{presentation:?}: typed draft steps"
            );
            assert_eq!(c.value(), 0.9, "draft stays local");
        });
        assert!(
            events.borrow().is_empty(),
            "draft arrows do not create history"
        );
        visual.simulate_keystrokes("enter");
        assert_eq!(control.read_with(cx, |c, _| c.value()), 3.4);
        assert_eq!(
            events.borrow().as_slice(),
            &[
                (crate::NumericEventPhase::Begin, 0.9),
                (crate::NumericEventPhase::Change, 3.4),
                (crate::NumericEventPhase::Commit, 3.4),
            ]
        );
    }
}

#[gpui::test]
fn numeric_arrow_keys_preserve_invalid_composition_disabled_and_cancelled_drafts(
    cx: &mut TestAppContext,
) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual.debug_bounds("test-control-value").unwrap();
    visual.simulate_click(value.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.simulate_keystrokes("up shift-down");
    control.read_with(cx, |c, _| assert_eq!(c.display_text(), "invalid"));
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.25");
    visual.simulate_keystrokes("shift-up");
    control.read_with(cx, |c, _| assert_eq!(c.display_text(), "0.35"));
    visual.update(|window, app| {
        control.update(app, |c, cx| {
            gpui::EntityInputHandler::replace_and_mark_text_in_range(
                c,
                Some(0..4),
                "0.5",
                None,
                window,
                cx,
            );
        })
    });
    visual.simulate_keystrokes("up shift-down");
    control.read_with(cx, |c, _| {
        assert_eq!(c.display_text(), "0.5");
        assert!(c.marked.is_some(), "composition retains input custody");
        assert_eq!(c.value(), 0.0);
    });
    visual.simulate_keystrokes("escape");
    assert_eq!(control.read_with(cx, |c, _| c.value()), 0.0);
    control.update(cx, |c, cx| c.set_enabled(false, cx));
    visual.simulate_keystrokes("up shift-up");
    assert_eq!(control.read_with(cx, |c, _| c.value()), 0.0);
}

#[gpui::test]
fn owner_finish_edit_commits_valid_text_once_and_preserves_invalid_text(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let phases = Rc::new(RefCell::new(Vec::new()));
    let output = phases.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            output.borrow_mut().push(event.phase)
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual
        .debug_bounds("test-control-value")
        .expect("value field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.5");
    control.update(cx, |control, cx| {
        control.finish_edit(cx).expect("valid draft commits");
    });
    assert_eq!(
        phases.borrow().as_slice(),
        [
            crate::NumericEventPhase::Begin,
            crate::NumericEventPhase::Change,
            crate::NumericEventPhase::Commit,
        ]
    );
    control.read_with(cx, |control, _| {
        assert!(!control.is_editing());
        assert_eq!(control.value(), 0.5);
    });

    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    let result = control.update(cx, |control, cx| control.finish_edit(cx));
    assert!(result.is_err());
    control.read_with(cx, |control, _| {
        assert!(control.is_editing());
        assert!(control.invalid_text);
        assert_eq!(control.text, "invalid");
        assert_eq!(control.value(), 0.5);
    });
    assert_eq!(phases.borrow().len(), 3, "invalid text emits no edit");
}

#[gpui::test]
fn owner_finalization_returns_typed_value_without_emitting_protocol_events(
    cx: &mut TestAppContext,
) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let phases = Rc::new(RefCell::new(Vec::new()));
    let output = phases.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            output.borrow_mut().push(event.phase)
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual
        .debug_bounds("test-control-value")
        .expect("value field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    let passive = control
        .update(cx, |control, cx| control.prepare_finish_edit_for_owner(cx))
        .unwrap()
        .unwrap();
    assert!(
        !passive.explicit_edit,
        "passive owner finalization is not a touch"
    );
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0");
    let same = control
        .update(cx, |control, cx| control.prepare_finish_edit_for_owner(cx))
        .unwrap()
        .unwrap();
    assert_eq!(same.previous, same.value);
    assert!(
        same.explicit_edit,
        "modified equal-value text retains edit intent"
    );
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.625");
    let finalized = control.update(cx, |control, cx| control.prepare_finish_edit_for_owner(cx));
    let finalized = finalized
        .expect("valid draft prepares")
        .expect("draft value");
    assert_eq!((finalized.previous, finalized.value), (0.0, 0.625));
    assert!(finalized.explicit_edit);
    assert!(
        phases.borrow().is_empty(),
        "owner emits protocol exactly once"
    );
    control.update(cx, |control, cx| {
        control.accept_finish_edit_for_owner(finalized, cx)
    });
    control.read_with(cx, |control, _| {
        assert!(!control.is_editing());
        assert_eq!(control.value(), 0.625);
    });
}

#[gpui::test]
fn oversized_numeric_input_is_refused_without_replacing_current_text(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::Knob);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual
        .debug_bounds("test-control-value")
        .expect("numeric value bounds");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let oversized = "9".repeat(300);
    visual.update(|window, app| {
        control.update(app, |control, cx| {
            gpui::EntityInputHandler::replace_text_in_range(control, None, &oversized, window, cx);
            gpui::EntityInputHandler::replace_and_mark_text_in_range(
                control, None, &oversized, None, window, cx,
            );
        });
    });
    control.read_with(cx, |control, _| {
        assert!(control.is_editing());
        assert!(control.invalid_text);
        assert_eq!(control.text, "0.00");
        assert_eq!(control.value(), 0.0);
    });
}

#[gpui::test]
fn marked_input_ranges_round_trip_through_utf16_for_multibyte_text(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::Knob);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual
        .debug_bounds("test-control-value")
        .expect("numeric value bounds");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    visual.update(|window, app| {
        control.update(app, |control, cx| {
            gpui::EntityInputHandler::replace_and_mark_text_in_range(
                control,
                Some(1..3),
                "2µ",
                Some(2..2),
                window,
                cx,
            );
        });
    });
    control.read_with(cx, |control, _| assert_eq!(control.text, "02µ0"));
    let (marked, selected) = visual.update(|window, app| {
        control.update(app, |control, cx| {
            let marked = gpui::EntityInputHandler::marked_text_range(control, window, cx);
            let selected =
                gpui::EntityInputHandler::selected_text_range(control, false, window, cx)
                    .map(|selection| selection.range);
            (marked, selected)
        })
    });
    assert_eq!(marked, Some(1..3));
    assert_eq!(selected, Some(3..3));
}

#[gpui::test]
fn external_value_does_not_replace_an_active_draft_and_escape_cancels(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let slider = visual
        .debug_bounds("test-control-slider")
        .expect("rendered slider");
    let start = point(
        slider.origin.x + px(10.0),
        slider.origin.y + slider.size.height / 2.0,
    );
    let drag = point(start.x + px(45.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(drag, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let local = control.read_with(cx, |control, _| control.value());
    control.update(cx, |control, cx| {
        control
            .set_external_value(-0.75, cx)
            .expect("valid external value");
        assert_eq!(control.value(), local);
    });
    visual.simulate_keystrokes("escape");
    control.read_with(cx, |control, _| {
        assert_eq!(control.value(), 0.0);
    });
}

#[gpui::test]
fn rendered_toggle_and_selector_emit_updated_states(cx: &mut TestAppContext) {
    let (handle, toggle, selector, key_events) = open_discrete_controls(cx);
    let toggle_events = Rc::new(RefCell::new(Vec::new()));
    let selector_events = Rc::new(RefCell::new(Vec::new()));
    let toggle_output = toggle_events.clone();
    let selector_output = selector_events.clone();
    let (_toggle_subscription, _selector_subscription) = cx.update(|app| {
        (
            app.subscribe(&toggle, move |_, event, _| {
                toggle_output.borrow_mut().push(event.checked)
            }),
            app.subscribe(&selector, move |_, event, _| {
                selector_output
                    .borrow_mut()
                    .push((event.index, event.label.clone()))
            }),
        )
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let toggle_bounds = visual.debug_bounds("test-toggle").expect("toggle bounds");
    let toggle_point = point(
        toggle_bounds.origin.x + toggle_bounds.size.width / 2.0,
        toggle_bounds.origin.y + toggle_bounds.size.height / 2.0,
    );
    visual.simulate_mouse_down(toggle_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_point, MouseButton::Left, Modifiers::default());
    toggle.read_with(cx, |toggle, _| assert!(toggle.checked()));
    assert_eq!(toggle_events.borrow().as_slice(), [true]);
    visual.simulate_keystrokes("space enter");
    visual.run_until_parked();
    toggle.read_with(cx, |toggle, _| assert!(toggle.checked()));
    assert_eq!(toggle_events.borrow().as_slice(), [true, false, true]);
    assert_eq!(key_events.get(), 0, "handled toggle keys must not bubble");

    let selector_bounds = visual
        .debug_bounds("test-selector")
        .expect("selector bounds");
    let selector_point = point(
        selector_bounds.origin.x + selector_bounds.size.width / 2.0,
        selector_bounds.origin.y + selector_bounds.size.height / 2.0,
    );
    visual.simulate_mouse_down(selector_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(selector_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds("test-selector-option-1")
        .expect("opened second option");
    let option_point = point(
        option.origin.x + option.size.width / 2.0,
        option.origin.y + option.size.height / 2.0,
    );
    visual.simulate_mouse_down(option_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(option_point, MouseButton::Left, Modifiers::default());
    selector.read_with(cx, |selector, _| assert_eq!(selector.selected(), 1));
    assert_eq!(selector_events.borrow()[0], (1, "two".into()));
    visual.simulate_mouse_down(
        point(px(390.0), px(170.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(px(390.0), px(170.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    selector.read_with(cx, |selector, _| assert!(!selector.is_open()));
}

#[gpui::test]
fn selector_popup_is_foreground_and_hit_testable_beyond_clipped_parent(cx: &mut TestAppContext) {
    let (handle, selector, underlying_clicks) = open_clipped_selector(cx, 0.0, 2);
    let changed = Rc::new(RefCell::new(Vec::new()));
    let changed_output = changed.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&selector, move |_, event, _| {
            changed_output.borrow_mut().push(event.index);
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(240.0), px(120.0)));
    visual.run_until_parked();
    let field_before = visual
        .debug_bounds("clipped-selector")
        .expect("selector field bounds");
    assert_eq!(field_before.size.height, px(22.0));

    let field_point = point(
        field_before.origin.x + field_before.size.width / 2.0,
        field_before.origin.y + field_before.size.height / 2.0,
    );
    visual.simulate_mouse_down(field_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    let parent = visual
        .debug_bounds("selector-clipped-parent")
        .expect("clipped selector parent");
    let popup = visual
        .debug_bounds("clipped-selector-popup")
        .expect("foreground selector popup");
    assert_eq!(
        visual
            .debug_bounds("clipped-selector")
            .expect("selector field remains mounted")
            .size,
        field_before.size,
        "opening the popup must not change the field footprint"
    );
    assert_eq!(popup.size.width, field_before.size.width);
    assert!(
        popup.bottom() > parent.bottom(),
        "popup should escape parent clip"
    );
    let option = visual
        .debug_bounds("clipped-selector-option-1")
        .expect("second option is laid out outside the clipping parent");
    let option_point = point(
        option.origin.x + option.size.width / 2.0,
        option.origin.y + option.size.height / 2.0,
    );
    visual.simulate_mouse_down(option_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(option_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    selector.read_with(cx, |selector, _| assert_eq!(selector.selected(), 1));
    assert_eq!(changed.borrow().as_slice(), [1]);
    assert_eq!(
        underlying_clicks.get(),
        0,
        "popup clicks must not leak underneath"
    );

    selector.update(cx, |selector, cx| {
        selector.set_selected(0, cx).expect("valid selection")
    });
    let field = visual
        .debug_bounds("clipped-selector")
        .expect("selector field after selection");
    let field_point = point(
        field.origin.x + field.size.width / 2.0,
        field.origin.y + field.size.height / 2.0,
    );
    visual.simulate_mouse_down(field_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field_point, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    selector.read_with(cx, |selector, _| assert!(!selector.is_open()));

    visual.simulate_mouse_down(field_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let outside = visual
        .debug_bounds("selector-outside-focus")
        .expect("outside focus target");
    let outside_point = point(
        outside.origin.x + outside.size.width / 2.0,
        outside.origin.y + outside.size.height / 2.0,
    );
    visual.simulate_mouse_down(outside_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(outside_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    selector.read_with(cx, |selector, _| assert!(!selector.is_open()));

    selector.update(cx, |selector, cx| {
        selector.set_enabled(true, cx);
        selector.set_selected(0, cx).expect("valid selection");
    });
    let field = visual
        .debug_bounds("clipped-selector")
        .expect("selector field for disabled close");
    let field_point = point(
        field.origin.x + field.size.width / 2.0,
        field.origin.y + field.size.height / 2.0,
    );
    visual.simulate_mouse_down(field_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    selector.update(cx, |selector, cx| selector.set_enabled(false, cx));
    visual.run_until_parked();
    selector.read_with(cx, |selector, _| assert!(!selector.is_open()));
}

#[gpui::test]
fn selector_popup_flips_above_near_window_bottom(cx: &mut TestAppContext) {
    let (handle, _, _) = open_clipped_selector(cx, 75.0, 2);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(240.0), px(100.0)));
    visual.run_until_parked();
    let field = visual
        .debug_bounds("clipped-selector")
        .expect("selector field bounds");
    let field_point = point(
        field.origin.x + field.size.width / 2.0,
        field.origin.y + field.size.height / 2.0,
    );
    visual.simulate_mouse_down(field_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let popup = visual
        .debug_bounds("clipped-selector-popup")
        .expect("selector popup bounds");
    assert!(
        popup.origin.y < field.origin.y,
        "popup should flip above the field"
    );
    assert!(
        popup.bottom() <= px(100.0),
        "popup should remain inside the window"
    );
}

#[gpui::test]
fn selector_popup_caps_many_choices_to_window_height(cx: &mut TestAppContext) {
    let (handle, selector, _) = open_clipped_selector(cx, 0.0, 128);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(240.0), px(80.0)));
    visual.run_until_parked();
    let field = visual
        .debug_bounds("clipped-selector")
        .expect("selector field bounds");
    let field_point = point(
        field.origin.x + field.size.width / 2.0,
        field.origin.y + field.size.height / 2.0,
    );
    visual.simulate_mouse_down(field_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let popup = visual
        .debug_bounds("clipped-selector-popup")
        .expect("selector popup bounds");
    assert!(
        popup.size.height <= px(72.0),
        "popup should remain scrollable within the window"
    );
    selector.read_with(cx, |selector, _| assert!(selector.is_open()));
}

#[gpui::test]
fn value_cursor_preserves_click_editing_and_restores_after_outside_drag(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let start = visual
        .debug_bounds("test-control-value")
        .expect("value field")
        .center();
    visual.simulate_mouse_move(start, None, Modifiers::default());
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::Arrow)
    );
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(!visual.update(|window, _| window.has_hidden_cursor_override()));
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::Arrow),
        "a stationary readout press retains its own cursor"
    );
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(control.read_with(cx, |control, _| control.is_editing()));
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::Arrow)
    );
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.5");
    visual.simulate_keystrokes("enter");
    assert_eq!(control.read_with(cx, |control, _| control.value()), 0.5);

    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    // Horizontal threshold crossing begins a value gesture without changing its value.
    let sideways = start + point(px(4.0), px(0.0));
    visual.simulate_mouse_move(sideways, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::Hidden)
    );
    assert!(visual.update(|window, _| window.has_hidden_cursor_override()));
    assert_eq!(control.read_with(cx, |control, _| control.value()), 0.5);
    let outside = point(px(-40.0), px(-40.0));
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.update(|window, _| window.has_hidden_cursor_override()));
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(!visual.update(|window, _| window.has_hidden_cursor_override()));
    assert!(!control.read_with(cx, |control, _| control.is_interacting()));
}

#[gpui::test]
fn value_cursor_cancellation_focus_loss_and_removal_release_override(cx: &mut TestAppContext) {
    for termination in 0..7 {
        let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let start = visual
            .debug_bounds("test-control-value")
            .expect("value field")
            .center();
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let moved = start - point(px(0.0), px(12.0));
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        assert!(visual.update(|window, _| window.has_hidden_cursor_override()));
        assert_eq!(visual.platform_cursor_style(), gpui::CursorStyle::Hidden);
        match termination {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.simulate_mouse_move(moved, None, Modifiers::default()),
            2 => visual.deactivate_window(),
            3 => {
                control.update(cx, |control, cx| control.set_enabled(false, cx));
            }
            4 => {
                visual.update(|window, _| window.blur());
            }
            5 => {
                visual.update(|window, cx| {
                    window.replace_root(cx, |_, _| Empty);
                });
            }
            _ => {
                visual.update(|window, _| window.remove_window());
            }
        }
        visual.run_until_parked();
        assert_ne!(
            visual.platform_cursor_style(),
            gpui::CursorStyle::Hidden,
            "termination {termination}"
        );
        if termination == 6 {
            continue;
        }
        assert!(
            !visual.update(|window, _| window.has_hidden_cursor_override()),
            "termination {termination}"
        );
        if termination < 5 {
            assert!(!control.read_with(cx, |control, _| control.is_interacting()));
            assert_eq!(control.read_with(cx, |control, _| control.value()), 0.0);
        }
    }
}

#[gpui::test]
fn numeric_focus_corners_preserve_tile_geometry_hits_and_editing(cx: &mut TestAppContext) {
    for appearance in [
        NumericControlPresentation::Knob,
        NumericControlPresentation::CompactDial,
        NumericControlPresentation::MicroDial,
        NumericControlPresentation::InlineValue,
    ] {
        let kind = if appearance == NumericControlPresentation::InlineValue {
            NumericControlKind::HorizontalSlider
        } else {
            NumericControlKind::Knob
        };
        let spec = NumericSpec::new(-1.0, 1.0, 0.0, NumericScale::Bipolar, 2, None::<String>)
            .expect("spec");
        let (handle, control) = open_numeric_control_appearance(cx, kind, spec, 0.0, appearance);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let idle = visual.debug_bounds("test-control").expect("tile");
        assert!(
            visual
                .debug_bounds("test-control-focus-top-left-horizontal")
                .is_none()
        );
        let target = visual
            .debug_bounds(if kind == NumericControlKind::Knob {
                "test-control-knob"
            } else {
                "test-control-value"
            })
            .expect("target");
        if appearance == NumericControlPresentation::MicroDial {
            assert_eq!(
                idle.size,
                size(px(crate::MICRO_DIAL_WIDTH), px(crate::MICRO_DIAL_HEIGHT))
            );
            assert_eq!(
                target.size,
                size(
                    px(crate::MICRO_DIAL_DIAMETER),
                    px(crate::MICRO_DIAL_DIAMETER)
                )
            );
        }
        visual.simulate_click(target.center(), Modifiers::default());
        visual.run_until_parked();
        let check_corners = |visual: &mut VisualTestContext| {
            assert_eq!(
                visual.debug_bounds("test-control").expect("selected tile"),
                idle
            );
            for (name, right, bottom) in [
                ("top-left", false, false),
                ("top-right", true, false),
                ("bottom-left", false, true),
                ("bottom-right", true, true),
            ] {
                for (axis, width, height) in [("horizontal", 3.0, 1.0), ("vertical", 1.0, 3.0)] {
                    let selector = match (name, axis) {
                        ("top-left", "horizontal") => "test-control-focus-top-left-horizontal",
                        ("top-left", _) => "test-control-focus-top-left-vertical",
                        ("top-right", "horizontal") => "test-control-focus-top-right-horizontal",
                        ("top-right", _) => "test-control-focus-top-right-vertical",
                        ("bottom-left", "horizontal") => {
                            "test-control-focus-bottom-left-horizontal"
                        }
                        ("bottom-left", _) => "test-control-focus-bottom-left-vertical",
                        (_, "horizontal") => "test-control-focus-bottom-right-horizontal",
                        _ => "test-control-focus-bottom-right-vertical",
                    };
                    let mark = visual.debug_bounds(selector).expect("corner mark");
                    assert_eq!(mark.size, size(px(width), px(height)));
                    // Border inset is retained. Marks stay near edges and never span a side.
                    assert!(idle.contains(&mark.origin));
                    assert!(idle.contains(&mark.bottom_right()));
                    let x_edge = if right {
                        idle.right() - mark.right()
                    } else {
                        mark.left() - idle.left()
                    };
                    let y_edge = if bottom {
                        idle.bottom() - mark.bottom()
                    } else {
                        mark.top() - idle.top()
                    };
                    assert!(x_edge <= px(1.0) && y_edge <= px(1.0));
                }
            }
        };
        check_corners(&mut visual);
        // Keyboard focus is still owned by the entity; a focused arrow changes its draft.
        visual.simulate_keystrokes("escape");
        visual.simulate_keystrokes("up");
        assert!(control.read_with(cx, |control, _| control.value()) > 0.0);
        let value_target = visual
            .debug_bounds("test-control-value")
            .expect("value target");
        visual.simulate_click(value_target.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("0.50");
        visual.run_until_parked();
        assert!(control.read_with(cx, |control, _| control.is_editing()));
        check_corners(&mut visual);
        visual.simulate_keystrokes("enter");
        assert_eq!(control.read_with(cx, |control, _| control.value()), 0.5);
        let outside = visual.debug_bounds("outside-focus").expect("outside");
        visual.simulate_click(outside.center(), Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("test-control-focus-top-left-horizontal")
                .is_none()
        );
        assert_eq!(
            visual.debug_bounds("test-control").expect("idle tile"),
            idle
        );
        // Disabled controls do not display an active affordance even if focused by owner code.
        control.update(cx, |control, cx| control.set_enabled(false, cx));
        visual.update(|window, cx| control.read(cx).focus.focus(window));
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("test-control-focus-top-left-horizontal")
                .is_none()
        );
    }
}

#[gpui::test]
fn numeric_dial_bounds_stay_fixed_through_press_drag_text_and_release(cx: &mut TestAppContext) {
    for appearance in [
        NumericControlPresentation::Knob,
        NumericControlPresentation::CompactDial,
        NumericControlPresentation::MicroDial,
    ] {
        let spec = NumericSpec::new(
            20.0,
            20_000.0,
            20.0,
            NumericScale::Logarithmic,
            2,
            None::<String>,
        )
        .expect("spec");
        let (handle, _) =
            open_numeric_control_appearance(cx, NumericControlKind::Knob, spec, 20.0, appearance);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let bounds = |visual: &mut VisualTestContext| {
            (
                visual.debug_bounds("test-control").expect("tile"),
                visual.debug_bounds("test-control-knob").expect("dial"),
                visual.debug_bounds("test-control-value").expect("readout"),
            )
        };
        let idle = bounds(&mut visual);
        for _ in 0..3 {
            let start = idle.1.center();
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert_eq!(bounds(&mut visual), idle, "{appearance:?}: mouse-down");
            let moved = start - point(px(0.0), px(24.0));
            visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
            visual.run_until_parked();
            assert_eq!(bounds(&mut visual), idle, "{appearance:?}: dragging");
            visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert_eq!(bounds(&mut visual), idle, "{appearance:?}: release");
        }
        visual.simulate_click(idle.2.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("20000.00");
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual), idle, "{appearance:?}: long text draft");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(
            bounds(&mut visual),
            idle,
            "{appearance:?}: committed wider readout"
        );
    }
}

#[gpui::test]
fn pan_marker_resets_to_neutral_without_starting_drag(cx: &mut TestAppContext) {
    let spec =
        NumericSpec::new(-1.0, 1.0, 0.0, NumericScale::Bipolar, 2, None::<String>).expect("spec");
    let (handle, control) = open_numeric_control_layout(
        cx,
        NumericControlKind::Knob,
        spec,
        0.75,
        NumericControlPresentation::CompactDial,
        DialLayout::Pan,
    );
    let phases = Rc::new(RefCell::new(Vec::new()));
    let output = phases.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            output.borrow_mut().push((event.phase, event.value))
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let knob = visual.debug_bounds("test-control-knob").expect("knob");
    let marker = visual
        .debug_bounds("test-control-pan-reset")
        .expect("marker");
    visual.simulate_click(marker.center(), Modifiers::default());
    visual.run_until_parked();
    control.read_with(cx, |control, _| {
        assert_eq!(control.value(), 0.0);
        assert!(!control.is_interacting());
    });
    assert_eq!(
        phases.borrow().as_slice(),
        [
            (crate::NumericEventPhase::Begin, 0.75),
            (crate::NumericEventPhase::Change, 0.0),
            (crate::NumericEventPhase::Commit, 0.0)
        ]
    );
    assert_eq!(
        visual.debug_bounds("test-control-knob").expect("knob"),
        knob
    );
    control.update(cx, |control, cx| {
        control.set_external_value(-0.5, cx).expect("value");
        control.set_enabled(false, cx);
    });
    visual.simulate_click(marker.center(), Modifiers::default());
    control.read_with(cx, |control, _| assert_eq!(control.value(), -0.5));
    assert_eq!(phases.borrow().len(), 3);
}

struct StereoMeterFixture {
    meter: crate::StereoPeakMeter,
    clicks: usize,
}
impl Render for StereoMeterFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("stereo-host")
            .h(px(120.0))
            .flex()
            .on_click(cx.listener(|fixture, _, _, _| fixture.clicks += 1))
            .child(crate::stereo_peak_meter(
                "meter".into(),
                self.meter.clone(),
                STYLE,
            ))
    }
}

#[gpui::test]
fn stereo_peak_meter_draws_independent_channels_and_passes_clicks_to_its_host(
    cx: &mut TestAppContext,
) {
    let colors = crate::MeterColors {
        signal: 0x33bb77,
        near_peak: 0xffcc33,
        clipping: 0xee3344,
    };
    let reading = |levels, held| crate::StereoPeakMeter {
        levels,
        held,
        tooltip: "output".into(),
        colors,
    };
    let handle = cx.add_window(|_, _| StereoMeterFixture {
        meter: reading([0.001, 1.0], [0.5, 1.2]),
        clicks: 0,
    });
    let view = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(80.0), px(160.0)));
    visual.run_until_parked();
    let left = visual.debug_bounds("meter-left").unwrap();
    let right = visual.debug_bounds("meter-right").unwrap();
    assert_eq!(left.size.width, px(crate::STEREO_PEAK_BAR_WIDTH));
    assert_eq!(right.size.width, px(crate::STEREO_PEAK_BAR_WIDTH));
    assert_eq!(right.left() - left.right(), px(crate::STEREO_PEAK_BAR_GAP));
    assert_eq!(left.size.height, right.size.height);
    // -60 dBFS is the empty floor; unity fills the bar without implying clipping.
    assert_eq!(
        visual.debug_bounds("meter-left-fill").unwrap().size.height,
        px(0.0)
    );
    assert_eq!(
        visual.debug_bounds("meter-right-fill").unwrap().size.height,
        right.size.height
    );
    assert_eq!(
        visual.rendered_background("meter-right-fill"),
        Some(gpui::rgb(colors.near_peak).into())
    );
    assert_eq!(
        visual.rendered_background("meter-left-peak"),
        Some(gpui::rgb(STYLE.text).into())
    );
    assert_eq!(
        visual.rendered_background("meter-right-peak"),
        Some(gpui::rgb(colors.clipping).into())
    );

    visual.simulate_click(right.center(), Modifiers::default());
    view.read_with(cx, |fixture, _| assert_eq!(fixture.clicks, 1));
    view.update(cx, |fixture, cx| {
        fixture.meter = reading([f32::NAN, 2.0], [f32::INFINITY, 0.0]);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("meter-left-fill").unwrap().size.height,
        px(0.0)
    );
    assert_eq!(
        visual.rendered_background("meter-right-fill"),
        Some(gpui::rgb(colors.clipping).into())
    );
    assert!(visual.debug_bounds("meter-left-peak").is_none());
    assert!(visual.debug_bounds("meter-right-peak").is_none());
}

#[gpui::test]
fn accessibility_numeric_uses_real_domain_and_one_event_transaction(cx: &mut TestAppContext) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let phases = Rc::new(RefCell::new(Vec::new()));
    let events = phases.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            events.borrow_mut().push((event.phase, event.value));
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.), px(180.)));
    visual.run_until_parked();
    let id = gpui::AccessibilityId(control.entity_id().as_u64());
    let descriptor = visual.update(|window, _| {
        window
            .accessibility_nodes()
            .find(|node| node.descriptor.id == id)
            .expect("real mounted control")
            .descriptor
            .clone()
    });
    assert_eq!(descriptor.name.as_ref(), "gain");
    let gpui::AccessibilityValue::Number {
        min, max, value, ..
    } = descriptor.value
    else {
        panic!("number");
    };
    assert!(min.is_finite() && max.is_finite() && (min..=max).contains(&value));
    let target = min + (max - min) * 0.75;
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::SetNumber(f64::NAN),
            app
        ))
    );
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::SetNumber(max + 1.),
            app
        ))
    );
    assert!(phases.borrow().is_empty());
    assert!(
        visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::SetNumber(target),
            app
        ))
    );
    visual.run_until_parked();
    assert_eq!(
        *phases.borrow(),
        vec![
            (crate::NumericEventPhase::Begin, value),
            (crate::NumericEventPhase::Change, target),
            (crate::NumericEventPhase::Commit, target)
        ]
    );
    // A cache-only frame must retain semantic nodes just like hitboxes and listeners.
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    assert!(visual.update(|window, _| {
        window
            .accessibility_nodes()
            .any(|node| node.descriptor.id == id)
    }));
    visual.update(|window, app| window.replace_root(app, |_, _| Empty));
    visual.run_until_parked();
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::Increment,
            app
        ))
    );
}

#[gpui::test]
fn accessibility_acknowledged_revision_and_busy_owner_refuse_stale_requests(
    cx: &mut TestAppContext,
) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let (value, acknowledged) = control.read_with(cx, |control, _| {
        (
            control.value,
            control
                .accessibility_value(control.value)
                .expect("acknowledged value"),
        )
    });
    let admitted = Rc::new(Cell::new(true));
    let guard_state = admitted.clone();
    control.update(cx, |control, cx| {
        control.set_accessibility_binding(
            gpui::AccessibilityBinding {
                revision: 7,
                name: "gain".into(),
                value: acknowledged.clone(),
                pending: false,
                guard: Rc::new(move |_| guard_state.get()),
            },
            cx,
        )
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.), px(180.)));
    visual.run_until_parked();
    let id = gpui::AccessibilityId(control.entity_id().as_u64());
    assert!(
        !visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision: 6,
                action: gpui::AccessibilityAction::Increment
            },
            app
        ))
    );
    admitted.set(false); // Owner changes between publication and execution.
    assert!(
        !visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision: 7,
                action: gpui::AccessibilityAction::Increment
            },
            app
        ))
    );
    assert_eq!(control.read_with(cx, |control, _| control.value), value);
    admitted.set(true);
    assert!(
        visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision: 7,
                action: gpui::AccessibilityAction::Increment
            },
            app
        ))
    );
    assert!(
        !visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision: 7,
                action: gpui::AccessibilityAction::Increment
            },
            app
        ))
    );
    visual.run_until_parked();
    let node = visual.update(|window, _| {
        window
            .accessibility_nodes()
            .find(|node| node.descriptor.id == id)
            .unwrap()
            .clone()
    });
    assert!(!node.descriptor.enabled);
    assert_eq!(
        node.descriptor.value, acknowledged,
        "query must retain acknowledged value until owner ACK"
    );
}

#[gpui::test]
fn accessibility_discrete_actions_preserve_choice_inventory_and_focus_only_navigation(
    cx: &mut TestAppContext,
) {
    let (handle, toggle, selector, _) = open_discrete_controls(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.), px(180.)));
    visual.run_until_parked();
    let toggle_id = gpui::AccessibilityId(toggle.entity_id().as_u64());
    let choice_id = gpui::AccessibilityId(selector.entity_id().as_u64());
    assert!(
        visual.update(|window, app| window.perform_accessibility_action(
            toggle_id,
            &gpui::AccessibilityAction::Focus,
            app
        ))
    );
    assert!(!toggle.read_with(cx, |toggle, _| toggle.checked()));
    assert!(
        visual.update(|window, app| window.perform_accessibility_action(
            toggle_id,
            &gpui::AccessibilityAction::Press,
            app
        ))
    );
    assert!(toggle.read_with(cx, |toggle, _| toggle.checked()));
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            choice_id,
            &gpui::AccessibilityAction::Choose(2),
            app
        ))
    );
    assert!(
        visual.update(|window, app| window.perform_accessibility_action(
            choice_id,
            &gpui::AccessibilityAction::Choose(1),
            app
        ))
    );
    assert_eq!(selector.read_with(cx, |selector, _| selector.selected()), 1);
    selector.update(cx, |selector, cx| selector.set_enabled(false, cx));
    // Even the previous painted snapshot cannot authorize a newly disabled live owner.
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            choice_id,
            &gpui::AccessibilityAction::Choose(0),
            app
        ))
    );
}

#[test]
fn accessibility_descriptor_rejects_expired_owner_and_malformed_values() {
    let lifetime = gpui::AccessibilityLifetime::default();
    let mut descriptor = gpui::AccessibilityDescriptor {
        id: gpui::AccessibilityId(1),
        revision: 0,
        lifetime: lifetime.weak(),
        name: "gain".into(),
        value: gpui::AccessibilityValue::Number {
            value: 0.,
            min: -1.,
            max: 1.,
            text: "0 db".into(),
        },
        enabled: true,
        focused: false,
        status: None,
    };
    assert!(descriptor.is_valid());
    descriptor.value = gpui::AccessibilityValue::Number {
        value: f64::INFINITY,
        min: -1.,
        max: 1.,
        text: "invalid".into(),
    };
    assert!(!descriptor.is_valid());
    descriptor.value = gpui::AccessibilityValue::Choice {
        selected: 1,
        options: vec!["one".into()],
    };
    assert!(!descriptor.is_valid());
    descriptor.value = gpui::AccessibilityValue::Toggle(false);
    descriptor.name = "bad\0name".into();
    assert!(!descriptor.is_valid());
    descriptor.name = "on".into();
    drop(lifetime);
    assert!(!descriptor.is_valid());
    assert!(!descriptor.permits(&gpui::AccessibilityAction::Press));
}

struct SemanticLeaf {
    lifetime: gpui::AccessibilityLifetime,
    nested_queries: Rc<Cell<usize>>,
}

impl Render for SemanticLeaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let nested = self.nested_queries.clone();
        div().w(px(100.)).h(px(20.)).accessibility(
            gpui::AccessibilityDescriptor {
                id: gpui::AccessibilityId(9),
                revision: 1,
                lifetime: self.lifetime.weak(),
                name: "fixture control".into(),
                value: gpui::AccessibilityValue::Toggle(false),
                enabled: true,
                focused: false,
                status: None,
            },
            move |_, window, _| {
                // Exercise nested source queries while dispatching; this is not native AX proof.
                assert_eq!(window.accessibility_nodes().count(), 1);
                nested.set(nested.get() + 1);
                false
            },
        )
    }
}

struct SemanticRecoveryRoot {
    first: Entity<SemanticLeaf>,
    duplicate: Option<Entity<SemanticLeaf>>,
}

impl Render for SemanticRecoveryRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut sizing = div().w(px(100.)).h(px(20.));
        let style = sizing.style().clone();
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .child(gpui::AnyView::from(self.first.clone()).cached(style.clone()));
        if let Some(duplicate) = &self.duplicate {
            root = root.child(gpui::AnyView::from(duplicate.clone()).cached(style));
        }
        root
    }
}

#[gpui::test]
fn accessibility_duplicate_snapshot_recovers_through_cached_prepaint_and_nested_query(
    cx: &mut TestAppContext,
) {
    let nested = Rc::new(Cell::new(0));
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let root = cx
        .update_window(handle, |_, window, cx| {
            let first = cx.new(|_| SemanticLeaf {
                lifetime: Default::default(),
                nested_queries: nested.clone(),
            });
            let second = cx.new(|_| SemanticLeaf {
                lifetime: Default::default(),
                nested_queries: nested.clone(),
            });
            window.replace_root(cx, |_, _| SemanticRecoveryRoot {
                first,
                duplicate: Some(second),
            })
        })
        .expect("window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(240.), px(100.)));
    visual.run_until_parked();
    assert_eq!(
        visual.update(|window, _| window.accessibility_nodes().count()),
        0,
        "duplicate IDs must refuse the snapshot"
    );
    root.update(cx, |root, cx| {
        root.duplicate = None;
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(
        visual.update(|window, _| window.accessibility_nodes().count()),
        1,
        "unmodified cached sibling must recover after removing invalid owner"
    );
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            gpui::AccessibilityId(9),
            &gpui::AccessibilityAction::Press,
            app
        ))
    );
    assert_eq!(nested.get(), 1);
}

#[gpui::test]
fn lowercase_numeric_presentation_preserves_raw_clipboard_parser_and_ime_offsets(
    cx: &mut TestAppContext,
) {
    let (handle, control) = open_numeric_control(cx, NumericControlKind::Knob);
    control.update(cx, |control, cx| {
        control
            .set_external_style(
                ControlStyle {
                    text_case: crate::TextCase::Lowercase,
                    ..STYLE
                },
                cx,
            )
            .unwrap();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual.debug_bounds("test-control-value").unwrap();
    visual.simulate_click(value.center(), Modifiers::default());
    visual.simulate_input("0.5E0");
    visual.run_until_parked();
    control.read_with(visual, |control, _| {
        assert_eq!(control.display_text(), "0.5E0");
        assert_eq!(control.last_layout.as_ref().unwrap().text.as_ref(), "0.5e0");
    });
    visual.simulate_keystrokes("cmd-a cmd-c");
    visual.update(|_, cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), "0.5E0"));
    visual.simulate_keystrokes("cmd-v enter");
    visual.run_until_parked();
    control.read_with(visual, |control, _| assert_eq!(control.value(), 0.5));
    let value = visual.debug_bounds("test-control-value").unwrap();
    visual.simulate_click(value.center(), Modifiers::default());
    visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            gpui::EntityInputHandler::replace_and_mark_text_in_range(
                control,
                None,
                "İA",
                Some(1..1),
                window,
                cx,
            );
        })
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            assert_eq!(control.display_text(), "İA");
            assert_eq!(
                control.last_layout.as_ref().unwrap().text.as_ref(),
                "i\u{307}a"
            );
            assert_eq!(
                gpui::EntityInputHandler::marked_text_range(control, window, cx),
                Some(0..2)
            );
            assert_eq!(
                gpui::EntityInputHandler::selected_text_range(control, false, window, cx)
                    .unwrap()
                    .range,
                1..1
            );
            let caret =
                gpui::EntityInputHandler::bounds_for_range(control, 1..1, value, window, cx)
                    .unwrap();
            assert_eq!(
                gpui::EntityInputHandler::character_index_for_point(
                    control,
                    caret.center(),
                    window,
                    cx
                ),
                Some(1),
                "shaped case expansion must map back to original UTF16 caret"
            );
            let mut actual = None;
            assert_eq!(
                gpui::EntityInputHandler::text_for_range(control, 0..2, &mut actual, window, cx)
                    .as_deref(),
                Some("İA")
            );
        })
    });
    visual.simulate_keystrokes("escape");
    control.read_with(visual, |control, _| assert_eq!(control.value(), 0.5));
}

#[gpui::test]
fn numeric_text_double_click_toggles_without_replacing_draft_or_emitting_value_events(
    cx: &mut TestAppContext,
) {
    use gpui::EntityInputHandler;
    let (handle, control) = open_numeric_control(cx, NumericControlKind::Knob);
    let events = Rc::new(RefCell::new(Vec::new()));
    let output = events.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            output.borrow_mut().push(event.phase)
        })
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let field = visual.debug_bounds("test-control-value").unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_input("0.625");
    visual.simulate_keystrokes("cmd-a");
    let at = visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            control
                .bounds_for_range(2..2, field, window, cx)
                .unwrap()
                .center()
        })
    });
    for (first_count, expected) in [(1, 2..2), (3, 0..5), (1, 2..2), (1, 0..5)] {
        for click_count in [first_count, first_count + 1] {
            visual.simulate_event(gpui::MouseDownEvent {
                position: at,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count,
                first_mouse: false,
            });
            visual.simulate_event(gpui::MouseUpEvent {
                position: at,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count,
            });
        }
        control.read_with(visual, |control, _| {
            assert_eq!(
                control.text, "0.625",
                "text selection must not reformat raw uncommitted draft"
            );
            assert_eq!(control.selected, expected);
            assert!(control.is_editing());
            assert_eq!(control.value(), 0.0);
        });
        assert!(
            events.borrow().is_empty(),
            "text doubleclick is not numeric reset/adjustment/commit"
        );
    }
    let start = visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            control
                .bounds_for_range(1..1, field, window, cx)
                .unwrap()
                .center()
        })
    });
    let end = visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            control
                .bounds_for_range(3..3, field, window, cx)
                .unwrap()
                .center()
        })
    });
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    control.read_with(visual, |control, _| {
        assert_eq!(control.selected, 1..3);
        assert_eq!(control.text, "0.625");
        assert_eq!(control.value(), 0.0);
    });
    visual.simulate_mouse_move(start, None, Modifiers::default());
    control.read_with(visual, |control, _| assert_eq!(control.selected, 1..3));
    assert!(events.borrow().is_empty());
    // Invalid composition remains raw text and selection-only, never a value edit.
    visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            control.style.text_case = crate::TextCase::Lowercase;
            control.replace_and_mark_text_in_range(Some(0..5), "İA🎛", Some(0..1), window, cx);
        })
    });
    visual.run_until_parked();
    let at = visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            control
                .bounds_for_range(1..1, field, window, cx)
                .unwrap()
                .center()
        })
    });
    for click_count in [1, 2] {
        visual.simulate_event(gpui::MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        visual.simulate_event(gpui::MouseUpEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count,
        });
    }
    visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            assert_eq!(control.text, "İA🎛");
            assert_eq!(
                control
                    .selected_text_range(false, window, cx)
                    .unwrap()
                    .range,
                0..4
            );
            assert_eq!(control.marked_text_range(window, cx), None);
        })
    });
    assert!(events.borrow().is_empty());
    // Text/IME replacement while a pointer is held invalidates the old anchor.
    let at = visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            control
                .bounds_for_range(4..4, field, window, cx)
                .unwrap()
                .center()
        })
    });
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        control.update(cx, |control, cx| {
            control.replace_and_mark_text_in_range(Some(0..4), "1", Some(1..1), window, cx)
        })
    });
    visual.simulate_mouse_move(
        field.origin + point(px(2.0), field.size.height / 2.0),
        MouseButton::Left,
        Modifiers::default(),
    );
    control.read_with(visual, |control, _| {
        assert_eq!(control.text, "1");
        assert_eq!(
            control.selected,
            1..1,
            "IME replacement must invalidate held pointer anchor"
        );
    });
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn fixed_numeric_presentations_match_their_public_tile_dimensions(cx: &mut TestAppContext) {
    for (kind, appearance, width, height) in [
        (
            NumericControlKind::Knob,
            NumericControlPresentation::CompactDial,
            crate::COMPACT_DIAL_WIDTH,
            crate::COMPACT_DIAL_HEIGHT,
        ),
        (
            NumericControlKind::Knob,
            NumericControlPresentation::MicroDial,
            crate::MICRO_DIAL_WIDTH,
            crate::MICRO_DIAL_HEIGHT,
        ),
        (
            NumericControlKind::HorizontalSlider,
            NumericControlPresentation::InlineValue,
            crate::INLINE_VALUE_WIDTH,
            crate::INLINE_VALUE_HEIGHT,
        ),
    ] {
        let spec =
            NumericSpec::new(0.0, 1.0, 0.5, NumericScale::Linear, 2, None::<String>).unwrap();
        let (handle, _) = open_numeric_control_appearance(cx, kind, spec, 0.5, appearance);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("test-control").expect("numeric tile");
        assert_eq!(bounds.size, size(px(width), px(height)), "{appearance:?}");
    }
}

#[gpui::test]
fn busy_controls_keep_available_paint_but_refuse_input(cx: &mut TestAppContext) {
    use crate::ControlAvailability;
    for kind in [
        NumericControlKind::HorizontalSlider,
        NumericControlKind::VerticalSlider,
        NumericControlKind::Knob,
    ] {
        let (handle, control) = open_numeric_control(cx, kind);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| control.read(cx).focus_handle(cx).focus(window));
        visual.run_until_parked();
        let before = visual.rendered_filled_quads("test-control");
        assert!(!before.is_empty());
        control.update(cx, |control, cx| {
            control.set_availability(ControlAvailability::Busy, cx)
        });
        let id = gpui::AccessibilityId(control.entity_id().as_u64());
        assert!(
            !visual.update(|window, app| window.perform_accessibility_action(
                id,
                &gpui::AccessibilityAction::SetNumber(0.5),
                app
            )),
            "previous available paint cannot authorize a busy control"
        );
        visual.run_until_parked();
        assert_eq!(visual.rendered_filled_quads("test-control"), before);
        let at = visual.debug_bounds("test-control").unwrap().center();
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(at.x, at.y - px(32.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("up enter");
        control.read_with(cx, |control, _| {
            assert_eq!(control.value(), 0.0);
            assert!(!control.is_interacting());
        });
        control.update(cx, |control, cx| control.set_enabled(false, cx));
        visual.run_until_parked();
        assert_ne!(
            visual.rendered_filled_quads("test-control"),
            before,
            "semantic unavailable remains visibly disabled"
        );
        control.update(cx, |control, cx| control.set_enabled(true, cx));
        visual.run_until_parked();
        assert_eq!(visual.rendered_filled_quads("test-control"), before);
    }
    let (handle, toggle, _, _) = open_discrete_controls(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| toggle.read(cx).focus_handle(cx).focus(window));
    visual.run_until_parked();
    let before = visual.rendered_filled_quads("test-toggle");
    toggle.update(cx, |toggle, cx| {
        toggle.set_availability(ControlAvailability::Busy, cx)
    });
    let id = gpui::AccessibilityId(toggle.entity_id().as_u64());
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::Press,
            app
        ))
    );
    visual.run_until_parked();
    assert_eq!(visual.rendered_filled_quads("test-toggle"), before);
    let at = visual.debug_bounds("test-toggle").unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("space");
    assert!(!toggle.read_with(cx, |toggle, _| toggle.checked()));
    toggle.update(cx, |toggle, cx| toggle.set_enabled(true, cx));
    visual.simulate_keystrokes("space");
    assert!(toggle.read_with(cx, |toggle, _| toggle.checked()));
}

#[gpui::test]
fn numeric_f32_storage_and_equivalent_accessibility_values_use_canonical_precision(
    cx: &mut TestAppContext,
) {
    let spec =
        NumericSpec::new_f32(0.1, 0.7, 0.2, NumericScale::Linear, 2, None::<String>).unwrap();
    let (handle, control) =
        open_numeric_control_with_spec(cx, NumericControlKind::HorizontalSlider, spec, 0.1);
    assert_eq!(control.read_with(cx, |c, _| c.value()), f64::from(0.1_f32));
    control.update(cx, |c, cx| c.set_external_value(0.7, cx).unwrap());
    assert_eq!(control.read_with(cx, |c, _| c.value()), f64::from(0.7_f32));
    // Native accessibility preflight retains its declared canonical bounds.
    // Use an interior decimal to test equivalent-value coalescing there.
    control.update(cx, |c, cx| c.set_external_value(0.3, cx).unwrap());
    let acknowledged = control.read_with(cx, |c, _| c.accessibility_value(0.3).unwrap());
    control.update(cx, |c, cx| {
        c.set_accessibility_binding(
            gpui::AccessibilityBinding {
                revision: 1,
                name: "f32".into(),
                value: acknowledged,
                pending: false,
                guard: Rc::new(|_| true),
            },
            cx,
        )
    });
    let phases = Rc::new(RefCell::new(Vec::new()));
    let events = phases.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            events.borrow_mut().push((event.phase, event.value));
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(180.0)));
    visual.run_until_parked();
    let id = gpui::AccessibilityId(control.entity_id().as_u64());
    assert!(
        visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::SetNumber(0.3),
            app
        ))
    );
    assert!(
        phases.borrow().is_empty(),
        "equivalent f32 value is not a new edit"
    );
    assert!(
        visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::SetNumber(f64::from(0.1_f32)),
            app
        )),
        "equivalent earlier value must not mark the owner pending"
    );
    assert_eq!(control.read_with(cx, |c, _| c.value()), f64::from(0.1_f32));
    assert_eq!(
        *phases.borrow(),
        vec![
            (crate::NumericEventPhase::Begin, f64::from(0.3_f32)),
            (crate::NumericEventPhase::Change, f64::from(0.1_f32)),
            (crate::NumericEventPhase::Commit, f64::from(0.1_f32)),
        ]
    );
}

#[gpui::test]
fn meter_fader_input_fill_and_ceiling_triangle_share_calibrated_bounds(cx: &mut TestAppContext) {
    let spec = NumericSpec::new(-24.0, 0.0, -0.3, NumericScale::Linear, 2, Some("db")).unwrap();
    let (handle, control) = open_numeric_control_appearance(
        cx,
        NumericControlKind::VerticalSlider,
        spec,
        -6.0,
        NumericControlPresentation::MeterFader,
    );
    control.update(cx, |control, cx| {
        control.set_meter_readout(
            crate::DeviceMeterReadout {
                level: 10.0_f32.powf(-6.0 / 20.0),
                signal: 0x00ff00,
                clipping: 0xff0000,
            },
            cx,
        )
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(240.0)));
    visual.run_until_parked();
    let rail = visual.debug_bounds("test-control-slider").unwrap();
    let fill = visual.debug_bounds("test-control-input-fill").unwrap();
    let thumb = visual.debug_bounds("test-control-slider-thumb").unwrap();
    assert!((fill.size.height / rail.size.height - 0.75).abs() < 0.02);
    assert!((thumb.center().y - fill.top()).abs() < px(2.0));
    assert!(visual.debug_bounds("test-control-value").unwrap().top() > rail.bottom());
    let start = thumb.center();
    let end = point(start.x, start.y + px(15.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!((control.read_with(cx, |control, _| control.value()) + 9.0).abs() < 0.01);
    assert_eq!(
        visual.debug_bounds("test-control-input-fill").unwrap(),
        fill,
        "ceiling drag cannot invent signal"
    );
}

#[gpui::test]
fn choice_dial_commits_one_bounded_choice_and_cancels_preview(cx: &mut TestAppContext) {
    let handle = *cx.add_window(|_, _| Empty);
    let selector = cx
        .update_window(handle, |_, window, cx| {
            let selector = cx.new(|cx| {
                Selector::from_config(
                    cx,
                    crate::SelectorConfig::new("lookahead", ["1.5 ms", "3 ms", "6 ms"], 0, STYLE)
                        .unwrap()
                        .with_accessibility_name("lookahead")
                        .with_appearance(crate::SelectorAppearance::CompactDial),
                )
            });
            let toggle = cx.new(|cx| Toggle::new(cx, "outside", "outside", false, STYLE).unwrap());
            window.replace_root(cx, |_, _| DiscreteTestRoot {
                toggle,
                selector: selector.clone(),
                key_events: Rc::default(),
            });
            selector
        })
        .unwrap();
    let choices = Rc::new(RefCell::new(Vec::new()));
    let capture = choices.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&selector, move |_, event, _| {
            capture.borrow_mut().push(event.index)
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let start = visual.debug_bounds("lookahead-knob").unwrap().center();
    let end = point(start.x, start.y - px(48.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("up");
    assert!(
        choices.borrow().is_empty(),
        "preview does not publish timing edits"
    );
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(*choices.borrow(), vec![2]);
    assert_eq!(selector.read_with(cx, |selector, _| selector.selected()), 2);
    let start = visual.debug_bounds("lookahead-knob").unwrap().center();
    let end = point(start.x, start.y + px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(*choices.borrow(), vec![2]);
    visual.simulate_keystrokes("down");
    assert_eq!(selector.read_with(cx, |selector, _| selector.selected()), 1);
    let start = visual.debug_bounds("lookahead-knob").unwrap().center();
    let end = point(start.x, start.y - px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    selector.update(cx, |selector, cx| selector.set_selected(0, cx).unwrap());
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(selector.read_with(cx, |selector, _| selector.selected()), 0);
    assert_eq!(
        *choices.borrow(),
        vec![2, 1],
        "external update cancels stale preview without publishing it"
    );
    selector.update(cx, |selector, cx| selector.set_enabled(false, cx));
    visual.simulate_keystrokes("up");
    assert_eq!(selector.read_with(cx, |selector, _| selector.selected()), 0);
}

#[gpui::test]
fn reduction_gauge_grows_down_from_zero_and_bounds_invalid_readings(cx: &mut TestAppContext) {
    struct Gauge(f32);
    impl Render for Gauge {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(24.0)).h(px(100.0)).child(crate::reduction_meter(
                "gr".into(),
                self.0,
                crate::LevelMeterStyle {
                    background: 0x202020,
                    fill: 0xffaa00,
                    above: 0xff0000,
                },
            ))
        }
    }
    let window = cx.add_window(|_, _| Gauge(0.5));
    let gauge = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    for (fraction, expected_height) in [(0.5, 50.0), (2.0, 100.0), (f32::NAN, 0.0)] {
        gauge.update(cx, |gauge, cx| {
            gauge.0 = fraction;
            cx.notify();
        });
        visual.run_until_parked();
        let well = visual.debug_bounds("gr").unwrap();
        let fill = visual.debug_bounds("gr-fill").unwrap();
        assert_eq!(fill.top(), well.top());
        assert_eq!(fill.size.height, px(expected_height));
    }
}
