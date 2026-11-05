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

#[gpui::test]
fn queued_pointer_events_keep_identity_of_synchronously_finalized_drag(cx: &mut TestAppContext) {
    let control = cx.update(|cx| {
        let spec =
            NumericSpec::new(0.0, 200.0, 100.0, crate::NumericScale::Linear, 0, Some("%")).unwrap();
        cx.new(|cx| {
            NumericControl::from_config(
                cx,
                NumericControlConfig::new(
                    "queued-gesture",
                    "volume",
                    NumericControlKind::HorizontalSlider,
                    spec,
                    100.0,
                    STYLE,
                )
                .unwrap(),
            )
        })
    });
    let finalized = Rc::new(RefCell::new(None));
    let events = Rc::new(RefCell::new(Vec::<NumericControlEvent>::new()));
    let capture = events.clone();
    let finalized_before_delivery = finalized.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            assert!(finalized_before_delivery.borrow().is_some());
            capture.borrow_mut().push(event.clone());
        })
    });
    cx.update(|app| {
        control.update(app, |control, cx| {
            let origin = point(px(0.0), px(0.0));
            control.press = Some(Press {
                origin,
                last_position: origin,
                start_value: 100.0,
                integer_pointer_position: None,
                unit_pointer_value: 100.0,
                target: PressTarget::Control,
                dragging: false,
                value_changed: false,
            });
            control.pointer_move(
                &MouseMoveEvent {
                    position: point(px(0.0), px(-20.0)),
                    modifiers: Modifiers::default(),
                    pressed_button: Some(MouseButton::Left),
                },
                cx,
            );
            let value = control.prepare_finish_edit_for_owner(cx).unwrap().unwrap();
            *finalized.borrow_mut() = value.gesture.clone();
            control.accept_finish_edit_for_owner(value, cx);
        });
    });
    let token = finalized.borrow().clone().unwrap();
    let events = events.borrow();
    assert!(
        events
            .iter()
            .any(|event| event.phase == NumericEventPhase::Begin)
    );
    assert!(
        events
            .iter()
            .any(|event| event.phase == NumericEventPhase::Change)
    );
    assert!(
        events
            .iter()
            .all(|event| event.gesture.as_ref() == Some(&token))
    );
    assert!(
        events
            .iter()
            .all(|event| event.phase != NumericEventPhase::Commit)
    );
}

#[gpui::test]
fn pointer_gesture_identity_survives_owner_finalization_and_distinguishes_later_edits(
    cx: &mut TestAppContext,
) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let control = cx
        .update_window(handle, |_, window, cx| {
            let spec =
                NumericSpec::new(0.0, 200.0, 100.0, crate::NumericScale::Linear, 0, Some("%"))
                    .unwrap();
            let control = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    NumericControlConfig::new(
                        "gesture-identity",
                        "volume",
                        NumericControlKind::HorizontalSlider,
                        spec,
                        100.0,
                        STYLE,
                    )
                    .unwrap()
                    .with_presentation(NumericControlPresentation::CompactSlider),
                )
            });
            window.replace_root(cx, |_, _| Root {
                control: control.clone(),
                width: 86.0,
            });
            control
        })
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::<NumericControlEvent>::new()));
    let capture = events.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            capture.borrow_mut().push(event.clone())
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(140.0)));
    visual.run_until_parked();
    let start = visual
        .debug_bounds("gesture-identity-slider")
        .unwrap()
        .center();
    let end = point(start.x + px(25.0), start.y - px(20.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let first = events.borrow().first().unwrap().gesture.clone().unwrap();
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| event.gesture.as_ref() == Some(&first))
    );
    assert_eq!(
        events.borrow().last().unwrap().phase,
        NumericEventPhase::Commit
    );

    events.borrow_mut().clear();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let finalized = events.borrow().first().unwrap().gesture.clone().unwrap();
    assert_ne!(first, finalized);
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| event.gesture.as_ref() == Some(&finalized))
    );
    events.borrow_mut().clear();
    control.update(cx, |control, cx| {
        assert!(control.emit_accessible_value(80.0, cx))
    });
    visual.run_until_parked();
    assert_eq!(events.borrow().len(), 3);
    assert!(events.borrow().iter().all(|event| event.gesture.is_none()));
    let final_value = control.update(cx, |control, cx| {
        control.prepare_finish_edit_for_owner(cx).unwrap().unwrap()
    });
    assert_eq!(final_value.gesture.as_ref(), Some(&finalized));
    let count = events.borrow().len();
    control.update(cx, |control, cx| {
        control.accept_finish_edit_for_owner(final_value, cx)
    });
    visual.run_until_parked();
    assert_eq!(
        events.borrow().len(),
        count,
        "owner finalization does not emit events"
    );

    events.borrow_mut().clear();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    control.update(cx, |control, cx| control.cancel(cx));
    visual.run_until_parked();
    let cancelled = events.borrow().last().unwrap().gesture.clone().unwrap();
    assert_ne!(cancelled, first);
    assert_ne!(cancelled, finalized);
    assert_eq!(
        events.borrow().last().unwrap().phase,
        NumericEventPhase::Cancel
    );
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| event.gesture.as_ref() == Some(&cancelled))
    );
}

struct Root {
    control: Entity<NumericControl>,
    width: f32,
}
impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .debug_selector(|| "compact-slot".into())
                .flex()
                .w(px(self.width))
                .h(px(match self.control.read(cx).kind {
                    NumericControlKind::VerticalSlider => 90.0,
                    NumericControlKind::Knob => 72.0,
                    NumericControlKind::HorizontalSlider
                        if self.control.read(cx).appearance
                            == NumericControlAppearance::LabeledSlider =>
                    {
                        32.0
                    }
                    NumericControlKind::HorizontalSlider => 21.0,
                }))
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
                    .with_presentation(NumericControlPresentation::CompactSlider),
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
            captured.borrow_mut().push(event.clone())
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
        assert!(value.left() >= slider.left() && value.right() <= slider.right());
        assert!(value.top() >= slider.top() && value.bottom() <= slider.bottom());
        for gain in [0.0, 200.0] {
            control.update(cx, |control, cx| {
                control.set_external_value(gain, cx).unwrap()
            });
            visual.run_until_parked();
            let fill = visual.debug_bounds("header-volume-bar-fill").unwrap();
            assert!(fill.left() >= slider.left() && fill.right() <= slider.right());
            assert!(visual.debug_bounds("header-volume-slider-thumb").is_none());
        }
    }
    control.update(cx, |control, cx| {
        control.set_external_value(100.0, cx).unwrap()
    });
    visual.run_until_parked();
    let slider = visual.debug_bounds("header-volume-slider").unwrap();
    let start = slider.center();
    let outside = point(start.x + px(100.0), start.y - px(40.0));
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
        point(start.x, start.y + px(25.0)),
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
fn horizontal_slider_centers_readout_and_bounds_gain_and_bipolar_fill(cx: &mut TestAppContext) {
    cx.update(bind_numeric_control_keys);
    for (bipolar, appearance) in [false, true].into_iter().flat_map(|b| {
        [
            NumericControlPresentation::FilledSlider,
            NumericControlPresentation::LabeledSlider,
        ]
        .map(|appearance| (b, appearance))
    }) {
        let kind = NumericControlKind::HorizontalSlider;
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
                            kind,
                            spec.clone(),
                            spec.default(),
                            STYLE,
                        )
                        .unwrap()
                        .with_presentation(appearance),
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
                assert_eq!(
                    field.center(),
                    bar.center(),
                    "text is centered inside the bar"
                );
                assert_eq!(field.size.width, bar.size.width);
                assert_eq!(field.size.height, bar.size.height);
                assert!(visual.debug_bounds("filled-volume-slider-thumb").is_none());
                assert!(bar.right() <= slot.right() && bar.bottom() <= slot.bottom());
                assert!(fill.left() >= bar.left() && fill.right() <= bar.right());
                assert_eq!(bar.size.height, px(14.0));
                if appearance == NumericControlPresentation::LabeledSlider {
                    let widget = visual.debug_bounds("filled-volume").unwrap();
                    assert!(bar.top() >= widget.top() + px(15.0));
                }
                let extent = fill.size.width;
                let full_extent = field.size.width;
                assert!(fill.top() >= bar.top() && fill.bottom() <= bar.bottom());
                if bipolar {
                    let center = visual.debug_bounds("filled-volume-bar-center").unwrap();
                    assert_eq!(center.left(), field.center().x);
                    assert!((extent - full_extent * (value as f32).abs() * 0.5).abs() <= px(1.0));
                } else if value == 0.0 {
                    assert_eq!(extent, px(0.0));
                } else if value == 2.0 {
                    assert_eq!(extent, full_extent);
                }
            }
        }
    }
}

#[gpui::test]
fn vertical_fader_keeps_handle_in_bounds_and_readout_below_with_shared_drag_lifecycle(
    cx: &mut TestAppContext,
) {
    cx.update(bind_numeric_control_keys);
    let handle = *cx.add_window(|_, _| Empty);
    let control = cx
        .update_window(handle, |_, window, cx| {
            let config = NumericControlConfig::new(
                "fader",
                "gain",
                NumericControlKind::VerticalSlider,
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
            .unwrap();
            let control = cx.new(|cx| NumericControl::from_config(cx, config));
            window.replace_root(cx, |_, _| Root {
                control: control.clone(),
                width: 60.0,
            });
            control
        })
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            captured.borrow_mut().push(event.clone())
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(140.0)));
    for value in [-1.0, 0.0, 1.0] {
        control.update(cx, |control, cx| {
            control.set_external_value(value, cx).unwrap()
        });
        visual.run_until_parked();
        let rail = visual.debug_bounds("fader-slider").unwrap();
        let thumb = visual.debug_bounds("fader-slider-thumb").unwrap();
        let readout = visual.debug_bounds("fader-value").unwrap();
        let widget = visual.debug_bounds("fader").unwrap();
        assert_eq!(rail.size.width, px(10.0));
        assert!(thumb.left() > rail.right());
        assert!(thumb.top() >= widget.top() && thumb.bottom() < readout.top());
        assert!(readout.top() > rail.bottom());
        let expected = rail.bottom() - rail.size.height * ((value as f32 + 1.0) / 2.0);
        assert!((thumb.center().y - expected).abs() <= px(1.0));
        assert!(visual.debug_bounds("fader-bar-fill").is_none());
    }
    control.update(cx, |control, cx| {
        control.set_external_value(0.0, cx).unwrap()
    });
    visual.run_until_parked();
    let readout = visual.debug_bounds("fader-value").unwrap();
    let start = visual.debug_bounds("fader-slider-thumb").unwrap().center();
    let end = point(start.x, start.y - px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!((control.read_with(cx, |c, _| c.value()) - 0.4).abs() < 1e-6);
    assert_eq!(visual.debug_bounds("fader-value").unwrap(), readout);
    assert_eq!(
        events.borrow().first().unwrap().phase,
        NumericEventPhase::Begin
    );
    assert_eq!(
        events.borrow().last().unwrap().phase,
        NumericEventPhase::Commit
    );

    let start = visual.debug_bounds("fader-slider-thumb").unwrap().center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x, start.y + px(12.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    control.update(cx, |control, cx| control.cancel(cx));
    visual.run_until_parked();
    assert!((control.read_with(cx, |c, _| c.value()) - 0.4).abs() < 1e-6);
    assert_eq!(
        events.borrow().last().unwrap().phase,
        NumericEventPhase::Cancel
    );
    for deactivate in [false, true] {
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let start = visual.debug_bounds("fader-slider-thumb").unwrap().center();
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(start.x, start.y + px(12.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.run_until_parked();
        if deactivate {
            visual.deactivate_window();
        } else {
            visual.update(|window, _| window.blur());
        }
        visual.run_until_parked();
        assert!(!control.read_with(cx, |c, _| c.is_interacting()));
        assert!((control.read_with(cx, |c, _| c.value()) - 0.4).abs() < 1e-6);
        assert_eq!(
            events.borrow().last().unwrap().phase,
            NumericEventPhase::Cancel
        );
        assert!(!visual.update(|window, _| window.has_hidden_cursor_override()));
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
                    .with_presentation(NumericControlPresentation::FilledSlider),
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
            captured.borrow_mut().push(event.clone())
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(140.0)));
    let field = visual.debug_bounds("filled-edit-value").unwrap();
    let start = field.center();
    let end = point(start.x, start.y - px(20.0));
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
fn all_numeric_presentations_scrub_upward_and_ignore_horizontal_motion(cx: &mut TestAppContext) {
    cx.update(bind_numeric_control_keys);
    for (kind, appearance) in [
        (
            NumericControlKind::HorizontalSlider,
            NumericControlPresentation::HorizontalSlider,
        ),
        (
            NumericControlKind::HorizontalSlider,
            NumericControlPresentation::CompactSlider,
        ),
        (
            NumericControlKind::HorizontalSlider,
            NumericControlPresentation::FilledSlider,
        ),
        (
            NumericControlKind::HorizontalSlider,
            NumericControlPresentation::LabeledSlider,
        ),
        (
            NumericControlKind::VerticalSlider,
            NumericControlPresentation::VerticalSlider,
        ),
        (NumericControlKind::Knob, NumericControlPresentation::Knob),
        (
            NumericControlKind::Knob,
            NumericControlPresentation::MicroDial,
        ),
    ] {
        let handle = *cx.add_window(|_, _| Empty);
        let control = cx
            .update_window(handle, |_, window, cx| {
                let config = NumericControlConfig::new(
                    "axis-pan",
                    "pan",
                    kind,
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
                .with_presentation(appearance);
                let control = cx.new(|cx| NumericControl::from_config(cx, config));
                window.replace_root(cx, |_, _| Root {
                    control: control.clone(),
                    width: 132.0,
                });
                control
            })
            .unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.update(|app| {
            app.subscribe(&control, move |_, event, _| {
                captured.borrow_mut().push(event.clone())
            })
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(400.0), px(140.0)));
        visual.run_until_parked();
        let field = visual.debug_bounds("axis-pan-value").unwrap();
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
            let expected = -dy / 60.0;
            assert!(
                (control.read_with(cx, |control, _| control.value()) - f64::from(expected)).abs()
                    < 1e-6,
                "kind={kind:?} appearance={appearance:?} dx={dx} dy={dy}"
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
                    .with_presentation(NumericControlPresentation::InlineValue),
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
            captured.borrow_mut().push(event.clone())
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

#[gpui::test]
fn explicit_unit_readout_drag_accumulates_fine_motion_clamps_and_cancels(cx: &mut TestAppContext) {
    cx.update(bind_numeric_control_keys);
    let handle = *cx.add_window(|_, _| Empty);
    let control = cx
        .update_window(handle, |_, window, cx| {
            let spec = NumericSpec::new(
                0.0,
                60_000.0,
                0.0,
                crate::NumericScale::Linear,
                6,
                Some("ms"),
            )
            .unwrap();
            let control = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    NumericControlConfig::new(
                        "envelope-time",
                        "attack",
                        NumericControlKind::HorizontalSlider,
                        spec,
                        0.0,
                        STYLE,
                    )
                    .unwrap()
                    .with_presentation(NumericControlPresentation::InlineValue)
                    .with_value_drag_steps(1.0, 0.1)
                    .unwrap(),
                )
            });
            window.replace_root(cx, |_, _| Root {
                control: control.clone(),
                width: 120.0,
            });
            control
        })
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let _subscription = cx.update(|app| {
        app.subscribe(&control, move |_, event, _| {
            captured.borrow_mut().push(event.clone())
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(140.0)));
    visual.run_until_parked();
    let start = visual.debug_bounds("envelope-time-value").unwrap().center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    // Threshold crossing includes the first full movement; subsequent tiny
    // motions retain their exact displayed-unit contribution.
    let coarse = point(start.x, start.y - px(10.0));
    visual.simulate_mouse_move(coarse, Some(MouseButton::Left), Modifiers::default());
    assert_eq!(control.read_with(cx, |c, _| c.value()), 10.0);
    for i in 1..=20 {
        visual.simulate_mouse_move(
            point(start.x, coarse.y - px(i as f32 * 0.25)),
            Some(MouseButton::Left),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
    }
    let end = point(start.x, coarse.y - px(5.0));
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    assert!((control.read_with(cx, |c, _| c.value()) - 10.5).abs() < 1e-12);
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
    events.borrow_mut().clear();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(coarse, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(coarse, MouseButton::Left, Modifiers::default());
    assert!((control.read_with(cx, |c, _| c.value()) - 10.5).abs() < 1e-12);
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
    control.update(cx, |c, cx| c.set_external_value(59_999.0, cx).unwrap());
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(coarse, Some(MouseButton::Left), Modifiers::default());
    assert_eq!(control.read_with(cx, |c, _| c.value()), 60_000.0);
    let back = point(start.x, coarse.y + px(1.0));
    visual.simulate_mouse_move(back, Some(MouseButton::Left), Modifiers::default());
    assert_eq!(control.read_with(cx, |c, _| c.value()), 59_999.0);
    visual.simulate_mouse_up(back, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.0625 ms");
    visual.simulate_keystrokes("enter");
    assert_eq!(control.read_with(cx, |c, _| c.value()), 0.0625);
}

#[gpui::test]
fn numeric_same_position_input_restarts_hidden_caret(cx: &mut TestAppContext) {
    cx.update(bind_numeric_control_keys);
    let handle = *cx.add_window(|_, _| Empty);
    cx.update_window(handle, |_, window, cx| {
        let control = cx.new(|cx| {
            NumericControl::from_config(
                cx,
                NumericControlConfig::new(
                    "caret-edit",
                    "value",
                    NumericControlKind::HorizontalSlider,
                    NumericSpec::new(0.0, 2.0, 1.0, crate::NumericScale::Linear, 2, None::<&str>)
                        .unwrap(),
                    1.0,
                    STYLE,
                )
                .unwrap()
                .with_presentation(NumericControlPresentation::FilledSlider),
            )
        });
        window.replace_root(cx, |_, _| Root {
            control,
            width: 86.0,
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(400.0), px(140.0)));
    let at = visual.debug_bounds("caret-edit-value").unwrap().center();
    visual.simulate_click(at, Modifiers::default());
    visual.simulate_keystrokes("end");
    visual.run_until_parked();
    let caret = gpui::Background::from(gpui::rgb(STYLE.accent)).opacity(0.5);
    let is_visible = |visual: &mut VisualTestContext| {
        visual
            .rendered_filled_quads("caret-edit-value")
            .iter()
            .any(|(_, fill)| *fill == caret)
    };
    assert!(is_visible(&mut visual));
    visual
        .executor()
        .advance_clock(std::time::Duration::from_millis(510));
    visual.run_until_parked();
    assert!(!is_visible(&mut visual));
    visual.simulate_keystrokes("delete");
    visual.run_until_parked();
    assert!(is_visible(&mut visual));
}
