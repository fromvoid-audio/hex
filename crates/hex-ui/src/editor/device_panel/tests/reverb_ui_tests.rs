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
    visual.simulate_resize(size(px(1920.0), px(1080.0)));
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
        for parameter in 1..=33 {
            let control = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", device.value()).into_boxed_str(),
                ))
                .expect("bound native control");
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
            visual.run_until_parked();
            let parameter = hex_lattice_contract::ParameterId::from_static(parameter);
            let selector = Box::leak(
                format!(
                    "device-{}-parameter-{}-value",
                    id.value(),
                    parameter.value()
                )
                .into_boxed_str(),
            );
            if let Some(viewport) = visual.debug_bounds("clip-detail-viewport") {
                let bounds = visual.debug_bounds(selector).unwrap();
                let delta = if bounds.bottom() > viewport.bottom() {
                    viewport.bottom() - bounds.bottom() - px(2.0)
                } else if bounds.top() < viewport.top() {
                    viewport.top() - bounds.top() + px(2.0)
                } else {
                    px(0.0)
                };
                visual.simulate_event(gpui::ScrollWheelEvent {
                    position: point(viewport.right() - px(1.0), viewport.center().y),
                    delta: gpui::ScrollDelta::Pixels(point(px(0.0), delta)),
                    ..Default::default()
                });
                visual.run_until_parked();
                assert!(viewport.contains(&visual.debug_bounds(selector).unwrap().center()));
            }
            if let Some(rail) = visual.debug_bounds("device-chain-scroll") {
                let bounds = visual.debug_bounds(selector).unwrap();
                let delta = if bounds.right() > rail.right() {
                    rail.right() - bounds.right() - px(2.0)
                } else if bounds.left() < rail.left() {
                    rail.left() - bounds.left() + px(2.0)
                } else {
                    px(0.0)
                };
                visual.simulate_event(gpui::ScrollWheelEvent {
                    position: rail.center(),
                    delta: gpui::ScrollDelta::Pixels(point(delta, px(0.0))),
                    ..Default::default()
                });
                visual.run_until_parked();
                assert!(
                    rail.contains(&visual.debug_bounds(selector).unwrap().center()),
                    "numeric field must be inside the chain's actual hit mask"
                );
            }
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

        visual.run_until_parked();
        let freeze = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-26", id.value()).into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_click(freeze.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        for parameter in [24, 25] {
            visual.run_until_parked();
            let selector = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", id.value()).into_boxed_str(),
                ))
                .unwrap();
            visual.simulate_click(selector.center(), Modifiers::default());
            visual.run_until_parked();
            let option = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}-option-2", id.value())
                        .into_boxed_str(),
                ))
                .unwrap();
            visual.simulate_click(option.center(), Modifiers::default());
            pump_until(&editor, cx, |view| {
                view.worker_requests_pending == 0 && view.pending_requests.is_empty()
            });
        }
        editor.read_with(cx, |view, _| {
            let metadata = hex_engine::native_device_view(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(id)
                    .unwrap()
                    .state(),
            )
            .unwrap();
            for (parameter, value) in [
                (26, ParameterValue::Bool(true)),
                (24, ParameterValue::Choice(2)),
                (25, ParameterValue::Choice(2)),
            ] {
                assert_eq!(
                    metadata
                        .parameters
                        .iter()
                        .find(|p| p.descriptor.id.value() == parameter)
                        .unwrap()
                        .value,
                    value
                );
            }
        });
    }
    let expected = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let path = std::env::temp_dir().join(format!(
        "hex-reverb-controls-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!path.exists());
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| assert!(view.error.is_none()));
    let reload = Service::new(48000, 8, 8_000_000).unwrap();
    let _ = receive_service_event(&reload);
    reload.submit(Request::Open(path.clone())).unwrap();
    let reopened = receive_service_event(&reload);
    assert!(reopened.error.is_none());
    assert_eq!(reopened.state.project, expected);
    drop(reload);
    std::fs::remove_file(path).unwrap();
}
