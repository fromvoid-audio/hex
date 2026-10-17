use super::*;
use hex_engine::eq::{EQ_TYPE_ID, EqState};
fn fixture(legacy: bool) -> (Service, Arc<ViewState>, TrackId, DeviceId) {
    let service = Service::new(48000, 16, 8000000).unwrap();
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "eq source".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(track) = event.created.unwrap() else {
        panic!()
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(if legacy {
                hex_lattice_contract::DeviceTypeId::from_static("hex.eq8")
            } else {
                EQ_TYPE_ID
            }),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).unwrap().devices()[0].id();
    (service, state, track, device)
}
fn read(view: &EditorView, device: DeviceId) -> EqState {
    EqState::decode(
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state(),
    )
    .unwrap()
}
fn settled(view: &EditorView) -> bool {
    view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture_barriers.is_empty()
        && view.device_gesture.snapshot().is_none()
}
fn selector(prefix: &str, device: DeviceId) -> &'static str {
    Box::leak(format!("{prefix}-{}", device.value()).into_boxed_str())
}
fn plot(
    visual: &mut VisualTestContext,
    device: DeviceId,
    x: f32,
    y: f32,
) -> gpui::Point<gpui::Pixels> {
    let bounds = visual
        .debug_bounds(selector("eq-response", device))
        .expect("rendered eq graph");
    let location = point(
        bounds.origin.x + px(7.0) + (bounds.size.width - px(14.0)) * x,
        bounds.origin.y + px(7.0) + (bounds.size.height - px(14.0)) * (1.0 - y),
    );
    let viewport = visual.update(|window, _| window.viewport_size());
    assert!(
        location.x >= px(0.0)
            && location.x < viewport.width
            && location.y >= px(0.0)
            && location.y < viewport.height,
        "test plot point {location:?} lies outside viewport {viewport:?}; graph bounds: {bounds:?}"
    );
    location
}
#[gpui::test]
fn eq_rendered_creation_drag_automation_point_edit_and_history(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    // Keep the complete EQ visible after the preceding instrument card.
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let location = plot(&mut visual, device, 0.4, 0.5);
    visual.simulate_click(location, Modifiers::default());
    // The second press can arrive before the worker publishes the first node.
    visual.simulate_event(gpui::MouseDownEvent {
        position: location,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    visual.simulate_event(gpui::MouseUpEvent {
        position: location,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(selector("eq-auto", device)).is_some(),
        "a newly created node remains selected after publication"
    );
    let initial = editor.read_with(cx, |v, _| read(v, device));
    let id = initial.nodes[0].id;
    let moved = plot(&mut visual, device, 0.7, 0.7);
    visual.simulate_mouse_down(location, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes[0].frequency_hz > initial.nodes[0].frequency_hz * 2.0
    });
    visual.run_until_parked();
    assert_eq!(editor.read_with(cx, |v, _| read(v, device).nodes[0].id), id);
    let button = visual.debug_bounds(selector("eq-auto", device)).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let point = plot(&mut visual, device, 0.3, 0.75);
    visual.simulate_click(point, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).lanes.len() == 1
    });
    visual.run_until_parked();
    let destination = plot(&mut visual, device, 0.65, 0.25);
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(destination, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).lanes[0].points[0].normalized < 0.3
    });
    visual.run_until_parked();
    // Insert before a selected point: sorting must not move its selection onto
    // the newly inserted neighbor at the old lane index.
    visual.simulate_click(destination, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("eq-auto-delete", device))
            .is_some()
    );
    let neighbor = plot(&mut visual, device, 0.3, 0.8);
    visual.simulate_click(neighbor, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).lanes[0].points.len() == 2
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("eq-auto-delete", device))
            .is_none()
    );
    let surviving_neighbor = editor.read_with(cx, |v, _| read(v, device).lanes[0].points[1]);
    visual.simulate_click(neighbor, Modifiers::default());
    visual.run_until_parked();
    let button = visual
        .debug_bounds(selector("eq-auto-delete", device))
        .unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).lanes[0].points.len() == 1
    });
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |v, _| read(v, device).lanes[0].points.clone()),
        vec![surviving_neighbor],
        "deleting the first point preserves its neighbor exactly"
    );
    assert!(
        visual
            .debug_bounds(selector("eq-auto-delete", device))
            .is_none()
    );
    // History can shift the selected point's lane index too.
    visual.simulate_click(destination, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("eq-auto-delete", device))
            .is_some()
    );
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).lanes[0].points.len() == 2
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("eq-auto-delete", device))
            .is_none()
    );
    visual.simulate_click(neighbor, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("eq-auto-delete", device))
            .is_some()
    );
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Redo));
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).lanes[0].points.len() == 1
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("eq-auto-delete", device))
            .is_none()
    );
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).lanes[0].points.len() == 2
    });
    visual.run_until_parked();
    let button = visual.debug_bounds(selector("eq-remove", device)).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.is_empty()
    });
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    editor.read_with(cx, |v, _| {
        let state = read(v, device);
        assert_eq!(state.nodes[0].id, id);
        assert_eq!(state.lanes.len(), 1);
        assert_eq!(state.lanes[0].node, id);
    });
    visual.run_until_parked();

    // Restore the automation view if undo left the node unselected, then remove
    // its last node while that view is open. The response must become editable.
    let automation_field =
        Box::leak(format!("eq-auto-field-{}-gain", device.value()).into_boxed_str());
    if visual.debug_bounds(automation_field).is_none() {
        visual.simulate_click(moved, Modifiers::default());
        visual.run_until_parked();
        let button = visual.debug_bounds(selector("eq-auto", device)).unwrap();
        visual.simulate_click(button.center(), Modifiers::default());
        visual.run_until_parked();
    }
    assert!(visual.debug_bounds(automation_field).is_some());
    let button = visual.debug_bounds(selector("eq-remove", device)).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.is_empty()
    });
    visual.run_until_parked();
    let replacement_position = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(replacement_position, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel.message.is_none(),
            "empty response creation refused: {:?}",
            view.device_panel.message
        )
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    visual.run_until_parked();
    let replacement = editor.read_with(cx, |v, _| {
        let state = read(v, device);
        assert!(state.lanes.is_empty());
        assert!(
            state.nodes[0].id > id,
            "deleting and undoing never reuses a node identity"
        );
        state.nodes[0].id
    });

    // Undoing a creation while its automation is open must leave the same usable
    // empty response, rather than sending future clicks to the removed node.
    let button = visual.debug_bounds(selector("eq-auto", device)).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(automation_field).is_some());
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.is_empty()
    });
    visual.run_until_parked();
    visual.simulate_click(replacement_position, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel.message.is_none(),
            "empty response creation refused: {:?}",
            view.device_panel.message
        )
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(selector("eq-auto", device)).is_some(),
        "the replacement node is selected after undo and recreation"
    );
    assert!(editor.read_with(cx, |v, _| read(v, device).nodes[0].id) > replacement);
}
#[gpui::test]
fn legacy_eq_upgrade_is_one_undoable_state_and_identity_transaction(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(true);
    let original = state.project.device(device).unwrap().state().clone();
    let cursor = state.project.next_identity();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1100.0), px(650.0)));
    visual.run_until_parked();
    let upgrade = visual.debug_bounds(selector("eq-upgrade", device)).unwrap();
    visual.simulate_click(upgrade.center(), Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v)
            && v.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state()
                .type_id()
                == EQ_TYPE_ID
    });
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device).nodes.len(), 40);
        assert_eq!(
            v.state.as_ref().unwrap().project.next_identity(),
            cursor + 40
        );
    });
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |v| {
        settled(v)
            && v.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state()
                == &original
    });
    assert_eq!(
        editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.next_identity()),
        cursor + 40
    );
}
