use super::*;

#[gpui::test]
fn overdrive_shared_panel_fits_and_drag_commits_one_undoable_edit(cx: &mut TestAppContext) {
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "overdrive".into(),
        }))
        .expect("track");
    let event = receive_service_event(&service);
    let Created::Track(track) = event.created.expect("track result") else {
        panic!("track")
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.overdrive",
            )),
            before: None,
        })
        .expect("device");
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let original = state.project.track(track).expect("track").devices()[0]
        .state()
        .clone();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "overdrive return".into(),
        }))
        .expect("return track");
    let event = receive_service_event(&service);
    let Created::ReturnTrack(return_track) = event.created.expect("return result") else {
        panic!("return track")
    };
    service
        .submit(Request::Edit(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: original.clone(),
        }))
        .expect("return overdrive");
    let state = receive_service_event(&service).state;
    let return_device = state.project.return_tracks()[0].devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = |name: &str| -> &'static str {
        Box::leak(format!("{name}-{}", device.value()).into_boxed_str())
    };
    let card = visual
        .debug_bounds(selector("effect-device"))
        .expect("card");
    let body = visual.debug_bounds(selector("device-body")).expect("body");
    let graph = visual
        .debug_bounds(selector("overdrive-filter"))
        .expect("transfer plot");
    assert_eq!(
        card.size.width,
        px(overdrive_panel::FRAME_WIDTH + device_meter::WIDTH)
    );
    assert_eq!(card.size.height, px(DEVICE_FRAME_HEIGHT));
    assert!(graph.size.height >= px(30.0));
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(
        visual
            .rendered_surface_geometry("device-plot-section")
            .expect("shared rounded transfer section")
            .0
            .top_left,
        px(hex_widgets::DEVICE_SECTION_RADIUS).scale(scale)
    );
    for parameter in 1..=6 {
        let control = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{parameter}", device.value()).into_boxed_str(),
            ))
            .expect("shared control");
        assert!(control.origin.y >= body.origin.y);
        assert!(control.origin.y + control.size.height <= body.origin.y + body.size.height);
        assert!(control.origin.x + control.size.width <= body.origin.x + body.size.width);
    }
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    let return_card = visual
        .debug_bounds(Box::leak(
            format!("effect-device-{}", return_device.value()).into_boxed_str(),
        ))
        .expect("return card");
    let return_graph = visual
        .debug_bounds(Box::leak(
            format!("overdrive-filter-{}", return_device.value()).into_boxed_str(),
        ))
        .expect("return transfer plot");
    assert_eq!(return_card.size, card.size);
    assert!(return_graph.size.height >= px(30.0));
    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    let drive_selector =
        Box::leak(format!("device-{}-parameter-1", device.value()).into_boxed_str());
    let drive = visual.debug_bounds(drive_selector).expect("drive knob");
    let start = drive.center();
    let moved = point(start.x, start.y - px(25.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let gesture = view
            .device_gesture
            .snapshot()
            .expect("active shared-control gesture");
        let expected = hex_engine::patch_native_device_parameter(
            &original,
            hex_lattice_contract::ParameterId::from_static(1),
            gesture.value,
        )
        .expect("drive draft");
        assert_eq!(
            overdrive_panel::cached_curve(view, device),
            Some(
                &hex_engine::native_overdrive_filter_response(&expected, 48_000)
                    .expect("draft curve")
            )
        );
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
            && view.device_gesture_barriers.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_ne!(
            view.state
                .as_ref()
                .expect("state")
                .project
                .track(track)
                .expect("track")
                .devices()[0]
                .state(),
            &original
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .expect("state")
                .project
                .track(track)
                .expect("track")
                .devices()[0]
                .state(),
            &original,
            "one undo restores the entire drag"
        );
    });
}

#[gpui::test]
fn overdrive_preview_uses_unacknowledged_draft_and_retires_on_owner_change(
    cx: &mut TestAppContext,
) {
    let (project, track, _, _) = crate::editor::clip_drag_ui_test::midi_project();
    let original = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.overdrive"),
    )
    .expect("state");
    let mut controller = Editor::new(project, 8).expect("editor");
    controller
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: original.clone(),
        })
        .expect("device");
    let device = controller.project().track(track).expect("track").devices()[0].id();
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
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let expected = hex_engine::patch_native_device_parameter(
        &original,
        hex_lattice_contract::ParameterId::from_static(1),
        ParameterValue::Float(2_400.0),
    )
    .expect("draft");
    editor.update(cx, |view, cx| {
        // The no-worker fixture deliberately holds acknowledgement outstanding.
        view.worker_requests_pending = 1;
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(1),
            },
            ParameterValue::Float(2_400.0),
        );
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .expect("state")
                .project
                .track(track)
                .expect("track")
                .devices()[0]
                .state(),
            &original
        );
        assert_eq!(
            overdrive_panel::cached_curve(view, device),
            Some(
                &hex_engine::native_overdrive_filter_response(&expected, 48_000)
                    .expect("draft preview")
            )
        );
    });
    editor.update(cx, |view, cx| {
        view.worker_requests_pending = 0;
        view.selected_track = None;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(overdrive_panel::cached_curve(view, device).is_none())
    });
}
