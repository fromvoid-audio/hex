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
        assert_eq!(
            frame.size.width,
            px(auto_pan_panel::FRAME_WIDTH + gain_meters::WIDTH)
        );
        assert_eq!(frame.size.height, px(DEVICE_FRAME_HEIGHT));
        for parameter in 1..=4 {
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

#[gpui::test]
fn auto_pan_waveform_menu_track_return_cancel_commit_preview_history_and_reopen(
    cx: &mut TestAppContext,
) {
    let (service, _, track, return_track, return_device, _) = actor_return_utility_fixture();
    let original = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.auto-pan"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device: return_device,
            state: original.clone(),
        }))
        .unwrap();
    assert!(receive_service_event(&service).error.is_none());
    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: original.clone(),
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).unwrap().devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    for (owner, device) in [(None, device), (Some(return_track), return_device)] {
        editor.update(cx, |view, cx| {
            view.focused_return = owner;
            cx.notify();
        });
        visual.run_until_parked();
        let selector = Box::leak(format!("device-{}-parameter-4", device.value()).into_boxed_str());
        let graph = visual
            .debug_bounds(Box::leak(
                format!("auto-pan-waveform-{}", device.value()).into_boxed_str(),
            ))
            .expect("actual provider-backed plot");
        assert!(graph.size.width > px(30.0) && graph.size.height > px(40.0));
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        let position = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(position, Modifiers::default());
        visual.run_until_parked();
        let option =
            Box::leak(format!("device-{}-parameter-4-option-1", device.value()).into_boxed_str());
        let option_bounds = visual
            .debug_bounds(option)
            .expect("actual waveform options");
        visual.simulate_mouse_move(option_bounds.center(), None, Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(option).is_none(),
            "dismiss selector without applying hovered option"
        );
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, before)
        });
        let position = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(position, Modifiers::default());
        visual.run_until_parked();
        let position = visual.debug_bounds(option).unwrap().center();
        visual.simulate_click(position, Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        let changed = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        assert_ne!(changed, before);
        let changed_state = changed.device(device).unwrap().state();
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                changed_state,
                hex_lattice_contract::ParameterId::from_static(4)
            )
            .unwrap(),
            ParameterValue::Choice(1)
        );
        editor.update(cx, |view, _| {
            let device_view = hex_engine::native_device_view(changed_state).unwrap();
            let plotted = auto_pan_panel::preview(view, device, &device_view).unwrap();
            assert_eq!(
                plotted,
                hex_engine::native_auto_pan_waveform_preview(changed_state).unwrap()
            );
            assert_eq!(
                plotted.left[16],
                [0.125, 0.875],
                "triangle eighth-cycle gain, not sine"
            );
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project,
                before,
                "one Undo covers selected option"
            )
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, changed)
        });
    }
    let expected = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let path = std::env::temp_dir().join(format!(
        "hex-auto-pan-shape-{}-{}.hex",
        std::process::id(),
        device.value()
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
    for id in [device, return_device] {
        assert_eq!(
            reopened.state.project.device(id).unwrap().state(),
            expected.device(id).unwrap().state()
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn auto_pan_preview_uses_effective_pending_choice_without_processing_in_paint(
    cx: &mut TestAppContext,
) {
    let (service, _, track, _, device, _) = actor_return_utility_fixture();
    let original = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.auto-pan"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: original.clone(),
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let return_id = state.project.return_tracks()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let visual = VisualTestContext::from_window(handle, cx);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_id);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        // Hold a real pending-value lifecycle: idle state intentionally retires
        // completed drafts through the shared binding adapter.
        view.worker_requests_pending = 1;
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(4),
            },
            ParameterValue::Choice(4),
        );
        let device_view = hex_engine::native_device_view(&original).unwrap();
        let preview = auto_pan_panel::preview(view, device, &device_view).unwrap();
        let expected = hex_engine::patch_native_device_parameter(
            &original,
            hex_lattice_contract::ParameterId::from_static(4),
            ParameterValue::Choice(4),
        )
        .unwrap();
        assert_eq!(
            preview,
            hex_engine::native_auto_pan_waveform_preview(&expected).unwrap()
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state(),
            &original
        );
        view.device_panel.pending_values.clear();
        view.worker_requests_pending = 0;
    });
}
