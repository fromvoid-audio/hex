use super::*;

#[gpui::test]
fn filter_panel_exposes_controls_and_wet_response(cx: &mut TestAppContext) {
    let (project, track, _, _) = crate::editor::clip_drag_ui_test::midi_project();
    let type_id = hex_lattice_contract::DeviceTypeId::from_static("hex.filter");
    let mut controller = Editor::new(project, 8).unwrap();
    controller
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(type_id).unwrap(),
        })
        .unwrap();
    let device = controller.project().track(track).unwrap().devices()[0].id();
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        controller.project().clone(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let response = visual
        .debug_bounds(Box::leak(
            format!("filter-response-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(response.size.width >= px(100.0));
    assert!(response.size.height >= px(80.0));
    for parameter in [1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13] {
        let selector =
            Box::leak(format!("device-{}-parameter-{parameter}", device.value()).into_boxed_str());
        assert!(
            visual.debug_bounds(selector).is_some(),
            "filter parameter {parameter}"
        );
    }
}
