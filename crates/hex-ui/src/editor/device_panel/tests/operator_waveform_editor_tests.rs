use super::*;

fn bounds(visual: &mut VisualTestContext, selector: String) -> Bounds<Pixels> {
    visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .unwrap()
}

#[gpui::test]
fn waveform_panel_keeps_the_envelope_stable_and_obeys_native_edit_barriers(
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
    visual.simulate_resize(size(px(2000.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let envelope = visual.debug_bounds("operator-envelope-graph").unwrap();
    let closed = bounds(&mut visual, format!("device-drop-card-{}", device.value()));
    let next_closed = bounds(&mut visual, format!("device-drop-card-{}", next.value()));
    let primary = bounds(&mut visual, format!("operator-preview-{}", device.value()));
    let wave = bounds(
        &mut visual,
        format!("operator-page-{}-oscillator", device.value()),
    );
    visual.simulate_click(wave.center(), Modifiers::default());
    visual.run_until_parked();
    let panel = bounds(
        &mut visual,
        format!("operator-waveform-editor-{}", device.value()),
    );
    assert_eq!(
        panel.size.width,
        px(crate::device_composition::VECTOR_WAVEFORM_WIDTH)
    );
    assert_eq!(
        panel.size.height,
        bounds(&mut visual, format!("operator-detail-{}", device.value()))
            .size
            .height
    );
    assert!(panel.size.height <= px(crate::device_composition::BODY_HEIGHT));
    assert_eq!(
        visual.debug_bounds("operator-envelope-graph").unwrap(),
        envelope
    );
    assert!(envelope.right() <= panel.left());
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("operator-harmonics-{}-0", device.value()).into_boxed_str()
            ))
            .is_some()
    );
    let opened = bounds(&mut visual, format!("device-drop-card-{}", device.value()));
    assert_eq!(opened.size.width - closed.size.width, panel.size.width);
    assert_eq!(
        bounds(&mut visual, format!("device-drop-card-{}", next.value())).left()
            - next_closed.left(),
        panel.size.width
    );
    assert_eq!(
        bounds(&mut visual, format!("operator-preview-{}", device.value())).left() - primary.left(),
        panel.size.width
    );
    assert_eq!(
        bounds(&mut visual, format!("instrument-device-{}", device.value()))
            .size
            .height,
        px(200.0)
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        bounds(&mut visual, format!("device-drop-card-{}", device.value())),
        closed
    );
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    let row = operator_context_selection_tests::row_background(&mut visual, device, 3);
    visual.simulate_click(row, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("operator-harmonics-{}-3", device.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert_eq!(
        visual.debug_bounds("operator-envelope-graph").unwrap(),
        envelope
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Oscillator
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            3
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.worker_requests_pending, 0);
    });
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(1),
        "invalid",
    );
    visual.simulate_click(wave.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Oscillator
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    let field = bounds(
        &mut visual,
        format!("device-{}-parameter-1-value", device.value()),
    );
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_click(wave.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("operator-waveform-editor-{}", device.value()).into_boxed_str()
            ))
            .is_none()
    );
    assert!(visual.debug_bounds("operator-envelope-graph").is_some());
}

fn cycle_points(
    editor: &Entity<EditorView>,
    cx: &TestAppContext,
    device: DeviceId,
) -> Vec<[f32; 2]> {
    editor.read_with(cx, |view, _| {
        operator_panel::harmonics::displayed_cycle(view, device, 0)
            .unwrap()
            .to_vec()
    })
}

#[gpui::test]
fn waveform_cycle_updates_from_real_partial_edits_and_undo(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(2000.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let wave = bounds(
        &mut visual,
        format!("operator-page-{}-oscillator", device.value()),
    );
    visual.simulate_click(wave.center(), Modifiers::default());
    visual.run_until_parked();
    let before = cycle_points(&editor, cx, device);
    assert!(
        !before.is_empty(),
        "mounted native cycle has its actual display data"
    );
    let painter = bounds(
        &mut visual,
        format!("operator-harmonics-{}-0", device.value()),
    );
    visual.simulate_click(
        point(
            painter.left() + px(2.0),
            painter.top() + painter.size.height * 0.75,
        ),
        Modifiers::default(),
    );
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 57) == ParameterValue::Choice(4)
            })
    });
    visual.run_until_parked();
    assert_ne!(
        cycle_points(&editor, cx, device),
        before,
        "native partial edits update the displayed cycle data"
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.device(device).unwrap().state() == &original)
    });
    visual.run_until_parked();
    assert_eq!(
        cycle_points(&editor, cx, device),
        before,
        "Undo restores the original native cycle data"
    );
    assert!(visual.debug_bounds("operator-envelope-graph").is_some());
}
