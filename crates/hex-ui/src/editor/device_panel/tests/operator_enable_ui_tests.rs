use super::*;

#[gpui::test]
fn operator_enable_badges_commit_canonical_history_and_reopen(cx: &mut TestAppContext) {
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: hex_engine::DEVICE_OUTPUT_GAIN.id,
            value: ParameterValue::Float(-12.345),
        })
        .unwrap();
    let trimmed = receive_service_event(&service);
    assert!(trimmed.error.is_none());
    assert_eq!(
        trimmed
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .output_gain_millidb(),
        -12_345
    );
    let path = std::env::temp_dir().join(format!(
        "hex-enable-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    service.submit(Request::SaveNew(path.clone())).unwrap();
    let saved = receive_service_event(&service);
    assert!(saved.error.is_none());
    let (handle, editor) = actor_editor_window(cx, service, saved.state.clone(), track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for operator in 0..4 {
        let selector =
            Box::leak(format!("operator-select-{}-{operator}", device.value()).into_boxed_str());
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 513 + operator)
                        == ParameterValue::Bool(false)
                })
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&device].selected_operator, 0,
                "power edits preserve the explicitly selected envelope"
            );
            assert_eq!(
                view.device_panel.operator_pages[&device].page,
                // Existing navigation keeps the open envelope page while
                // selecting the matching oscillator and changing its enable.
                operator_panel::OperatorPage::Envelope
            );
        });
        visual.run_until_parked();
    }
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 516) == ParameterValue::Bool(true)
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 516) == ParameterValue::Bool(false)
            })
    });
    editor.update(cx, |view, cx| view.save_project(cx));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && !view.saving_project
            && view.state.as_ref().is_some_and(|state| !state.unsaved)
    });
    let reload = Service::new(48_000, 8, 8_000_000).unwrap();
    let _ = receive_service_event(&reload);
    reload.submit(Request::Open(path.clone())).unwrap();
    let reopened = receive_service_event(&reload);
    assert!(reopened.error.is_none());
    assert_eq!(
        reopened
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .output_gain_millidb(),
        -12_345
    );
    for operator in 0..4 {
        assert_eq!(
            operator_parameter_value(&reopened.state, track, device, 513 + operator),
            ParameterValue::Bool(false)
        );
        let authored = operator * 3 + 2;
        // The badge edit preserves authored levels while disabling DSP.
        assert_eq!(
            operator_parameter_value(&reopened.state, track, device, authored),
            operator_parameter_value(&saved.state, track, device, authored)
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn operator_enable_rapid_clicks_use_pending_value_and_queue_refusal_restores_it(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        assert!(operator_panel::toggle_enable(view, device, 0));
        assert!(operator_panel::toggle_enable(view, device, 0));
        let key = ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(513),
        };
        assert_eq!(
            view.device_panel.pending_values[&key],
            ParameterValue::Bool(true)
        );
        while view.pending_requests.len() < crate::editor::UI_REQUEST_CAPACITY {
            view.pending_requests.push_back(Request::Undo);
        }
        assert!(!operator_panel::toggle_enable(view, device, 0));
        assert_eq!(
            view.device_panel.pending_values[&key],
            ParameterValue::Bool(true)
        );
        assert!(!operator_panel::toggle_enable(view, device, 4));
        view.pending_requests.clear();
        view.device_panel.pending_values.clear();
    });
}

#[gpui::test]
fn oscillator_power_keyboard_and_pointer_preserve_selection(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let power = editor.read_with(cx, |view, _| {
        view.device_panel.operator_navigation_focus
            [&(device, operator_panel::NavigationTarget::Oscillator(3))]
            .clone()
    });
    visual.update(|window, _| power.focus(window));
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.revision == revision + 1)
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 516),
            ParameterValue::Bool(false)
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            0
        );
    });
    visual.run_until_parked();
    let background =
        super::operator_context_selection_tests::row_background(&mut visual, device, 2);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            2
        );
        assert_eq!(view.state.as_ref().unwrap().revision, revision + 1);
    });
    let selector = Box::leak(format!("operator-select-{}-3", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.revision == revision + 2)
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            2
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.revision == revision + 3)
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 516),
            ParameterValue::Bool(false)
        )
    });
}
