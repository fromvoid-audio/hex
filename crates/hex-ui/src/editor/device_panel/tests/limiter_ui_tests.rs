use super::*;

#[gpui::test]
fn limiter_compact_panel_fits_track_and_return_and_keeps_edits_undoable(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "limiter track".into(),
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
                "hex.limiter",
            )),
            before: None,
        })
        .expect("track limiter");
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let original = state.project.track(track).expect("track").devices()[0]
        .state()
        .clone();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "limiter return".into(),
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
        .expect("return limiter");
    let state = receive_service_event(&service).state;
    let return_device = state.project.return_tracks()[0].devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let assert_limiter_bounds = |visual: &mut VisualTestContext, device: DeviceId| {
        let frame = visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", device.value()).into_boxed_str(),
            ))
            .expect("limiter frame");
        let body = visual
            .debug_bounds(Box::leak(
                format!("device-body-{}", device.value()).into_boxed_str(),
            ))
            .expect("limiter body");
        assert_eq!(
            frame.size.width,
            px(limiter_panel::FRAME_WIDTH + device_meter::WIDTH)
        );
        assert_eq!(frame.size.height, px(DEVICE_FRAME_HEIGHT));
        for parameter in [1, 2, 3, 4, 5, 6, 7, 8, 9] {
            let control = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", device.value()).into_boxed_str(),
                ))
                .expect("shared compact dial");
            assert!(control.origin.x >= body.origin.x);
            assert!(control.origin.y >= body.origin.y);
            assert!(control.origin.x + control.size.width <= body.origin.x + body.size.width);
            assert!(control.origin.y + control.size.height <= body.origin.y + body.size.height);
        }
        (frame, body)
    };
    let (track_frame, _) = assert_limiter_bounds(&mut visual, device);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    let (return_frame, _) = assert_limiter_bounds(&mut visual, return_device);
    assert_eq!(return_frame.size, track_frame.size);

    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    let bounds = |visual: &mut VisualTestContext, suffix: &str| {
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{suffix}", device.value()).into_boxed_str(),
            ))
            .unwrap()
    };
    let input = bounds(&mut visual, "1");
    let ceiling = bounds(&mut visual, "2");
    let release = bounds(&mut visual, "3");
    let lookahead = bounds(&mut visual, "6-knob");
    assert!(input.right() < ceiling.left());
    assert!(release.top() > input.bottom());
    assert!(lookahead.left() > ceiling.right());
    for suffix in [
        "8-option-0",
        "8-option-1",
        "8-option-2",
        "7-option-0",
        "7-option-1",
    ] {
        bounds(&mut visual, suffix);
    }
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(2),
        "-6",
    );
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    visual.run_until_parked();
    let start = bounds(&mut visual, "2-slider-thumb").center();
    let end = point(start.x, start.y + px(15.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        let state = view
            .state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state();
        let ParameterValue::Float(value) = hex_engine::native_device_parameter(
            state,
            hex_lattice_contract::ParameterId::from_static(2),
        )
        .unwrap() else {
            panic!("ceiling")
        };
        assert!(
            (value + 9.0).abs() < 0.01,
            "triangle edits the real ceiling"
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    let key = ParameterKey {
        device,
        parameter: hex_lattice_contract::ParameterId::from_static(2),
    };
    editor.read_with(cx, |view, app| {
        let Some(ParameterControl::Float { entity, .. }) = view.device_panel.controls.get(&key)
        else {
            panic!("ceiling entity")
        };
        assert_eq!(entity.read(app).value(), -6.0);
    });
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(1),
        "12",
    );
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
            && view.device_gesture_barriers.is_empty()
    });
    editor.read_with(cx, |view, _| {
        let current = view
            .state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state();
        assert_eq!(
            hex_engine::native_device_parameter(
                current,
                hex_lattice_contract::ParameterId::from_static(1)
            )
            .unwrap(),
            ParameterValue::Float(12.0)
        );
    });
    let maximize = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-9", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_mouse_down(maximize.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(maximize.center(), MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    for parameter in [10, 11] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", device.value()).into_boxed_str()
                ))
                .is_some(),
            "maximize reveals real threshold/output bindings"
        );
    }
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-1", device.value()).into_boxed_str()
            ))
            .is_some()
    );
    let routing = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-7", device.value()).into_boxed_str(),
        ))
        .unwrap();
    // Both routing options are always visible; there is no dropdown.
    assert!(routing.size.width > px(0.0));
    visual.run_until_parked();
    let ms = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-7-option-1", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(ms.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    let ms_link = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-12", device.value()).into_boxed_str(),
        ))
        .expect("actual m/s link");
    let body = visual
        .debug_bounds(Box::leak(
            format!("device-body-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(
        ms_link.origin.x >= body.origin.x
            && ms_link.origin.x + ms_link.size.width <= body.origin.x + body.size.width
    );
    assert!(
        ms_link.origin.y >= body.origin.y
            && ms_link.origin.y + ms_link.size.height <= body.origin.y + body.size.height
    );
    let auto = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-4", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(auto.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    let release = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-3-knob", device.value()).into_boxed_str(),
        ))
        .unwrap();
    let start = release.center();
    let end = point(start.x, start.y - px(20.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let entry = view.state.as_ref().unwrap().project.device(device).unwrap();
        let metadata =
            hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, entry.state())
                .unwrap();
        let release = metadata
            .parameters
            .iter()
            .find(|p| p.descriptor.id == hex_lattice_contract::ParameterId::from_static(3))
            .unwrap();
        assert!(!release.enabled);
        assert_eq!(
            release.value,
            ParameterValue::Float(100.0),
            "disabled manual release ignores the drag"
        );
        assert!(view.device_gesture.snapshot().is_none());
    });
}

#[gpui::test]
fn limiter_ceiling_keeps_released_triangle_and_readout_during_delayed_ack(cx: &mut TestAppContext) {
    let (project, track, _, _) = crate::editor::clip_drag_ui_test::midi_project();
    let mut controller = Editor::new(project, 8).unwrap();
    let state = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    controller
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let device = controller.project().track(track).unwrap().devices()[0].id();
    // The fixture deliberately does not tick the service: terminal responses remain
    // outstanding while completed repaints expose any stale canonical geometry.
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
    visual.run_until_parked();
    let selector =
        Box::leak(format!("device-{}-parameter-2-slider-thumb", device.value()).into_boxed_str());
    let initial = visual.debug_bounds(selector).unwrap();
    let start = initial.center();
    let end = point(start.x, start.y + px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let preview = visual.debug_bounds(selector).unwrap();
    assert!(preview.top() > initial.top());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector).unwrap(), preview);
        editor.read_with(cx, |view, app| {
            let key = ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(2),
            };
            let Some(ParameterControl::Float { entity, .. }) = view.device_panel.controls.get(&key)
            else {
                panic!("ceiling")
            };
            assert!((entity.read(app).value() + 5.1).abs() < 0.01);
            let canonical = view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state();
            assert_eq!(
                hex_engine::native_device_parameter(canonical, key.parameter).unwrap(),
                ParameterValue::Float(-0.3)
            );
        });
    }
}
