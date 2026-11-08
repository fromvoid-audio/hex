use super::*;

fn window(
    cx: &mut TestAppContext,
) -> (gpui::AnyWindowHandle, Entity<EditorView>, TrackId, DeviceId) {
    cx.update(hex_widgets::bind_response_graph_keys);
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack { name: "eq8".into() }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(track) = event.created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static("hex.eq8")),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).unwrap().devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    (handle, editor, track, device)
}
fn mask(view: &EditorView, device: DeviceId) -> u8 {
    let p = &view.device_panel.eq8_panels[&device];
    p.selected_bands.unwrap_or(1 << p.selected_band)
}
fn idle(editor: &Entity<EditorView>, cx: &mut TestAppContext) {
    pump_until(editor, cx, |v| {
        v.worker_requests_pending == 0
            && v.pending_requests.is_empty()
            && v.device_gesture.snapshot().is_none()
            && v.device_gesture_barriers.is_empty()
    });
}
fn geometry(
    editor: &Entity<EditorView>,
    cx: &TestAppContext,
    device: DeviceId,
) -> (gpui::Bounds<gpui::Pixels>, [gpui::Point<gpui::Pixels>; 2]) {
    editor.read_with(cx, |v, cx| {
        let graph = v.device_panel.eq8_graphs[&device].entity.read(cx);
        let plot = graph.plot_bounds().unwrap();
        let points = [1, 2].map(|id| {
            let p = graph.node_position(id).unwrap();
            point(
                plot.left() + plot.size.width * p[0],
                plot.top() + plot.size.height * (1.0 - p[1]),
            )
        });
        (plot, points)
    })
}
fn marquee(visual: &mut VisualTestContext, points: [gpui::Point<gpui::Pixels>; 2]) {
    let left = points[0].x.min(points[1].x) - px(10.0);
    let right = points[0].x.max(points[1].x) + px(10.0);
    let top = points[0].y.min(points[1].y) - px(14.0);
    let bottom = points[0].y.max(points[1].y) + px(14.0);
    let from = point(left, top);
    let to = point(right, bottom);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn eq8_marquee_group_move_and_disable_each_commit_one_undo(cx: &mut TestAppContext) {
    let (handle, editor, track, device) = window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (plot, points) = geometry(&editor, cx, device);
    let before = editor.read_with(cx, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .state()
            .clone()
    });
    marquee(&mut visual, points);
    editor.read_with(cx, |v, _| {
        assert_eq!(mask(v, device), 6);
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state(),
            &before
        );
    });
    let (_, points) = geometry(&editor, cx, device);
    let from = points[0];
    let to = point(
        from.x + plot.size.width * 0.04,
        from.y - plot.size.height * 0.08,
    );
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let snapshot = v.device_gesture.snapshot().unwrap();
        assert_eq!(
            snapshot.target,
            hex_session::device_gesture::GestureTarget::DeviceParameterSnapshot { device }
        );
        assert_eq!(snapshot.parameters.unwrap().entries().len(), 4);
        assert_eq!(mask(v, device), 6);
    });
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    idle(&editor, cx);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let state = v
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .state();
        assert_ne!(state, &before);
        for id in [8, 9, 13, 14] {
            assert_ne!(
                hex_engine::native_device_parameter(
                    state,
                    hex_lattice_contract::ParameterId::from_static(id)
                )
                .unwrap(),
                hex_engine::native_device_parameter(
                    &before,
                    hex_lattice_contract::ParameterId::from_static(id)
                )
                .unwrap()
            );
        }
        assert_eq!(mask(v, device), 6);
    });
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    idle(&editor, cx);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state(),
            &before
        )
    });
    // The marquee remains selected through ordinary history; delete disables it.
    visual.simulate_keystrokes("backspace");
    idle(&editor, cx);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let state = v
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .state();
        for id in [6, 11] {
            assert_eq!(
                hex_engine::native_device_parameter(
                    state,
                    hex_lattice_contract::ParameterId::from_static(id)
                )
                .unwrap(),
                ParameterValue::Bool(false)
            );
        }
        assert!(v.state.as_ref().unwrap().project.device(device).is_some());
    });
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    idle(&editor, cx);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state(),
            &before
        )
    });
}

#[gpui::test]
fn eq8_group_drag_cancel_and_blank_selection_never_remove_fixed_device(cx: &mut TestAppContext) {
    let (handle, editor, track, device) = window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (plot, points) = geometry(&editor, cx, device);
    marquee(&mut visual, points);
    let before = editor.read_with(cx, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .state()
            .clone()
    });
    let (_, points) = geometry(&editor, cx, device);
    let from = points[0];
    let to = point(from.x + px(15.0), from.y - px(12.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    idle(&editor, cx);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state(),
            &before
        );
        assert_eq!(mask(v, device), 6);
    });
    let blank = point(plot.left() + px(2.0), plot.top() + px(2.0));
    visual.simulate_mouse_down(blank, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(blank, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert_eq!(mask(v, device), 0));
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(v.state.as_ref().unwrap().project.device(device).is_some());
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state(),
            &before
        );
    });
}
