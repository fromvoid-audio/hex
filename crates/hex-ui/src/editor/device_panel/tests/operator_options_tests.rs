use super::*;

fn bounds(visual: &mut VisualTestContext, prefix: &str, device: DeviceId) -> Bounds<Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("{prefix}-{}", device.value()).into_boxed_str(),
        ))
        .unwrap()
}

#[gpui::test]
fn optional_oscillator_controls_resize_the_real_rail_and_commit_native_history(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let next = state.project.track(track).unwrap().devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1760.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let closed = bounds(&mut visual, "device-drop-card", device);
    let next_closed = bounds(&mut visual, "device-drop-card", next);
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("operator-options-panel-{}", device.value()).into_boxed_str()
            ))
            .is_none()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-3-value", device.value()).into_boxed_str()
            ))
            .is_none()
    );
    assert_eq!(
        bounds(&mut visual, "instrument-device", device).size.height,
        px(200.0)
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-81-slider", device.value()).into_boxed_str()
            ))
            .is_some(),
        "feedback is primary"
    );
    open_operator_options(&mut visual, device);
    let opened = bounds(&mut visual, "device-drop-card", device);
    let next_opened = bounds(&mut visual, "device-drop-card", next);
    assert_eq!(
        opened.size.width - closed.size.width,
        px(crate::device_composition::VECTOR_OPTIONS_WIDTH)
    );
    assert_eq!(
        next_opened.left() - next_closed.left(),
        px(crate::device_composition::VECTOR_OPTIONS_WIDTH)
    );
    let panel = bounds(&mut visual, "operator-options-panel", device);
    assert_eq!(panel.size.width, px(80.0));
    assert!(panel.size.height <= px(198.0));
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.worker_requests_pending, 0);
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        bounds(&mut visual, "device-drop-card", device).size.width,
        closed.size.width
    );
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    assert_eq!(
        bounds(&mut visual, "device-drop-card", device).size.width,
        opened.size.width
    );
    let phase = hex_lattice_contract::ParameterId::from_static(3);
    enter_typed_value(&mut visual, device, phase, "25");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 3),
            ParameterValue::Float(0.25)
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Envelope
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.tracks(),
            original.tracks()
        )
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 3),
            ParameterValue::Float(0.25)
        )
    });
}

#[gpui::test]
fn invalid_option_drafts_refuse_hiding_and_stale_owners_cannot_toggle(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1760.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    open_operator_options(&mut visual, device);
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(3),
        "invalid",
    );
    let button = bounds(&mut visual, "operator-options", device);
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.operator_pages[&device].options_open);
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.worker_requests_pending, 0);
    });
    editor.update(cx, |view, cx| {
        let old = operator_panel::context::Owner::capture(view, device).unwrap();
        view.project_generation += 1;
        assert!(!operator_panel::context::toggle_options(view, cx, old));
        assert!(view.device_panel.operator_pages[&device].options_open);
    });
}
