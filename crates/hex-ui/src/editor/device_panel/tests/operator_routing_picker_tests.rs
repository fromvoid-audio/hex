use super::*;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
fn popup(device: DeviceId) -> &'static str {
    selector(format!("operator-routing-popup-{}", device.value()))
}
fn link(device: DeviceId) -> &'static str {
    selector(format!("operator-preview-{}-route-link", device.value()))
}

#[gpui::test]
fn popup_pointer_choice_refuses_full_queue_and_tab_retires_the_popup(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (_service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, (*original).clone(), Arc::default());
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let field = visual.debug_bounds(link(device)).unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup(device)).is_none());
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds(selector(format!(
            "operator-routing-option-{}-10",
            device.value()
        )))
        .unwrap();
    editor.update(&mut visual, |view, _| {
        view.worker_requests_pending = 1;
        while view.pending_requests.len() < crate::editor::UI_REQUEST_CAPACITY {
            view.pending_requests.push_back(Request::Undo);
        }
    });
    visual.simulate_click(option.center(), Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(
            !view
                .device_panel
                .pending_values
                .contains_key(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(600)
                })
        );
        assert!(
            view.device_panel.operator_previews[&device]
                .routing_popup
                .is_none()
        );
        view.pending_requests.clear();
        view.worker_requests_pending = 0;
    });
}

#[gpui::test]
fn diagrams_stage_keyboard_choices_without_context_or_transport_edits_and_commit_one_history_step(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let source = state.project.track(track).unwrap().source_id().unwrap();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let field = visual.debug_bounds(link(device)).unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    let bounds = visual.debug_bounds(popup(device)).unwrap();
    assert!(bounds.left() >= px(0.0) && bounds.right() <= px(1440.0));
    assert!(bounds.top() >= px(0.0) && bounds.bottom() <= px(540.0));
    assert!(bounds.size.width <= px(180.0) && bounds.size.height <= px(120.0));
    for index in 0..11 {
        let option = selector(format!(
            "operator-routing-option-{}-{index}",
            device.value()
        ));
        let tile = visual.debug_bounds(option).unwrap();
        assert!(bounds.contains(&tile.center()));
        let quads = visual.rendered_filled_quads(option);
        for operator in 0..4 {
            let color =
                gpui::Background::from(rgb(theme::oscillator_identity_color(operator).unwrap()));
            assert!(
                quads
                    .iter()
                    .any(|(node, fill)| *fill == color && f64::from(node.size.width) >= 3.0)
            );
        }
    }
    visual.simulate_keystrokes("right");
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(
            view.device_panel.operator_previews[&device].routing_popup,
            Some(2)
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Envelope
        );
        assert!(view.last_transport_command.is_none());
        assert_eq!(view.worker_requests_pending, 0);
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup(device)).is_none());
    visual.simulate_keystrokes("space");
    visual.simulate_keystrokes("right");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && operator_parameter_value(view.state.as_ref().unwrap(), track, device, 600)
                == ParameterValue::Choice(2)
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup(device)).is_none());
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.state.as_ref().unwrap().project == original
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && operator_parameter_value(view.state.as_ref().unwrap(), track, device, 600)
                == ParameterValue::Choice(2)
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup(device)).is_some());
    let outside = operator_context_selection_tests::row_background(&mut visual, device, 1);
    visual.simulate_click(outside, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup(device)).is_none());
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveInstrument { track })))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup(device)).is_none());
    editor.read_with(&visual, |view, _| {
        let preview = view
            .device_panel
            .operator_previews
            .get(&device)
            .expect("retained source preview");
        assert_eq!(preview.routing_popup, None);
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::BindTrackSource {
            track,
            source: Some(source)
        })))
    });
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .source_id()
            == Some(source)
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(popup(device)).is_none(),
        "old popup must not reopen after rebinding"
    );
}
