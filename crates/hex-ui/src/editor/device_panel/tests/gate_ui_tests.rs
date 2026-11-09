use super::*;

#[gpui::test]
fn gate_compact_panel_fits_track_and_return_and_keeps_edits_undoable(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "gate track".into(),
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
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static("hex.gate")),
            before: None,
        })
        .expect("track gate");
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let original = state.project.track(track).expect("track").devices()[0]
        .state()
        .clone();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "gate return".into(),
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
        .expect("return gate");
    let state = receive_service_event(&service).state;
    let return_device = state.project.return_tracks()[0].devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let assert_gate_bounds = |visual: &mut VisualTestContext, device: DeviceId| {
        let frame = visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", device.value()).into_boxed_str(),
            ))
            .expect("gate frame");
        let body = visual
            .debug_bounds(Box::leak(
                format!("device-body-{}", device.value()).into_boxed_str(),
            ))
            .expect("gate body");
        assert_eq!(
            frame.size.width,
            px(gate_panel::FRAME_WIDTH
                + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
                + device_meter::WIDTH)
        );
        assert_eq!(frame.size.height, px(DEVICE_FRAME_HEIGHT));
        for parameter in 1..=6 {
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
    let (track_frame, _) = assert_gate_bounds(&mut visual, device);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    let (return_frame, _) = assert_gate_bounds(&mut visual, return_device);
    assert_eq!(return_frame.size, track_frame.size);

    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    let threshold = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-knob", device.value()).into_boxed_str(),
        ))
        .expect("threshold dial");
    let start = threshold.center();
    let moved = point(start.x, start.y - px(20.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
            && view.device_gesture_barriers.is_empty()
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        let current = state.project.device(device).expect("gate").state();
        assert_ne!(current, &original, "threshold drag commits canonically");
        assert!(
            state.can_undo,
            "gesture becomes one undoable project change"
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
                .expect("undo state")
                .project
                .device(device)
                .expect("gate")
                .state(),
            &original
        );
    });

    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(1),
        "-50",
    );
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .expect("state")
                .project
                .device(device)
                .expect("gate")
                .state(),
            &original
        );
        assert!(
            view.error
                .as_deref()
                .is_some_and(|error| error.contains("invalid")),
            "invalid threshold/return relation is surfaced"
        );
    });
}
