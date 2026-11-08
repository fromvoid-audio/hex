use super::*;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[gpui::test]
fn native_choice_popup_keys_own_input_before_editor_transport_and_launch_actions(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    assert_eq!(
        operator_parameter_value(&state, track, device, 601),
        ParameterValue::Choice(7)
    );
    let original = state.project.clone();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let link = visual
        .debug_bounds(selector(format!(
            "operator-preview-{}-route-link",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(link.center(), Modifiers::default());
    visual.run_until_parked();
    let field_id = selector(format!("device-{}-parameter-601", device.value()));
    let popup_id = selector(format!("device-{}-parameter-601-popup", device.value()));
    let field = visual.debug_bounds(field_id).unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_none());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(
            view.worker_requests_pending, 0,
            "popup cancel does not stop transport"
        );
        assert!(view.pending_requests.is_empty());
    });
    for key in ["enter", "space"] {
        visual.simulate_keystrokes(key);
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(popup_id).is_some(),
            "{key} opens the focused choice"
        );
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.worker_requests_pending, 0,
                "{key} does not launch or play"
            )
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
    }
    visual.simulate_keystrokes("enter");
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
    });
    assert!(visual.debug_bounds(popup_id).is_some());
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 601) == ParameterValue::Choice(6)
            })
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision + 1);
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Routing
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project == original)
    });
}
