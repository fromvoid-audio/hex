use super::*;
use hex_lattice_contract::ParameterId;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[gpui::test]
fn primary_voices_preserve_loaded_counts_context_and_single_edit_history(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(601),
            value: ParameterValue::Choice(2),
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let initial_revision = state.revision;
    let mode = operator_parameter_value(&state, track, device, 608);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let field_id = selector(format!("device-{}-parameter-601", device.value()));
    let popup_id = selector(format!("device-{}-parameter-601-popup", device.value()));
    let field = visual
        .debug_bounds(field_id)
        .expect("voice selector on envelope page");
    let preview = visual
        .debug_bounds(selector(format!("operator-preview-{}", device.value())))
        .unwrap();
    assert!(preview.contains(&field.center()));
    assert!(field.bottom() <= preview.bottom());
    assert!(preview.size.height <= px(200.0));
    let entity = editor.read_with(cx, |view, cx| {
        let ParameterControl::Choice { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: ParameterId::from_static(601),
        }] else {
            panic!("voices selector")
        };
        assert_eq!(
            entity.read(cx).selected(),
            2,
            "loaded three-voice count is displayed without replacement"
        );
        entity.clone()
    });
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_some());
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_none());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
        assert_eq!(
            view.worker_requests_pending, 0,
            "popup Escape does not enqueue transport stop"
        );
        assert!(view.pending_requests.is_empty());
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    for _ in 0..5 {
        visual.simulate_keystrokes("down");
    }
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 601),
            ParameterValue::Choice(2)
        );
    });
    assert!(visual.debug_bounds(popup_id).is_some());
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 601) == ParameterValue::Choice(7)
            })
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision + 1);
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 608),
            mode
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Envelope
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            0
        );
    });
    for (request, value) in [(Request::Undo, 2), (Request::Redo, 7)] {
        editor.update(cx, |view, _| assert!(view.enqueue(request)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 601)
                        == ParameterValue::Choice(value)
                })
        });
    }
    visual.run_until_parked();
    let link = visual
        .debug_bounds(selector(format!(
            "operator-preview-{}-route-link",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(link.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let ParameterControl::Choice {
            entity: current, ..
        } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: ParameterId::from_static(601),
        }]
        else {
            panic!("voices")
        };
        assert_eq!(
            current.entity_id(),
            entity.entity_id(),
            "one cached owner across detail pages"
        );
    });
    let field = visual.debug_bounds(field_id).unwrap();
    let row = visual
        .debug_bounds(selector(format!("operator-voices-{}", device.value())))
        .unwrap();
    assert!(
        row.contains(&field.center()),
        "routing detail does not mount a second selector"
    );
}

#[gpui::test]
fn primary_voice_popup_cancels_outside_and_refuses_invalid_working_drafts(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let initial = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let field_id = selector(format!("device-{}-parameter-601", device.value()));
    let popup_id = selector(format!("device-{}-parameter-601-popup", device.value()));
    let field = visual.debug_bounds(field_id).unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_some());
    visual.simulate_keystrokes("up");
    visual.simulate_click(point(px(10.0), px(400.0)), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_none());
    let gain = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-11-value",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(gain.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-601-option-7",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(option.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().project, initial);
        assert!(view.error.is_some());
        let ParameterControl::Choice { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: ParameterId::from_static(601),
        }] else {
            panic!("voices")
        };
        assert_eq!(
            entity.read(cx).selected(),
            14,
            "refused value returns to canonical count"
        );
    });
}
