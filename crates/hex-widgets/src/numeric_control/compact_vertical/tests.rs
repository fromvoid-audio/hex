use super::*;
use gpui::{Empty, Modifiers, TestAppContext, VisualTestContext, point, size};
use std::cell::RefCell;

const STYLE: ControlStyle = ControlStyle {
    background: 0x202020,
    surface: 0x303030,
    raised: 0x383838,
    rail: 0x181818,
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
struct Root(Entity<NumericControl>);
impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_start()
            .p(px(8.0))
            .child(self.0.clone())
    }
}
fn fixture(cx: &mut TestAppContext, bipolar: bool) -> (Entity<NumericControl>, VisualTestContext) {
    cx.update(bind_numeric_control_keys);
    let handle = *cx.add_window(|_, _| Empty);
    let control = cx
        .update_window(handle, |_, window, cx| {
            let spec = if bipolar {
                NumericSpec::new(
                    -1.0,
                    1.0,
                    0.0,
                    crate::NumericScale::Bipolar,
                    2,
                    None::<&str>,
                )
                .unwrap()
            } else {
                NumericSpec::new(0.0, 100.0, 50.0, crate::NumericScale::Linear, 1, Some("%"))
                    .unwrap()
            };
            let control = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    NumericControlConfig::new(
                        "keytrack",
                        "pitch keytrack",
                        NumericControlKind::VerticalSlider,
                        spec.clone(),
                        spec.default(),
                        STYLE,
                    )
                    .unwrap()
                    .with_presentation(NumericControlPresentation::FilledVerticalSlider),
                )
            });
            window.replace_root(cx, |_, _| Root(control.clone()));
            control
        })
        .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(200.0), px(120.0)));
    (control, visual)
}

#[gpui::test]
fn compact_geometry_and_bottom_up_fill_follow_external_values(cx: &mut TestAppContext) {
    let (control, mut visual) = fixture(cx, false);
    for value in [0.0, 50.0, 100.0] {
        control.update(cx, |c, cx| c.set_external_value(value, cx).unwrap());
        visual.run_until_parked();
        let root = visual.debug_bounds("keytrack").unwrap();
        let rail = visual.debug_bounds("keytrack-slider").unwrap();
        let fill = visual.debug_bounds("keytrack-bar-fill").unwrap();
        assert_eq!(root.size, size(px(12.0), px(32.0)));
        assert_eq!(fill.bottom(), rail.bottom());
        assert_eq!(fill.size.width, rail.size.width);
        assert!(
            (f32::from(fill.size.height) - f32::from(rail.size.height) * value as f32 / 100.0)
                .abs()
                < 0.01
        );
        assert!(visual.debug_bounds("keytrack-value").is_none());
        assert!(visual.debug_bounds("keytrack-slider-thumb").is_none());
        if value == 0.0 {
            assert_eq!(
                visual.rendered_background("keytrack-slider"),
                Some(rgb(STYLE.rail).into())
            );
        }
        if value > 0.0 {
            assert!(
                visual
                    .rendered_filled_quads("keytrack-slider")
                    .iter()
                    .any(|(_, color)| *color == gpui::Background::from(rgb(STYLE.numeric_fill)))
            );
        }
    }
}

#[gpui::test]
fn pointer_keyboard_cancel_and_disabled_keep_shared_numeric_events(cx: &mut TestAppContext) {
    let (control, mut visual) = fixture(cx, false);
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&control, move |_, event, _| {
            sink.borrow_mut().push(event.clone())
        })
    });
    let start = visual.debug_bounds("keytrack-slider").unwrap().center();
    let end = point(start.x, start.y - px(20.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let accepted = control.read_with(cx, |c, _| c.value());
    assert!(accepted > 50.0);
    assert_eq!(
        events.borrow().iter().map(|e| e.phase).collect::<Vec<_>>(),
        [
            NumericEventPhase::Begin,
            NumericEventPhase::Change,
            NumericEventPhase::Commit
        ]
    );
    events.borrow_mut().clear();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    assert_eq!(control.read_with(cx, |c, _| c.value()), accepted);
    assert!(
        events
            .borrow()
            .iter()
            .any(|e| e.phase == NumericEventPhase::Cancel)
    );
    assert!(
        !events
            .borrow()
            .iter()
            .any(|e| e.phase == NumericEventPhase::Commit)
    );
    control.update(cx, |c, cx| c.set_external_value(50.0, cx).unwrap());
    events.borrow_mut().clear();
    visual.simulate_click(start, Modifiers::default());
    visual.simulate_keystrokes("enter");
    assert!(
        !control.read_with(cx, |c, _| c.is_editing()),
        "no invisible text entry on a readout-free rail"
    );
    visual.simulate_keystrokes("up");
    let coarse = control.read_with(cx, |c, _| c.value());
    assert!(coarse > 50.0);
    visual.simulate_keystrokes("shift-up");
    let fine = control.read_with(cx, |c, _| c.value());
    assert!(fine > coarse && fine - coarse < coarse - 50.0);
    control.update(cx, |c, cx| c.set_enabled(false, cx));
    events.borrow_mut().clear();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("down");
    assert!(events.borrow().is_empty());
    assert_eq!(control.read_with(cx, |c, _| c.value()), fine);
}

#[gpui::test]
fn readout_free_rail_keeps_real_accessible_domain_and_exact_value_actions(cx: &mut TestAppContext) {
    let (control, mut visual) = fixture(cx, false);
    visual.run_until_parked();
    let id = gpui::AccessibilityId(control.entity_id().as_u64());
    let descriptor = visual.update(|window, _| {
        window
            .accessibility_nodes()
            .find(|n| n.descriptor.id == id)
            .unwrap()
            .descriptor
            .clone()
    });
    assert_eq!(descriptor.name.as_ref(), "pitch keytrack");
    assert!(matches!(
        descriptor.value,
        gpui::AccessibilityValue::Number {
            value: 50.0,
            min: 0.0,
            max: 100.0,
            ..
        }
    ));
    for invalid in [f64::NAN, -0.01, 100.01] {
        assert!(!visual.update(|w, cx| w.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::SetNumber(invalid),
            cx
        )));
    }
    assert!(visual.update(|w, cx| w.perform_accessibility_action(
        id,
        &gpui::AccessibilityAction::SetNumber(37.5),
        cx
    )));
    assert_eq!(control.read_with(cx, |c, _| c.value()), 37.5);
    control.update(cx, |c, cx| {
        c.set_external_value(37.5, cx).unwrap();
        c.set_enabled(false, cx)
    });
    visual.run_until_parked();
    assert!(!visual.update(|w, cx| w.perform_accessibility_action(
        id,
        &gpui::AccessibilityAction::Increment,
        cx
    )));
}

#[gpui::test]
fn bipolar_fill_uses_the_center_as_its_origin(cx: &mut TestAppContext) {
    let (control, mut visual) = fixture(cx, true);
    for value in [-1.0, 0.0, 1.0] {
        control.update(cx, |c, cx| c.set_external_value(value, cx).unwrap());
        visual.run_until_parked();
        let rail = visual.debug_bounds("keytrack-slider").unwrap();
        let fill = visual.debug_bounds("keytrack-bar-fill").unwrap();
        let center = rail.center().y;
        if value < 0.0 {
            assert_eq!(fill.top(), center);
            assert_eq!(fill.bottom(), rail.bottom());
        } else if value > 0.0 {
            assert_eq!(fill.top(), rail.top());
            assert_eq!(fill.bottom(), center);
        } else {
            assert_eq!(fill.size.height, px(0.0));
        }
    }
}
