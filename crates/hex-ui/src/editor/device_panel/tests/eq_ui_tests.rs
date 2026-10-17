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
fn eq_layout_contains_full_controls_without_overlap_at_supported_widths(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(position, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    let node = editor.read_with(cx, |view, _| read(view, device).nodes[0].id);
    for automation in [false, true] {
        if automation {
            let toggle = visual.debug_bounds(selector("eq-auto", device)).unwrap();
            visual.simulate_click(toggle.center(), Modifiers::default());
            visual.run_until_parked();
        }
        for width in [880.0, 1500.0] {
            visual.simulate_resize(size(px(width), px(650.0)));
            visual.run_until_parked();
            let toolbar = visual.debug_bounds(selector("eq-toolbar", device)).unwrap();
            let footer = visual.debug_bounds(selector("eq-footer", device)).unwrap();
            let control_bounds =
                |visual: &mut VisualTestContext, parameter: hex_lattice_contract::ParameterId| {
                    visual
                        .debug_bounds(Box::leak(
                            format!("eq-control-{}-{}", device.value(), parameter.value())
                                .into_boxed_str(),
                        ))
                        .unwrap()
                };
            let out = control_bounds(&mut visual, hex_engine::eq::OUTPUT_GAIN);
            assert!(
                out.top() >= toolbar.top() && out.bottom() <= toolbar.bottom(),
                "the output dial and value must fit inside the toolbar at {width}px"
            );
            let mut previous = None;
            for field in [
                hex_engine::eq::NodeParameter::Enabled,
                hex_engine::eq::NodeParameter::Kind,
                hex_engine::eq::NodeParameter::Frequency,
                hex_engine::eq::NodeParameter::Gain,
                hex_engine::eq::NodeParameter::Q,
                hex_engine::eq::NodeParameter::Slope,
            ] {
                let bounds = control_bounds(&mut visual, node.parameter(field).unwrap());
                assert!(bounds.top() >= footer.top() && bounds.bottom() <= footer.bottom());
                assert!(bounds.left() >= footer.left() && bounds.right() <= footer.right());
                if let Some(right) = previous {
                    assert!(bounds.left() >= right, "selected controls must not overlap");
                }
                previous = Some(bounds.right());
            }
            let graph = visual
                .debug_bounds(selector("eq-response", device))
                .unwrap();
            assert!(toolbar.bottom() <= graph.top());
            assert!(graph.bottom() <= footer.top());
            assert!(graph.size.height >= px(if automation { 48.0 } else { 60.0 }));
            let frequency = control_bounds(
                &mut visual,
                node.parameter(hex_engine::eq::NodeParameter::Frequency)
                    .unwrap(),
            );
            assert!(
                frequency.size.width >= px(74.0),
                "full frequency text and units need the wide micro dial"
            );
            for prefix in ["eq-auto", "eq-remove"] {
                let bounds = visual.debug_bounds(selector(prefix, device)).unwrap();
                assert!(bounds.left() >= previous.unwrap() && bounds.right() <= footer.right());
                assert!(bounds.top() >= footer.top() && bounds.bottom() <= footer.bottom());
                previous = Some(bounds.right());
            }
            let mut right = toolbar.left();
            for name in [
                format!(
                    "eq-control-{}-{}",
                    device.value(),
                    hex_engine::eq::MODE.value()
                ),
                format!(
                    "eq-control-{}-{}",
                    device.value(),
                    hex_engine::eq::OUTPUT_GAIN.value()
                ),
                format!("eq-bank-{}-0", device.value()),
                format!("eq-bank-{}-1", device.value()),
                format!("eq-bank-{}-2", device.value()),
                format!("eq-bank-{}-3", device.value()),
                format!("eq-bank-{}-4", device.value()),
                format!("eq8-spectrum-{}", device.value()),
                format!("eq-spectrum-input-{}", device.value()),
                format!("eq-audition-{}", device.value()),
            ] {
                let bounds = visual
                    .debug_bounds(Box::leak(name.into_boxed_str()))
                    .unwrap();
                assert!(
                    bounds.left() >= right && bounds.right() <= toolbar.right(),
                    "toolbar controls fit without overlap"
                );
                assert!(bounds.top() >= toolbar.top() && bounds.bottom() <= toolbar.bottom());
                right = bounds.right();
            }
            if automation {
                let row = visual
                    .debug_bounds(selector("eq-automation-toolbar", device))
                    .unwrap();
                let value = visual
                    .debug_bounds(selector("eq-playhead", device))
                    .unwrap();
                assert!(row.bottom() <= graph.top());
                assert!(value.left() >= row.left() && value.right() <= row.right());
                assert!(value.top() >= row.top() && value.bottom() <= row.bottom());
            }
        }
    }
}

#[gpui::test]
fn eq_blank_press_drags_edge_cuts_and_bell_as_one_history_edit(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    for (index, (x, kind)) in [
        (0.02, hex_engine::eq::NodeKind::LowCut),
        (0.98, hex_engine::eq::NodeKind::HighCut),
        (0.5, hex_engine::eq::NodeKind::Bell),
    ]
    .into_iter()
    .enumerate()
    {
        let start = plot(&mut visual, device, x, 0.4);
        let moved = plot(&mut visual, device, 0.6, 0.7);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert_eq!(
                read(v, device).nodes.len(),
                index,
                "held creation is a local preview"
            );
            assert!(
                settled(v),
                "no worker acknowledgment is needed to keep dragging"
            );
        });
        assert!(
            visual.debug_bounds(selector("eq-auto", device)).is_some(),
            "the provisional stable node is visibly selected"
        );
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        // A move after release, including one reporting a held button, must not
        // attach itself to the new node when asynchronous publication arrives.
        let late = plot(&mut visual, device, 0.8, 0.2);
        visual.simulate_mouse_move(late, Some(MouseButton::Left), Modifiers::default());
        pump_until(&editor, cx, |v| {
            settled(v) && read(v, device).nodes.len() == index + 1
        });
        visual.run_until_parked();
        let authored = editor.read_with(cx, |v, _| read(v, device));
        let node = authored.nodes[index];
        assert_eq!(
            node.kind, kind,
            "creation shape follows the press zone, not the release"
        );
        assert!((node.frequency_hz - 20.0 * 1000.0_f32.powf(0.6)).abs() < 0.1);
        if kind == hex_engine::eq::NodeKind::Bell {
            assert!((node.gain_db - 9.6).abs() < 0.001);
        } else {
            assert_eq!(node.gain_db, 0.0);
            assert!((node.q - 0.1 * 180.0_f32.powf(0.7)).abs() < 0.001);
        }
        visual.simulate_mouse_move(late, Some(MouseButton::Left), Modifiers::default());
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| {
            settled(v) && read(v, device).nodes.len() == index
        });
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Redo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == authored);
        visual.run_until_parked();
    }
    let authored = editor.read_with(cx, |v, _| read(v, device));
    assert!(
        authored
            .nodes
            .windows(2)
            .all(|pair| pair[0].id < pair[1].id)
    );
    let path = std::env::temp_dir().join(format!(
        "hex-eq-gesture-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump_until(&editor, cx, |v| settled(v) && path.exists());
    let reload = Service::new(48000, 8, 8000000).unwrap();
    let _ = receive_service_event(&reload);
    reload.submit(Request::Open(path.clone())).unwrap();
    let reopened = receive_service_event(&reload);
    assert!(reopened.error.is_none());
    assert_eq!(
        EqState::decode(reopened.state.project.device(device).unwrap().state()).unwrap(),
        authored
    );
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn eq_creation_escape_and_changed_source_cancel_without_reserving_identity(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device) = fixture(false);
    let initial_next = state.project.next_identity();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let start = plot(&mut visual, device, 0.02, 0.5);
    let moved = plot(&mut visual, device, 0.4, 0.7);
    for cancellation in 0..3 {
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        match cancellation {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, _| window.blur()),
            _ => visual.simulate_mouse_move(moved, None, Modifiers::default()),
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(
                read(v, device).nodes.is_empty(),
                "cancellation {cancellation} must discard preview"
            );
            assert_eq!(
                v.state.as_ref().unwrap().project.next_identity(),
                initial_next
            );
            assert!(settled(v));
        });
    }
    // If worker admission becomes busy between press and release, rejecting
    // the transaction must also remove the provisional selection immediately.
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    editor.update(cx, |v, _| v.worker_requests_pending = 1);
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    editor.update(cx, |v, _| v.worker_requests_pending = 0);
    visual.run_until_parked();
    assert!(visual.debug_bounds(selector("eq-auto", device)).is_none());
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    // A canonical edit during the held preview invalidates its expected state.
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::SetDeviceParameter {
            device,
            parameter: hex_engine::eq::OUTPUT_GAIN,
            value: ParameterValue::Float(0.5),
        }))
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).output_gain == 0.5
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(read(v, device).nodes.is_empty());
        assert_eq!(
            v.state.as_ref().unwrap().project.next_identity(),
            initial_next
        );
        assert!(settled(v));
    });
    // The cancelled capture does not poison the next independent press.
    visual.simulate_click(start, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
}

#[gpui::test]
fn eq_creation_outside_release_clamps_and_allocator_or_project_change_cancels(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let start = plot(&mut visual, device, 0.02, 0.4);
    let bounds = visual
        .debug_bounds(selector("eq-response", device))
        .unwrap();
    let outside = point(bounds.right() + px(12.0), bounds.center().y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    editor.read_with(cx, |v, _| {
        let state = read(v, device);
        assert_eq!(state.nodes[0].kind, hex_engine::eq::NodeKind::LowCut);
        assert!((state.nodes[0].frequency_hz - 20000.0).abs() < 0.1);
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    // The EQ source is unchanged, but another identity reservation invalidates
    // the provisional ID. It must never be rebased onto a replacement ID.
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "concurrent track".into(),
        })))
    });
    pump_until(&editor, cx, |v| {
        settled(v) && v.state.as_ref().unwrap().project.tracks().len() == 2
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(editor.read_with(cx, |v, _| read(v, device).nodes.len()), 1);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    editor.update(cx, |v, cx| {
        v.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_move(start, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device).nodes.len(), 1);
        assert!(settled(v));
    });
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
