use super::*;
use gpui::Focusable;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[gpui::test]
fn fine_sliders_commit_signed_hz_history_and_keyboard_without_changing_envelope(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (operator, value) in [-1000.0, 1000.0, 0.125, -0.125].into_iter().enumerate() {
        let parameter = 609 + operator as u32;
        let slider = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}-slider",
                device.value()
            )))
            .expect("real fine-Hz slider");
        let row = visual
            .debug_bounds(selector(format!(
                "operator-row-{}-{operator}",
                device.value()
            )))
            .unwrap();
        assert_eq!(slider.size, size(px(58.0), px(14.0)));
        assert!(slider.left() >= row.left() && slider.right() <= row.right());
        let fill = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}-bar-fill",
                device.value()
            )))
            .unwrap();
        assert_eq!(
            fill.left(),
            slider.left(),
            "signed Hz fills from the left edge"
        );
        assert!(
            (fill.size.width - slider.size.width / 2.0).abs() < px(1.0),
            "zero is halfway through [-1000,+1000]"
        );
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{parameter}-knob",
                    device.value()
                )))
                .is_none()
        );
        let field = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}-value",
                device.value()
            )))
            .unwrap();
        visual.simulate_click(field.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(&format!("{value} hz"));
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|s| {
                    operator_parameter_value(s, track, device, parameter)
                        == ParameterValue::Float(value as f32)
                })
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&device].selected_operator,
                0
            );
            assert_eq!(
                view.device_panel.operator_pages[&device].page,
                operator_panel::OperatorPage::Envelope
            );
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|s| {
                    operator_parameter_value(s, track, device, parameter)
                        == ParameterValue::Float(0.0)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|s| {
                    operator_parameter_value(s, track, device, parameter)
                        == ParameterValue::Float(value as f32)
                })
        });
        visual.run_until_parked();
    }
    visual.simulate_keystrokes("escape");
    visual.simulate_keystrokes("up");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|s| {
                operator_parameter_value(s, track, device, 612) == ParameterValue::Float(0.875)
            })
    });
    visual.run_until_parked();
    let field = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-612-value",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0 hz");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|s| {
                operator_parameter_value(s, track, device, 612) == ParameterValue::Float(0.0)
            })
    });
    visual.run_until_parked();
    let start = field.center();
    let end = point(start.x, start.y - px(12.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.state.as_ref().is_some_and(|s| matches!(operator_parameter_value(s, track, device, 612), ParameterValue::Float(value) if value > 0.0))
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            0
        )
    });
}

#[gpui::test]
fn fine_invalid_draft_and_stale_owner_cannot_change_the_project(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let entity = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(610),
        }] {
            ParameterControl::Float { entity, .. } => entity.clone(),
            _ => panic!("fine slider"),
        }
    });
    let field = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-610-value",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    for key in ["tab", "shift-tab"] {
        visual.simulate_keystrokes(key);
        visual.run_until_parked();
        assert!(visual.update(|window, app| entity.focus_handle(app).is_focused(window)));
    }
    visual.simulate_keystrokes("escape");
    editor.update(cx, |view, cx| {
        view.selected_track = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, app| entity.focus_handle(app).focus(window));
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.device_gesture.snapshot().is_none());
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.project_generation += 1;
        view.device_panel.operator_pages.clear();
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, app| entity.focus_handle(app).focus(window));
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.device_gesture.snapshot().is_none());
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            0
        );
    });
}
