use super::*;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[gpui::test]
fn oscillator_ratio_and_fixed_hz_sliders_edit_independently_with_canonical_history(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    open_operator_options(&mut visual, device);
    for operator in 0..4 {
        let ratio = 1 + operator * 3;
        let fixed = 62 + operator * 3;
        let field = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{ratio}-slider",
                device.value()
            )))
            .unwrap();
        assert_eq!(field.size, size(px(58.0), px(14.0)));
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{ratio}-knob",
                    device.value()
                )))
                .is_none()
        );
        for (parameter, value, text) in [
            (ratio, 2.0 + operator as f32, format!("{}", 2 + operator)),
            (
                fixed + 1,
                880.0 + operator as f32,
                format!("{} hz", 880 + operator),
            ),
        ] {
            if parameter != ratio {
                let at = operator_context_selection_tests::row_background(
                    &mut visual,
                    device,
                    operator as usize,
                );
                visual.simulate_click(at, Modifiers::default());
                visual.run_until_parked();
                let toggle = visual
                    .debug_bounds(selector(format!(
                        "device-{}-parameter-{fixed}",
                        device.value()
                    )))
                    .unwrap();
                visual.simulate_click(toggle.center(), Modifiers::default());
                pump_until(&editor, cx, |view| {
                    view.worker_requests_pending == 0
                        && view.state.as_ref().is_some_and(|s| {
                            operator_parameter_value(s, track, device, fixed)
                                == ParameterValue::Bool(true)
                        })
                });
                visual.run_until_parked();
            }
            let original = editor.read_with(cx, |view, _| {
                operator_parameter_value(view.state.as_ref().unwrap(), track, device, parameter)
            });
            let field = visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{parameter}-value",
                    device.value()
                )))
                .unwrap();
            visual.simulate_click(field.center(), Modifiers::default());
            visual.simulate_keystrokes("cmd-a");
            visual.simulate_input(&text);
            visual.simulate_keystrokes("enter");
            pump_until(&editor, cx, |view| {
                view.worker_requests_pending == 0
                    && view.state.as_ref().is_some_and(|s| {
                        operator_parameter_value(s, track, device, parameter)
                            == ParameterValue::Float(value)
                    })
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
            pump_until(&editor, cx, |view| {
                view.worker_requests_pending == 0
                    && view.state.as_ref().is_some_and(|s| {
                        operator_parameter_value(s, track, device, parameter) == original
                    })
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
            pump_until(&editor, cx, |view| {
                view.worker_requests_pending == 0
                    && view.state.as_ref().is_some_and(|s| {
                        operator_parameter_value(s, track, device, parameter)
                            == ParameterValue::Float(value)
                    })
            });
            visual.run_until_parked();
        }
    }
}
