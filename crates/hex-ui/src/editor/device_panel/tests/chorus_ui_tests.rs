use super::*;

#[gpui::test]
fn chorus_panel_fits_track_and_return_and_drag_commits_one_undoable_edit(cx: &mut TestAppContext) {
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "chorus track".into(),
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
                "hex.chorus",
            )),
            before: None,
        })
        .expect("chorus device");
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let original = state.project.track(track).expect("track").devices()[0]
        .state()
        .clone();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "chorus return".into(),
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
        .expect("return chorus");
    let state = receive_service_event(&service).state;
    let return_device = state.project.return_tracks()[0].devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let assert_panel = |visual: &mut VisualTestContext, dev: DeviceId| {
        let card = visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", dev.value()).into_boxed_str(),
            ))
            .expect("chorus card");
        let body = visual
            .debug_bounds(Box::leak(
                format!("device-body-{}", dev.value()).into_boxed_str(),
            ))
            .expect("chorus body");
        assert_eq!(
            card.size.width,
            px(chorus_panel::FRAME_WIDTH
                + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
                + device_meter::WIDTH)
        );
        assert_eq!(card.size.height, px(DEVICE_FRAME_HEIGHT));
        for parameter in 1..=8 {
            let control = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", dev.value()).into_boxed_str(),
                ))
                .expect("chorus control");
            assert!(control.origin.x >= body.origin.x);
            assert!(control.origin.y >= body.origin.y);
            assert!(control.origin.x + control.size.width <= body.origin.x + body.size.width);
            assert!(control.origin.y + control.size.height <= body.origin.y + body.size.height);
        }
        card.size
    };

    let track_size = assert_panel(&mut visual, device);
    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    let return_size = assert_panel(&mut visual, return_device);
    assert_eq!(return_size, track_size);

    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();

    let rate_knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-knob", device.value()).into_boxed_str(),
        ))
        .expect("rate knob");
    let start = rate_knob.center();
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
                .expect("chorus")
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
                .expect("chorus")
                .state(),
            &original,
            "one undo restores the entire chorus drag"
        );
    });
}
