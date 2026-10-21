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

#[gpui::test]
fn filled_slider_centers_readout_and_bounds_gain_and_bipolar_fill(cx: &mut TestAppContext) {
    cx.update(bind_numeric_control_keys);
    for bipolar in [false, true] {
        let handle = *cx.add_window(|_, _| Empty);
        let (control, root) = cx
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
                    NumericSpec::new(
                        0.0,
                        2.0,
                        1.0,
                        crate::NumericScale::LinearAmplitudeDecibels,
                        1,
                        None::<&str>,
                    )
                    .unwrap()
                };
                let control = cx.new(|cx| {
                    NumericControl::from_config(
                        cx,
                        NumericControlConfig::new(
                            "filled-volume",
                            "volume",
                            NumericControlKind::HorizontalSlider,
                            spec.clone(),
                            spec.default(),
                            STYLE,
                        )
                        .unwrap()
                        .with_appearance(NumericControlAppearance::FilledSlider),
                    )
                });
                let root = window.replace_root(cx, |_, _| Root {
                    control: control.clone(),
                    width: 60.0,
                });
                (control, root)
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(400.0), px(140.0)));
        for width in [60.0, 86.0, 168.0] {
            root.update(cx, |root, cx| {
                root.width = width;
                cx.notify();
            });
            for value in if bipolar {
                [-1.0, 0.0, 1.0]
            } else {
                [0.0, 1.0, 2.0]
            } {
                control.update(cx, |control, cx| {
                    control.set_external_value(value, cx).unwrap()
                });
                visual.run_until_parked();
                let slot = visual.debug_bounds("compact-slot").unwrap();
                let bar = visual.debug_bounds("filled-volume-slider").unwrap();
                let field = visual.debug_bounds("filled-volume-value").unwrap();
                let fill = visual.debug_bounds("filled-volume-bar-fill").unwrap();
                assert_eq!(
                    bar.size.width,
                    px(width - 2.0),
                    "retain shared focus-marker inset"
                );
                assert_eq!(field, bar, "text and bar must occupy one combined field");
                assert!(bar.right() <= slot.right() && bar.bottom() <= slot.bottom());
                assert!(fill.left() >= bar.left() && fill.right() <= bar.right());
                if bipolar {
                    let center = visual.debug_bounds("filled-volume-bar-center").unwrap();
                    assert_eq!(center.left(), bar.center().x);
                    if value < 0.0 {
                        assert_eq!(fill.right(), center.left());
                    }
                    if value > 0.0 {
                        assert_eq!(fill.left(), center.left());
                    }
                    if value == 0.0 {
                        assert_eq!(fill.size.width, px(0.0));
                    }
                } else if value == 0.0 {
                    assert_eq!(fill.size.width, px(0.0));
                } else if value == 2.0 {
                    assert_eq!(fill.size.width, bar.size.width);
                }
            }
        }
    }
}

#[gpui::test]
fn filled_slider_drag_click_entry_invalid_cancel_and_disabled_use_shared_lifecycle(
    cx: &mut TestAppContext,
) {
    cx.update(bind_numeric_control_keys);
    let handle = *cx.add_window(|_, _| Empty);
    let control = cx
        .update_window(handle, |_, window, cx| {
            let control = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    NumericControlConfig::new(
                        "filled-edit",
                        "volume",
                        NumericControlKind::HorizontalSlider,
                        NumericSpec::new(
                            0.0,
                            2.0,
                            1.0,
                            crate::NumericScale::LinearAmplitudeDecibels,
                            1,
                            None::<&str>,
                        )
                        .unwrap(),
                        1.0,
                        STYLE,
                    )
                    .unwrap()
                    .with_appearance(NumericControlAppearance::FilledSlider),
                )
            });
            window.replace_root(cx, |_, _| Root {
                control: control.clone(),
                width: 86.0,
            });
            control
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
    let field = visual.debug_bounds("filled-edit-value").unwrap();
    let start = field.center();
    let end = point(start.x + px(20.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    assert!(control.read_with(cx, |c, _| c.value()) > 1.0);
    assert_eq!(
        events.borrow().iter().map(|e| e.phase).collect::<Vec<_>>(),
        [
            NumericEventPhase::Begin,
            NumericEventPhase::Change,
            NumericEventPhase::Commit
        ]
    );
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.simulate_input("-6 db");
    visual.simulate_keystrokes("enter");
    assert!((control.read_with(cx, |c, _| c.value()) - 10.0_f64.powf(-6.0 / 20.0)).abs() < 1e-12);
    let accepted = control.read_with(cx, |c, _| c.value());
    for draft in ["+100 db", "NaN", "∞", "é"] {
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
        visual.simulate_input(draft);
        visual.simulate_keystrokes("enter");
        control.read_with(cx, |c, _| {
            assert!(c.is_editing());
            assert!(c.invalid_text);
            assert_eq!(c.value(), accepted);
        });
        visual.simulate_keystrokes("escape");
    }
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
    events.borrow_mut().clear();
    control.update(cx, |c, cx| c.set_enabled(false, cx));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn filled_slider_instance_drag_axis_preserves_geometry_and_other_instances(
    cx: &mut TestAppContext,
) {
    cx.update(bind_numeric_control_keys);
    for axis in [
        None,
        Some(NumericDragAxis::Horizontal),
        Some(NumericDragAxis::Vertical),
    ] {
        let handle = *cx.add_window(|_, _| Empty);
        let control = cx
            .update_window(handle, |_, window, cx| {
                let config = NumericControlConfig::new(
                    "axis-pan",
                    "pan",
                    NumericControlKind::HorizontalSlider,
                    NumericSpec::new(
                        -1.0,
                        1.0,
                        0.0,
                        crate::NumericScale::Bipolar,
                        2,
                        None::<&str>,
                    )
                    .unwrap(),
                    0.0,
                    STYLE,
                )
                .unwrap()
                .with_appearance(NumericControlAppearance::FilledSlider);
                let config = match axis {
                    Some(axis) => config.with_drag_axis(axis),
                    None => config,
                };
                let control = cx.new(|cx| NumericControl::from_config(cx, config));
                window.replace_root(cx, |_, _| Root {
                    control: control.clone(),
                    width: 86.0,
                });
                control
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
        visual.run_until_parked();
        let field = visual.debug_bounds("axis-pan-value").unwrap();
        assert_eq!(field, visual.debug_bounds("axis-pan-slider").unwrap());
        let start = field.center();
        for (dx, dy) in [(24.0, 0.0), (0.0, -24.0)] {
            control.update(cx, |control, cx| {
                control.set_external_value(0.0, cx).unwrap()
            });
            events.borrow_mut().clear();
            let end = point(start.x + px(dx), start.y + px(dy));
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
            visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let expected = if axis == Some(NumericDragAxis::Vertical) {
                -dy / 60.0
            } else {
                dx / 60.0
            };
            assert!(
                (control.read_with(cx, |control, _| control.value()) - f64::from(expected)).abs()
                    < 1e-6,
                "axis={axis:?} dx={dx} dy={dy}"
            );
            let events = events.borrow();
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.phase == NumericEventPhase::Begin)
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.phase == NumericEventPhase::Commit)
                    .count(),
                1
            );
            assert_eq!(visual.debug_bounds("axis-pan-value").unwrap(), field);
        }
    }
}

#[gpui::test]
fn integer_readout_accumulates_small_pointer_moves_and_refuses_fractional_text(
    cx: &mut TestAppContext,
) {
    cx.update(bind_numeric_control_keys);
    let handle = *cx.add_window(|_, _| Empty);
    let control = cx
        .update_window(handle, |_, window, cx| {
            let spec = NumericSpec::new(
                0.0,
                3.0,
                0.0,
                crate::NumericScale::Linear,
                0,
                Some("frames"),
            )
            .unwrap()
            .with_integer_values()
            .unwrap();
            let control = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    NumericControlConfig::new(
                        "whole-frames",
                        "start",
                        NumericControlKind::HorizontalSlider,
                        spec,
                        0.0,
                        STYLE,
                    )
                    .unwrap()
                    .with_appearance(NumericControlAppearance::InlineValue),
                )
            });
            window.replace_root(cx, |_, _| Root {
                control: control.clone(),
                width: 136.0,
            });
            control
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
    visual.run_until_parked();
    let start = visual.debug_bounds("whole-frames-value").unwrap().center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    for offset in 1..=24 {
        visual.simulate_mouse_move(
            point(start.x, start.y - px(offset as f32)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
    }
    visual.simulate_mouse_up(
        point(start.x, start.y - px(24.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(control.read_with(cx, |control, _| control.value()), 1.0);
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| event.value.fract() == 0.0)
    );
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|event| event.phase == NumericEventPhase::Commit)
            .count(),
        1
    );
    events.borrow_mut().clear();
    let field = visual.debug_bounds("whole-frames-value").unwrap().center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.5 frames");
    visual.simulate_keystrokes("enter");
    control.read_with(cx, |control, _| {
        assert!(control.is_editing());
        assert!(control.invalid_text);
        assert_eq!(control.value(), 1.0);
    });
    assert!(
        !events
            .borrow()
            .iter()
            .any(|event| event.phase == NumericEventPhase::Commit)
    );
    visual.simulate_keystrokes("escape right");
    assert_eq!(control.read_with(cx, |control, _| control.value()), 2.0);
}
