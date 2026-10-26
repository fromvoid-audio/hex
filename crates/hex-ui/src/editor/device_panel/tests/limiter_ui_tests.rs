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
            px(limiter_panel::FRAME_WIDTH + gain_meters::WIDTH)
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
    visual.simulate_click(routing.center(), Modifiers::default());
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
