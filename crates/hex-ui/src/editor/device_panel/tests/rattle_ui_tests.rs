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
            && v.device_panel.gesture.snapshot().is_none()
            && v.device_panel.gesture_barriers.is_empty()
    });
}

#[gpui::test]
fn visible_native_controls_follow_applied_automation_without_writing_the_project(
    cx: &mut TestAppContext,
) {
    use crate::editor::worker_ui_fixture;
    use hex_engine::PlaybackCommand;
    use hex_lattice_contract::{DeviceTypeId, ParameterId};
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};
    use std::sync::atomic::Ordering::Acquire;

    let effect = |id| {
        hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::Effect,
            DeviceTypeId::from_static(id),
        )
        .unwrap()
    };
    let rattle = Command::InsertTrackWithDevice {
        name: "automation".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Effect,
        state: effect("hex.rattle"),
    };
    let mut model =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = model.execute(rattle.clone()).unwrap() else {
        panic!("rattle track");
    };
    let drive = ParameterKey {
        device: model.project().track(track).unwrap().devices()[0].id(),
        parameter: ParameterId::from_static(1),
    };
    let utility = Command::InsertDevice {
        track,
        before: None,
        state: effect("hex.utility"),
    };
    let Created::Device(device) = model.execute(utility.clone()).unwrap() else {
        panic!("utility device");
    };
    let mono = ParameterKey {
        device,
        parameter: utility_panel::MONO,
    };
    let channel = ParameterKey {
        device,
        parameter: utility_panel::CHANNEL,
    };
    let envelope = |key: ParameterKey, start, end, outgoing| Command::ReplaceDeviceAutomation {
        device: key.device,
        parameter: key.parameter,
        points: [(BeatTime::ZERO, start), (BeatTime::ONE, end)]
            .into_iter()
            .map(|(beat, value)| AutomationPoint {
                beat,
                value: AutomationValue::new(value).unwrap(),
                outgoing,
            })
            .collect(),
    };
    let mut fixture = worker_ui_fixture::open(
        cx,
        vec![
            rattle,
            utility,
            envelope(drive, 0.2, 0.8, AutomationInterpolation::Linear),
            envelope(mono, 0.0, 1.0, AutomationInterpolation::Step),
            envelope(channel, 0.0, 1.0, AutomationInterpolation::Step),
            Command::SetLoopRegion {
                start: 0,
                end: 96_000,
                enabled: true,
            },
        ],
    );
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        cx.notify();
    });
    visual.run_until_parked();
    let painted = |visual: &VisualTestContext| {
        editor.read_with(visual, |view, cx| {
            let ParameterControl::Float { entity: knob, .. } = &view.device_panel.controls[&drive]
            else {
                panic!("drive knob");
            };
            let ParameterControl::Bool { entity: toggle, .. } = &view.device_panel.controls[&mono]
            else {
                panic!("mono toggle");
            };
            let ParameterControl::Choice {
                entity: choice,
                values,
                ..
            } = &view.device_panel.controls[&channel]
            else {
                panic!("channel selector");
            };
            (
                knob.read(cx).value(),
                toggle.read(cx).checked(),
                values[choice.read(cx).selected()],
            )
        })
    };
    let base = painted(&visual);
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.clone()
    });
    editor.update(&mut visual, |view, _| {
        view.queue_audio_command(PlaybackCommand::PlayFrom(0));
    });
    for (blocks, expected) in [(1, 0.2), (187, 0.5), (188, 0.8)] {
        for _ in 0..blocks {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        editor.update(&mut visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
        });
        visual.run_until_parked();
        let (value, mono, channel) = painted(&visual);
        assert!(
            (value - expected).abs() < 0.01,
            "drive knob: {value} at {expected}"
        );
        assert_eq!(mono, expected == 0.8);
        assert_eq!(channel, if expected == 0.8 { 3 } else { 0 });
        editor.read_with(&visual, |view, _| {
            let (graph, _) = rattle_panel::displayed(view, drive.device).unwrap();
            assert!((f64::from(graph) - value).abs() < 0.001);
            assert_eq!(view.state.as_ref().unwrap().project, before);
        });
    }
    let knob_text = |visual: &VisualTestContext| {
        editor.read_with(visual, |view, cx| {
            let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&drive] else {
                panic!("drive knob");
            };
            entity.read(cx).display_text().to_owned()
        })
    };
    let automated_text = knob_text(&visual);
    let from = visual
        .debug_bounds(parameter(drive.device))
        .unwrap()
        .center();
    let to = point(from.x, from.y + px(12.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    assert_ne!(
        knob_text(&visual),
        automated_text,
        "the active control keeps its draft"
    );
    editor.read_with(&visual, |view, _| {
        let draft = view.device_panel.gesture.snapshot().unwrap().value;
        let (graph, _) = rattle_panel::displayed(view, drive.device).unwrap();
        assert_eq!(ParameterValue::Float(graph), draft);
        assert!((graph - 0.8).abs() > 0.01);
    });
    visual.simulate_keystrokes("escape");
    let deadline = crate::test_wait::hang_deadline();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let settled = editor.update(&mut visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.device_panel.gesture.snapshot().is_none()
                && view.device_panel.gesture_barriers.is_empty()
        });
        if settled {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "gesture cancellation");
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!((painted(&visual).0 - 0.8).abs() < 0.01);
    assert_eq!(knob_text(&visual), automated_text);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before)
    });
    editor.update(&mut visual, |view, _| {
        view.queue_audio_command(PlaybackCommand::Pause);
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.update(&mut visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
        });
        if !fixture.backend.status().unwrap().playing.load(Acquire) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pause acknowledgement"
        );
    }
    visual.run_until_parked();
    assert_eq!(painted(&visual), base);
    fixture.finish(cx);
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
        let g = v.device_panel.gesture.snapshot().unwrap();
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
        v.session_grid.focused_return = Some(return_track);
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
    assert!(visual.debug_bounds("device-automation-indicator").is_none());
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
    let red = gpui::solid_background(gpui::rgb(
        theme::element_colors(hex_project::ElementColor::Red).accent,
    ));
    assert_eq!(
        visual.rendered_background("device-automation-indicator"),
        Some(red),
    );
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
        let ParameterValue::Float(value) = v.device_panel.gesture.snapshot().unwrap().value else {
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
    assert_eq!(
        visual.rendered_background("device-automation-indicator"),
        Some(red),
    );
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(1),
            points: vec![],
        })));
    });
    idle(&editor, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-automation-indicator").is_none());
}
