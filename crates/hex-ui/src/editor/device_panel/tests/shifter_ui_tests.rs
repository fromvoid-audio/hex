use super::*;

#[gpui::test]
fn shifter_panel_fits_track_and_return_switches_modes_and_commits_one_undoable_edit(
    cx: &mut TestAppContext,
) {
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "shifter track".into(),
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
                "hex.shifter",
            )),
            before: None,
        })
        .expect("shifter device");
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let original = state.project.track(track).expect("track").devices()[0]
        .state()
        .clone();

    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "shifter return".into(),
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
        .expect("return shifter");
    let state = receive_service_event(&service).state;
    let return_device = state.project.return_tracks()[0].devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let pitch_visible = [
        1, 2, 3, 6, 7, 8, 9, 10, 12, 13, 14, 15, 16, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
    ];
    let assert_panel = |visual: &mut VisualTestContext, dev: DeviceId, params: &[u32]| {
        let card = visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", dev.value()).into_boxed_str(),
            ))
            .expect("shifter card");
        let body = visual
            .debug_bounds(Box::leak(
                format!("device-body-{}", dev.value()).into_boxed_str(),
            ))
            .expect("shifter body");
        assert_eq!(
            card.size.width,
            px(shifter_panel::FRAME_WIDTH
                + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
                + device_meter::WIDTH)
        );
        assert_eq!(card.size.height, px(DEVICE_FRAME_HEIGHT));
        for &parameter in params {
            let control = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", dev.value()).into_boxed_str(),
                ))
                .unwrap_or_else(|| panic!("missing shifter control parameter {parameter}"));
            assert!(
                control.origin.x >= body.origin.x,
                "param {parameter} left overflow"
            );
            assert!(
                control.origin.y >= body.origin.y,
                "param {parameter} top overflow"
            );
            assert!(
                control.origin.x + control.size.width <= body.origin.x + body.size.width,
                "param {parameter} right overflow"
            );
            assert!(
                control.origin.y + control.size.height <= body.origin.y + body.size.height,
                "param {parameter} bottom overflow"
            );
        }
        card.size
    };

    let track_size = assert_panel(&mut visual, device, &pitch_visible);
    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    let return_size = assert_panel(&mut visual, return_device, &pitch_visible);
    assert_eq!(return_size, track_size);

    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();

    // Drag Coarse (parameter 2) knob -> commits one undoable edit, and Undo restores exact state.
    let coarse_knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-2-knob", device.value()).into_boxed_str(),
        ))
        .expect("coarse knob");
    let start = coarse_knob.center();
    let moved = point(start.x, start.y - px(25.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
            && view.device_panel.gesture_barriers.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_ne!(
            view.state
                .as_ref()
                .expect("state")
                .project
                .device(device)
                .expect("shifter")
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
                .device(device)
                .expect("shifter")
                .state(),
            &original,
            "one undo restores the entire shifter drag"
        );
    });

    // Switch MODE to Freq (1) -> Coarse Hz (4) and Fine Hz (5) replace Coarse St (2) and Fine Ct (3).
    let freq_state = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Effect,
        &original,
        hex_lattice_contract::ParameterId::from_static(1),
        hex_lattice_contract::ParameterValue::Choice(1),
    )
    .expect("patch mode to freq");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: freq_state,
        })));
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.refresh().expect("refresh");
    visual.run_until_parked();
    let freq_visible = [
        1, 4, 5, 6, 8, 9, 10, 12, 13, 14, 15, 16, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
    ];
    assert_panel(&mut visual, device, &freq_visible);
}
