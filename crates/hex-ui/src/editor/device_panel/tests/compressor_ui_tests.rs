use super::*;

#[gpui::test]
fn compressor_shared_panel_fits_and_drag_commits_one_undoable_edit(cx: &mut TestAppContext) {
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "compressor".into(),
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
                "hex.compressor",
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
            name: "compressor return".into(),
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
        .expect("return compressor");
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
        .debug_bounds(selector("compressor-transfer"))
        .expect("transfer plot");
    assert_eq!(
        card.size.width,
        px(compressor_panel::FRAME_WIDTH + device_meter::WIDTH)
    );
    assert_eq!(card.size.height, px(DEVICE_FRAME_HEIGHT));
    assert!(graph.size.height >= px(50.0));
    for parameter in 1..=8 {
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
            format!("compressor-transfer-{}", return_device.value()).into_boxed_str(),
        ))
        .expect("return transfer plot");
    assert_eq!(return_card.size, card.size);
    assert!(return_graph.size.height >= px(50.0));
    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    let threshold_selector =
        Box::leak(format!("device-{}-parameter-1", device.value()).into_boxed_str());
    let threshold = visual
        .debug_bounds(threshold_selector)
        .expect("threshold field");
    let start = threshold.center();
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
        .expect("threshold draft");
        assert_eq!(
            compressor_panel::cached_curve(view, device),
            Some(&hex_engine::native_compressor_transfer_curve(&expected).expect("draft curve"))
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
fn compressor_preview_uses_unacknowledged_draft_and_retires_on_owner_change(
    cx: &mut TestAppContext,
) {
    let (project, track, _, _) = crate::editor::clip_drag_ui_test::midi_project();
    let original = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.compressor"),
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
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let expected = hex_engine::patch_native_device_parameter(
        &original,
        hex_lattice_contract::ParameterId::from_static(1),
        ParameterValue::Float(-42.0),
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
            ParameterValue::Float(-42.0),
        );
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(8),
            },
            ParameterValue::Choice(1),
        );
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("compressor-rms-detector-{}", device.value()).into_boxed_str()
            ))
            .is_some(),
        "pending mode updates actual detector-axis caption"
    );
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
            compressor_panel::cached_curve(view, device),
            Some(&hex_engine::native_compressor_transfer_curve(&expected).expect("draft preview"))
        );
    });
    editor.update(cx, |view, cx| {
        view.worker_requests_pending = 0;
        view.selected_track = None;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(compressor_panel::cached_curve(view, device).is_none())
    });
}

#[gpui::test]
fn removing_return_compressor_retires_preview_without_changing_owner(cx: &mut TestAppContext) {
    let (service, _, track, return_track, device, _) = actor_return_utility_fixture();
    service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.compressor"),
            )
            .expect("compressor"),
        }))
        .expect("replace utility");
    let state = receive_service_event(&service).state;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(compressor_panel::cached_curve(view, device).is_some());
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveDevice(device))));
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.focused_return, Some(return_track));
        assert!(
            compressor_panel::cached_curve(view, device).is_none(),
            "removed Return preview must retire immediately"
        );
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", device.value()).into_boxed_str()
            ))
            .is_none()
    );
}

#[gpui::test]
fn compressor_mix_track_and_return_real_gesture_cancel_history_and_save_open(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _, track, return_track, return_device, _) = actor_return_utility_fixture();
    let original = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.compressor"),
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
    let track_device = state.project.track(track).unwrap().devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let mix_id = hex_lattice_contract::ParameterId::from_static(7);
    for (owner, device) in [(None, track_device), (Some(return_track), return_device)] {
        editor.update(cx, |view, cx| {
            view.focused_return = owner;
            cx.notify();
        });
        visual.run_until_parked();
        let selector = Box::leak(format!("device-{}-parameter-7", device.value()).into_boxed_str());
        let before = editor.read_with(cx, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state()
                .clone()
        });
        let start = visual
            .debug_bounds(selector)
            .expect("real mix control")
            .center();
        let moved = point(start.x, start.y + px(35.0));
        // Actual pointer Begin/Change, followed by the existing cancellation path.
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            let gesture = view.device_gesture.snapshot().expect("real mix gesture");
            assert_ne!(gesture.value, ParameterValue::Float(1.0));
            let expected =
                hex_engine::patch_native_device_parameter(&before, mix_id, gesture.value).unwrap();
            assert_eq!(
                compressor_panel::cached_curve(view, device),
                Some(&hex_engine::native_compressor_transfer_curve(&expected).unwrap())
            );
        });
        editor.update(cx, |view, _| view.cancel_device_gesture().unwrap());
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture.snapshot().is_none()
                && view.device_gesture_barriers.is_empty()
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .unwrap()
                    .state(),
                &before
            )
        });
        visual.run_until_parked();
        let value_selector =
            Box::leak(format!("device-{}-parameter-7-value", device.value()).into_boxed_str());
        for invalid in ["nan", "inf", "101%"] {
            let value = visual
                .debug_bounds(value_selector)
                .expect("real editable percentage");
            visual.simulate_click(value.center(), Modifiers::default());
            visual.simulate_keystrokes("cmd-a");
            visual.simulate_input(invalid);
            visual.simulate_keystrokes("enter");
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert_eq!(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .device(device)
                        .unwrap()
                        .state(),
                    &before
                );
                assert!(
                    view.device_gesture.snapshot().is_none(),
                    "invalid text cannot begin an edit"
                );
            });
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
        }
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture.snapshot().is_none()
                && view.device_gesture_barriers.is_empty()
        });
        let changed = editor.read_with(cx, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state()
                .clone()
        });
        assert_ne!(changed, before);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .unwrap()
                    .state(),
                &before
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
                    .device(device)
                    .unwrap()
                    .state(),
                &changed
            )
        });
    }
    let expected = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let path = std::env::temp_dir().join(format!(
        "hex-compressor-mix-{}-{}.hex",
        std::process::id(),
        track_device.value()
    ));
    assert!(!path.exists(), "disposable create-new fixture");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "save must succeed")
    });
    let reload = Service::new(48_000, 8, 8_000_000).unwrap();
    let _ = receive_service_event(&reload);
    reload.submit(Request::Open(path.clone())).unwrap();
    let reopened = receive_service_event(&reload);
    assert!(reopened.error.is_none());
    for device in [track_device, return_device] {
        assert_eq!(
            reopened.state.project.device(device).unwrap().state(),
            expected.device(device).unwrap().state()
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn compressor_mode_menu_track_return_cancel_commit_preview_history_and_reopen(
    cx: &mut TestAppContext,
) {
    let (service, _, track, return_track, return_device, _) = actor_return_utility_fixture();
    let original = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.compressor"),
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
        let selector = Box::leak(format!("device-{}-parameter-8", device.value()).into_boxed_str());
        let graph = visual
            .debug_bounds(Box::leak(
                format!("compressor-transfer-{}", device.value()).into_boxed_str(),
            ))
            .expect("actual provider-backed plot");
        assert!(graph.size.width > px(30.0) && graph.size.height > px(40.0));
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        let position = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(position, Modifiers::default());
        visual.run_until_parked();
        let option =
            Box::leak(format!("device-{}-parameter-8-option-1", device.value()).into_boxed_str());
        let option_bounds = visual
            .debug_bounds(option)
            .expect("actual detector mode options");
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
                hex_lattice_contract::ParameterId::from_static(8)
            )
            .unwrap(),
            ParameterValue::Choice(1)
        );
        visual.run_until_parked();
        let mode_label =
            Box::leak(format!("compressor-rms-detector-{}", device.value()).into_boxed_str());
        assert!(
            visual.debug_bounds(mode_label).is_some(),
            "actual RMS detector-axis caption"
        );
        editor.read_with(cx, |view, _| {
            assert_eq!(
                compressor_panel::cached_curve(view, device),
                Some(&hex_engine::native_compressor_transfer_curve(changed_state).unwrap())
            );
            assert_eq!(changed_state.state_version(), 3);
            assert_eq!(changed_state.payload().len(), 32);
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
        "hex-compressor-mode-{}-{}.hex",
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
