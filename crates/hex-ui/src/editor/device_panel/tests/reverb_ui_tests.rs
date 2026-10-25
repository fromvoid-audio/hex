use super::*;

#[gpui::test]
fn reverb_compact_controls_fit_track_and_return_and_share_edit_lifecycle(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "reverb track".into(),
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
                "hex.reverb",
            )),
            before: None,
        })
        .expect("track reverb");
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let original = state.project.track(track).expect("track").devices()[0]
        .state()
        .clone();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "reverb return".into(),
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
        .expect("return reverb");
    let state = receive_service_event(&service).state;
    let return_device = state.project.return_tracks()[0].devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let assert_panel = |visual: &mut VisualTestContext, device: DeviceId| {
        let selector = Box::leak(format!("effect-device-{}", device.value()).into_boxed_str());
        let frame = visual.debug_bounds(selector).expect("reverb frame");
        let body = visual
            .debug_bounds(Box::leak(
                format!("device-body-{}", device.value()).into_boxed_str(),
            ))
            .expect("reverb body");
        assert_eq!(
            frame.size.width,
            px(reverb_panel::FRAME_WIDTH + gain_meters::WIDTH)
        );
        assert_eq!(frame.size.height, px(DEVICE_FRAME_HEIGHT));
        for parameter in 1..=2 {
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

    for (owner, id) in [(None, device), (Some(return_track), return_device)] {
        editor.update(cx, |view, cx| {
            view.focused_return = owner;
            cx.notify();
        });
        visual.run_until_parked();
        for (parameter, input, expected) in [(1, "2", 2.0), (2, "70", 70.0)] {
            let parameter = hex_lattice_contract::ParameterId::from_static(parameter);
            enter_typed_value(&mut visual, id, parameter, input);
            visual.simulate_keystrokes("enter");
            pump_until(&editor, cx, |view| {
                view.worker_requests_pending == 0
                    && view.pending_requests.is_empty()
                    && view.device_gesture_barriers.is_empty()
                    && view.device_gesture.snapshot().is_none()
            });
            editor.read_with(cx, |view, _| {
                let project = &view.state.as_ref().unwrap().project;
                let state = project.device(id).unwrap().state();
                let entry = hex_engine::native_device_view(state).unwrap();
                assert_eq!(
                    entry
                        .parameters
                        .iter()
                        .find(|p| p.descriptor.id == parameter)
                        .unwrap()
                        .value,
                    ParameterValue::Float(expected)
                );
            });
        }
        let changed = editor.read_with(cx, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(id)
                .unwrap()
                .state()
                .clone()
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        editor.read_with(cx, |view, _| {
            assert_ne!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(id)
                    .unwrap()
                    .state(),
                &changed
            )
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(id)
                    .unwrap()
                    .state(),
                &changed
            )
        });
    }
}
