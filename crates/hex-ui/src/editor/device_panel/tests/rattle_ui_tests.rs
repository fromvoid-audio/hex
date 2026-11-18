use super::*;

fn window(
    cx: &mut TestAppContext,
) -> (gpui::AnyWindowHandle, Entity<EditorView>, TrackId, DeviceId) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "rattle".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(track) = event.created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.rattle",
            )),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).unwrap().devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    (handle, editor, track, device)
}
fn selector(device: DeviceId, name: &str) -> &'static str {
    Box::leak(format!("{name}-{}", device.value()).into_boxed_str())
}
fn parameter(device: DeviceId) -> &'static str {
    Box::leak(format!("device-{}-parameter-1", device.value()).into_boxed_str())
}
fn idle(editor: &Entity<EditorView>, cx: &mut TestAppContext) {
    pump_until(editor, cx, |v| {
        v.worker_requests_pending == 0
            && v.pending_requests.is_empty()
            && v.device_gesture.snapshot().is_none()
            && v.device_gesture_barriers.is_empty()
    });
}

#[gpui::test]
fn rattle_track_return_panel_fits_and_drive_draft_commits_one_undo(cx: &mut TestAppContext) {
    let (handle, editor, track, device) = window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let card = visual
        .debug_bounds(selector(device, "effect-device"))
        .unwrap();
    let body = visual
        .debug_bounds(selector(device, "device-body"))
        .unwrap();
    let display = visual
        .debug_bounds(selector(device, "rattle-diamonds"))
        .unwrap();
    let knob = visual.debug_bounds(parameter(device)).unwrap();
    assert_eq!(
        card.size.width,
        px(rattle_panel::FRAME_WIDTH
            + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
            + device_meter::WIDTH)
    );
    assert_eq!(card.size.height, px(DEVICE_FRAME_HEIGHT));
    assert!((f32::from(card.size.width - card.size.height)).abs() < 32.0);
    assert!(display.size.height > px(70.0));
    assert!(body.contains(&knob.center()));
    assert!(knob.bottom() <= body.bottom());
    let (original, initial) = editor.read_with(cx, |v, _| {
        (
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state()
                .clone(),
            rattle_panel::displayed(v, device).unwrap(),
        )
    });
    let from = knob.center();
    let to = point(from.x, from.y - px(25.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let g = v.device_gesture.snapshot().unwrap();
        let ParameterValue::Float(value) = g.value else {
            panic!("drive")
        };
        let preview = rattle_panel::displayed(v, device).unwrap();
        assert_eq!(preview.0, value);
        assert_ne!(preview, initial);
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state(),
            &original
        );
    });
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    idle(&editor, cx);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_ne!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state(),
            &original
        )
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
            &original
        );
        assert_eq!(rattle_panel::displayed(v, device).unwrap(), initial);
    });
    let return_track = editor.update(cx, |v, cx| {
        assert!(v.enqueue(Request::Edit(Command::AddReturnTrack {
            name: "return".into()
        })));
        let event = receive_service_event(&v.service);
        let Created::ReturnTrack(id) = event.created.unwrap() else {
            panic!("return")
        };
        v.receive(event);
        cx.notify();
        id
    });
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: original.clone()
        })))
    });
    idle(&editor, cx);
    let return_device = editor.update(cx, |v, cx| {
        let id = v.state.as_ref().unwrap().project.return_tracks()[0].devices()[0].id();
        v.focused_return = Some(return_track);
        cx.notify();
        id
    });
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds(selector(return_device, "effect-device"))
            .unwrap()
            .size,
        card.size
    );
    assert!(
        visual
            .debug_bounds(selector(return_device, "rattle-diamonds"))
            .unwrap()
            .size
            .height
            > px(70.0)
    );
    assert!(visual.debug_bounds(parameter(return_device)).is_some());
}

#[gpui::test]
fn rattle_display_reads_authored_automation_and_manual_draft_precedence(cx: &mut TestAppContext) {
    use hex_lattice_contract::{DeviceAutomationTarget, ParameterId};
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
        BeatTime,
    };
    let (handle, editor, _, device) = window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let base = editor.read_with(cx, |v, _| rattle_panel::displayed(v, device).unwrap());
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(1),
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.8).unwrap(),
                outgoing: AutomationInterpolation::Linear
            }]
        })))
    });
    idle(&editor, cx);
    visual.run_until_parked();
    let automated = editor.read_with(cx, |v, _| rattle_panel::displayed(v, device).unwrap());
    assert_eq!(automated.0, 0.8);
    assert_ne!(automated.1, base.1);
    let knob = visual.debug_bounds(parameter(device)).unwrap();
    let from = knob.center();
    let to = point(from.x, from.y - px(12.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let ParameterValue::Float(value) = v.device_gesture.snapshot().unwrap().value else {
            panic!("drive")
        };
        assert_eq!(rattle_panel::displayed(v, device).unwrap().0, value);
    });
    visual.simulate_keystrokes("escape");
    idle(&editor, cx);
    visual.run_until_parked();
    editor.update(cx, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::SetDeviceAutomationReadSelection {
                device,
                target: DeviceAutomationTarget::Parameter(ParameterId::from_static(1)),
                selection: AutomationReadSelection::Base
            }))
        )
    });
    idle(&editor, cx);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(rattle_panel::displayed(v, device).unwrap(), base)
    });
}
