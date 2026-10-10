use crate::{
    ControlStyle, NumericControl, NumericControlConfig, NumericControlKind, NumericScale,
    NumericSpec, Selector, Toggle,
};
use gpui::{
    AppContext, Context, Empty, Entity, FocusHandle, InteractiveElement, IntoElement, Modifiers,
    MouseButton, MouseDownEvent, ParentElement, Render, Styled, TestAppContext, VisualTestContext,
    Window, div, point, px, size,
};
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
    hover: 0x404040,
    font_size: 10.0,
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

fn open_numeric_control(
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
    cx.update(crate::bind_numeric_control_keys);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let control = cx
        .update_window(handle, |_, window, cx| {
            let control = cx.new(|cx| {
                NumericControl::new(cx, "test-control", "gain", kind, spec, value, STYLE)
                    .expect("numeric control")
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
        start.y + px(30.0),
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
    let normal_position = point(start.x + px(30.0), start.y);
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

    let fine_position = point(normal_position.x + px(12.0), normal_position.y);
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
    let moved = point(start.x + px(36.0), start.y);
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
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.625");
    let finalized = control.update(cx, |control, cx| control.prepare_finish_edit_for_owner(cx));
    let finalized = finalized
        .expect("valid draft prepares")
        .expect("draft value");
    assert_eq!((finalized.previous, finalized.value), (0.0, 0.625));
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
