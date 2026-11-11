use super::*;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
fn fixture() -> (Service, Arc<ViewState>, TrackId, DeviceId) {
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(20_001),
            value: ParameterValue::Bool(true),
        })
        .unwrap();
    let event = receive_service_event(&service);
    assert!(event.error.is_none());
    (service, event.state, track, device)
}
fn select_filter(visual: &mut VisualTestContext, device: DeviceId) {
    let left = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-20001",
            device.value()
        )))
        .unwrap();
    let right = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-20002",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(
        point((left.right() + right.left()) / 2.0, left.center().y),
        Modifiers::default(),
    );
    visual.run_until_parked();
}
fn displayed(view: &EditorView, device: DeviceId) -> Vec<[f32; 2]> {
    operator_panel::filter_response::displayed(view, device)
        .unwrap()
        .0
        .to_vec()
}

#[gpui::test]
fn native_filter_display_fits_reuses_cache_and_follows_real_control_history(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(800.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    select_filter(&mut visual, device);
    let graph = visual.debug_bounds("operator-filter-response").unwrap();
    let body = visual
        .debug_bounds(selector(format!("operator-body-{}", device.value())))
        .unwrap();
    assert!(body.size.height <= px(200.0));
    assert!(
        graph.size.height >= px(60.0)
            && graph.top() >= body.top()
            && graph.bottom() <= body.bottom()
    );
    let curve = editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        let curve = displayed(view, device);
        assert_eq!(curve.len(), 128);
        assert!(curve[0][1] > curve[127][1]);
        curve
    });
    let cache = editor.read_with(&visual, |view, _| {
        operator_panel::filter_response::displayed(view, device)
            .unwrap()
            .0
            .as_ptr() as usize
    });
    editor.update(&mut visual, |_, cx| cx.notify());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            operator_panel::filter_response::displayed(view, device)
                .unwrap()
                .0
                .as_ptr() as usize,
            cache
        )
    });
    let field = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-20005-value",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("250 hz");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && matches!(operator_parameter_value(view.state.as_ref().unwrap(),track,device,20005),ParameterValue::Float(v) if v==250.0)
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_ne!(displayed(view, device), curve)
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && matches!(operator_parameter_value(view.state.as_ref().unwrap(),track,device,20005),ParameterValue::Float(v) if v==1000.0)
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(displayed(view, device), curve)
    });
    let at = operator_context_selection_tests::row_background(&mut visual, device, 0);
    visual.simulate_click(at, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(operator_panel::filter_response::displayed(view, device).is_none())
    });
    assert!(visual.debug_bounds("operator-filter-response").is_none());
    select_filter(&mut visual, device);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveInstrument { track })));
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
    editor.read_with(&visual, |view, _| {
        assert!(operator_panel::filter_response::displayed(view, device).is_none());
    });
    assert!(visual.debug_bounds("operator-filter-response").is_none());
}

#[gpui::test]
fn filter_curve_keeps_released_parameter_preview_across_completed_repaints(
    cx: &mut TestAppContext,
) {
    let (_service, state, track, device) = fixture();
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        (*state.project).clone(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(800.0)));
    visual.run_until_parked();
    select_filter(&mut visual, device);
    let before = editor.read_with(&visual, |view, _| displayed(view, device));
    let knob = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-20005-knob",
            device.value()
        )))
        .unwrap();
    let at = knob.center();
    let moved = point(at.x, at.y - px(20.0));
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = editor.read_with(&visual, |view, _| displayed(view, device));
    assert_ne!(preview, before);
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(displayed(view, device), preview);
            assert_eq!(
                operator_parameter_value(view.state.as_ref().unwrap(), track, device, 20005),
                ParameterValue::Float(1000.0)
            );
            assert!(view.device_gesture.has_pending_terminal());
        });
    }
}
