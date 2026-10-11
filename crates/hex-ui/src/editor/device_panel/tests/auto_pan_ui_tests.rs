use super::*;

#[gpui::test]
fn auto_pan_compact_controls_fit_track_and_return_and_share_edit_lifecycle(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "auto pan track".into(),
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
                "hex.auto-pan",
            )),
            before: None,
        })
        .expect("track auto pan");
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let original = state.project.track(track).expect("track").devices()[0]
        .state()
        .clone();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "auto pan return".into(),
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
        .expect("return auto pan");
    let state = receive_service_event(&service).state;
    let return_device = state.project.return_tracks()[0].devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let assert_panel = |visual: &mut VisualTestContext, device: DeviceId| {
        let selector = Box::leak(format!("effect-device-{}", device.value()).into_boxed_str());
        let frame = visual.debug_bounds(selector).expect("auto pan frame");
        let body = visual
            .debug_bounds(Box::leak(
                format!("device-body-{}", device.value()).into_boxed_str(),
            ))
            .expect("auto pan body");
        assert_eq!(frame.size.width, px(auto_pan_panel::FRAME_WIDTH));
        assert_eq!(frame.size.height, px(DEVICE_FRAME_HEIGHT));
        for parameter in 1..=3 {
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
    let (track_frame, _) = assert_panel(&mut visual, device);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    let (return_frame, _) = assert_panel(&mut visual, return_device);
    assert_eq!(return_frame.size, track_frame.size);

    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    let amount = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-knob", device.value()).into_boxed_str(),
        ))
        .expect("amount dial");
    let start = amount.center();
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
        assert_ne!(
            state.project.device(device).expect("auto pan").state(),
            &original
        );
        assert_ne!(
            parameter_value(
                state,
                track,
                device,
                hex_lattice_contract::ParameterId::from_static(1)
            ),
            ParameterValue::Float(0.5)
        );
        assert_eq!(
            parameter_value(
                state,
                track,
                device,
                hex_lattice_contract::ParameterId::from_static(2)
            ),
            ParameterValue::Float(1.0)
        );
        assert_eq!(
            parameter_value(
                state,
                track,
                device,
                hex_lattice_contract::ParameterId::from_static(3)
            ),
            ParameterValue::Float(180.0)
        );
        assert!(state.can_undo, "amount drag is one undoable edit");
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
                .device(device)
                .expect("auto pan")
                .state(),
            &original
        );
    });

    // Exercise every descriptor binding through the rendered typed-value control.
    for (parameter, input, expected) in [(1, "0.75", 0.75), (2, "2.5", 2.5), (3, "90", 90.0)] {
        let parameter = hex_lattice_contract::ParameterId::from_static(parameter);
        enter_typed_value(&mut visual, device, parameter, input);
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture_barriers.is_empty()
                && view.device_gesture.snapshot().is_none()
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                parameter_value(
                    view.state.as_ref().expect("state"),
                    track,
                    device,
                    parameter
                ),
                ParameterValue::Float(expected),
                "typed value reaches its canonical parameter"
            );
        });
    }

    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Edit(Command::RemoveDevice(device))));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .track(track)
                    .is_some_and(|track| track.devices().is_empty())
            })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .controls
                .keys()
                .all(|key| key.device != device),
            "removed owner retires all three shared controls"
        );
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", device.value()).into_boxed_str(),
            ))
            .is_none(),
        "removed frame is absent from the rendered tree"
    );

    // Removing the Return owner must retire the same shared controls.
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    enter_typed_value(
        &mut visual,
        return_device,
        hex_lattice_contract::ParameterId::from_static(3),
        "270",
    );
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        let device = state.project.return_tracks()[0].devices()[0].state();
        let parameters = hex_engine::native_device_view(device)
            .expect("return device")
            .parameters;
        assert_eq!(
            parameters
                .iter()
                .find(|p| p.descriptor.id.value() == 3)
                .expect("phase")
                .value,
            ParameterValue::Float(270.0)
        );
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveReturnTrack(return_track))))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.return_tracks().is_empty())
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .controls
                .keys()
                .all(|key| key.device != return_device)
        );
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", return_device.value()).into_boxed_str()
            ))
            .is_none()
    );
}
