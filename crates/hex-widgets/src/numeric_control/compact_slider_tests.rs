use super::*;
use gpui::{Empty, Modifiers, TestAppContext, VisualTestContext, point, size};
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
    focus_marker: 0xaaaaaa,
    dial_rail: 0x000000,
    hover: 0x404040,
    font_size: 10.0,
    text_case: crate::TextCase::Preserve,
};

struct Root {
    control: Entity<NumericControl>,
    width: f32,
}
impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .debug_selector(|| "compact-slot".into())
                .flex()
                .w(px(self.width))
                .h(px(21.0))
                .child(self.control.clone()),
        )
    }
}

#[gpui::test]
fn compact_slider_fits_header_slot_and_keeps_capture_cancel_and_text(cx: &mut TestAppContext) {
    cx.update(bind_numeric_control_keys);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let (control, root) = cx
        .update_window(handle, |_, window, cx| {
            let spec =
                NumericSpec::new(0.0, 200.0, 100.0, crate::NumericScale::Linear, 0, Some("%"))
                    .unwrap();
            let control = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    NumericControlConfig::new(
                        "header-volume",
                        "volume",
                        NumericControlKind::HorizontalSlider,
                        spec,
                        100.0,
                        STYLE,
                    )
                    .unwrap()
                    .with_appearance(NumericControlAppearance::CompactSlider),
                )
            });
            let root = window.replace_root(cx, |_, _| Root {
                control: control.clone(),
                width: 86.0,
            });
            (control, root)
        })
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            captured.borrow_mut().push(*event)
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(140.0)));
    for width in [86.0, 210.0] {
        root.update(cx, |root, cx| {
            root.width = width;
            cx.notify();
        });
        visual.run_until_parked();
        let slot = visual.debug_bounds("compact-slot").unwrap();
        let slider = visual.debug_bounds("header-volume-slider").unwrap();
        let value = visual.debug_bounds("header-volume-value").unwrap();
        let widget = visual.debug_bounds("header-volume").unwrap();
        assert!(widget.right() <= slot.right() && widget.bottom() <= slot.bottom());
        assert!(slider.size.width >= px(36.0));
        assert!(slider.right() <= value.origin.x && value.right() <= slot.right());
        for gain in [0.0, 200.0] {
            control.update(cx, |control, cx| {
                control.set_external_value(gain, cx).unwrap()
            });
            visual.run_until_parked();
            let thumb = visual.debug_bounds("header-volume-slider-thumb").unwrap();
            assert!(thumb.left() >= slider.left() && thumb.right() <= slider.right());
        }
    }
    control.update(cx, |control, cx| {
        control.set_external_value(100.0, cx).unwrap()
    });
    visual.run_until_parked();
    let slider = visual.debug_bounds("header-volume-slider").unwrap();
    let start = slider.center();
    let outside = point(start.x + px(100.0), start.y + px(40.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|e| e.phase == NumericEventPhase::Begin)
            .count(),
        1
    );
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|e| e.phase == NumericEventPhase::Commit)
            .count(),
        1
    );
    assert!(control.read_with(cx, |control, _| control.value()) > 100.0);
    events.borrow_mut().clear();
    let before = control.read_with(cx, |control, _| control.value());
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x - px(25.0), start.y),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(control.read_with(cx, |control, _| control.value()), before);
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
    let field = visual.debug_bounds("header-volume-value").unwrap().center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("75%");
    visual.simulate_keystrokes("enter");
    control.read_with(cx, |control, _| {
        assert!(!control.is_editing());
        assert_eq!(control.value(), 75.0);
    });
}
