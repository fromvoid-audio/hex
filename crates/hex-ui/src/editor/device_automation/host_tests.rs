//! Real host-control and header assignment, canonical worker edits and receipt
//! refusal. Native binary/UI/listening acceptance remains a separate user check.
use super::*;
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{Command, Created, TrackId};
use hex_session::service::Service;
use std::time::{Duration, Instant};

fn event(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::yield_now()
            }
            Err(reason) => panic!("event: {reason}"),
        }
    }
}
#[derive(Clone, Copy)]
enum Slot {
    Track,
    Main,
    Return,
}
fn window(
    cx: &mut TestAppContext,
    type_id: &str,
    slot: Slot,
) -> (
    VisualTestContext,
    Entity<EditorView>,
    Owner,
    TrackId,
    DeviceId,
) {
    cx.update(super::super::bind_editor_keys);
    let service = Service::new(48000, 32, 8_000_000).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "host automation".into(),
        }))
        .unwrap();
    let Created::Track(track) = event(&service).created.unwrap() else {
        panic!("track")
    };
    let state =
        hex_engine::default_native_device_state(DeviceTypeId::new(type_id).unwrap()).unwrap();
    let (owner, command) = match slot {
        Slot::Track => (
            Owner::Track(track),
            Command::InsertDevice {
                track,
                before: None,
                state,
            },
        ),
        Slot::Main => (
            Owner::Main,
            Command::InsertMainDevice {
                before: None,
                state,
            },
        ),
        Slot::Return => {
            service
                .submit(Request::Edit(Command::AddReturnTrack {
                    name: "return".into(),
                }))
                .unwrap();
            let Created::ReturnTrack(id) = event(&service).created.unwrap() else {
                panic!("return")
            };
            (
                Owner::Return(id),
                Command::InsertReturnDevice {
                    return_track: id,
                    before: None,
                    state,
                },
            )
        }
    };
    service.submit(Request::Edit(command)).unwrap();
    let added = event(&service);
    let Created::Device(device) = added.created.unwrap() else {
        panic!("device")
    };
    let bootstrap = cx.add_window(|_, _| gpui::Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(
                    window,
                    cx,
                    service,
                    Err("offline host automation test".into()),
                )
            });
            editor.update(cx, |view, cx| {
                view.state = Some(added.state);
                view.selected_track = Some(track);
                view.device_panel_active = true;
                view.clip_view_open = true;
                match owner {
                    Owner::Main => super::super::device_panel::select_main(view, window, cx),
                    Owner::Return(id) => view.focused_return = Some(id),
                    Owner::Track(_) => {}
                }
                view.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1100.0), px(800.0)));
    visual.run_until_parked();
    (visual, editor, owner, track, device)
}
fn choose_index(visual: &mut VisualTestContext, editor: &Entity<EditorView>, index: usize) {
    let down = editor.read_with(visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert!(index < menu.items.len());
        (index + menu.items.len() - menu.selected) % menu.items.len()
    });
    visual.simulate_keystrokes(&format!("{}enter", "down ".repeat(down)));
    visual.run_until_parked();
}
fn choose_label(visual: &mut VisualTestContext, editor: &Entity<EditorView>, label: &str) {
    let index = editor.read_with(visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| item.label == label)
            .unwrap()
    });
    choose_index(visual, editor, index);
}
fn header_menu(visual: &mut VisualTestContext, owner: Owner) {
    let key = match owner {
        Owner::Track(id) => format!("arrangement-track-header-{}", id.value()),
        Owner::Main => "main-track-header".into(),
        Owner::Return(id) => format!("return-track-header-{}", id.value()),
    };
    let bounds = visual
        .debug_bounds(Box::leak(key.into_boxed_str()))
        .unwrap();
    let at = point(bounds.left() + px(8.0), bounds.top() + px(8.0));
    visual.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
}
fn header_assign(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    owner: Owner,
    target: DeviceAutomationTarget,
) {
    header_menu(visual, owner);
    choose_label(visual, editor, "automatable parameters…");
    choose_index(visual, editor, 0);
    let index=editor.read_with(visual,|view,_|view.context_menu.as_ref().unwrap().items.iter().position(|item|matches!(&item.action,super::super::context_menu::EditorContextAction::Automation(Action::Show(entry,_)) if entry.target.kind==target)).unwrap());
    choose_index(visual, editor, index);
}
fn control_assign(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    device: DeviceId,
    target: DeviceAutomationTarget,
) {
    let (parameter, side) = match target {
        DeviceAutomationTarget::HostInputGain => (hex_engine::DEVICE_INPUT_GAIN.id, "input"),
        DeviceAutomationTarget::HostOutputGain => (hex_engine::DEVICE_OUTPUT_GAIN.id, "output"),
        DeviceAutomationTarget::Parameter(_) => panic!("host control"),
    };
    let selector = format!(
        "device-{}-parameter-{}-{side}-meter",
        device.value(),
        parameter.value()
    );
    let at = visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .unwrap()
        .center();
    visual.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    choose_label(visual, editor, "add new automation lane");
}
fn pump(visual: &mut VisualTestContext, editor: &Entity<EditorView>) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let done = editor.update(visual, |view, cx| {
            view.tick(cx);
            cx.notify();
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        visual.run_until_parked();
        if done {
            return;
        }
        assert!(Instant::now() < deadline, "worker did not finish");
        std::thread::yield_now();
    }
}
fn plot(
    visual: &mut VisualTestContext,
    device: DeviceId,
    target: DeviceAutomationTarget,
) -> gpui::Bounds<Pixels> {
    let key = TargetKey(target);
    visual
        .debug_bounds(Box::leak(
            format!("automation-plot-{}-{}-false", device.value(), key).into_boxed_str(),
        ))
        .unwrap()
}
fn draw(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    device: DeviceId,
    target: DeviceAutomationTarget,
    y: f32,
) {
    let bounds = plot(visual, device, target);
    let at = point(bounds.center().x, bounds.top() + bounds.size.height * y);
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    for _ in 0..2 {
        editor.update(visual, |view, cx| {
            assert!(view.automation_view.devices.pending.is_some());
            cx.notify();
        });
        visual.run_until_parked();
    }
    pump(visual, editor);
    editor.read_with(visual, |view, _| {
        assert!(view.automation_view.devices.pending.is_none());
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .automation_target(target)
                .unwrap()
                .points
                .len(),
            1
        );
    });
}

#[gpui::test]
fn host_controls_and_header_picker_are_real_for_utility_and_other_native_owners(
    cx: &mut TestAppContext,
) {
    for type_id in ["hex.utility", "hex.overdrive"] {
        for slot in [Slot::Track, Slot::Main, Slot::Return] {
            let (mut visual, editor, owner, _, device) = window(cx, type_id, slot);
            let before = editor.read_with(&visual, |view, _| {
                view.state.as_ref().unwrap().project.snapshot()
            });
            control_assign(
                &mut visual,
                &editor,
                device,
                DeviceAutomationTarget::HostInputGain,
            );
            control_assign(
                &mut visual,
                &editor,
                device,
                DeviceAutomationTarget::HostOutputGain,
            );
            header_assign(
                &mut visual,
                &editor,
                owner,
                DeviceAutomationTarget::HostOutputGain,
            );
            editor.read_with(&visual, |view, _| {
                assert_eq!(
                    view.automation_view
                        .devices
                        .extra
                        .iter()
                        .filter(|entry| entry.target.owner == owner)
                        .count(),
                    2,
                    "repeat header assignment reuses output lane"
                );
                assert_eq!(view.worker_requests_pending, 0);
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
                for entry in view
                    .automation_view
                    .devices
                    .extra
                    .iter()
                    .filter(|entry| entry.target.owner == owner)
                {
                    assert!(entry.target.kind.is_host());
                    assert!(matches!(entry.metadata, EntryMetadata::HostGain));
                    assert!(
                        view.automation_view
                            .devices
                            .points(view.state.as_ref().unwrap(), entry.target)
                            .is_empty()
                    );
                }
            });
        }
    }
}

#[gpui::test]
fn provider_and_two_host_lanes_edit_clear_and_undo_independently(cx: &mut TestAppContext) {
    let (mut visual, editor, owner, _, device) = window(cx, "hex.utility", Slot::Track);
    let provider =
        DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1));
    header_assign(&mut visual, &editor, owner, provider);
    control_assign(
        &mut visual,
        &editor,
        device,
        DeviceAutomationTarget::HostInputGain,
    );
    control_assign(
        &mut visual,
        &editor,
        device,
        DeviceAutomationTarget::HostOutputGain,
    );
    for (target, y) in [
        (provider, 0.4),
        (DeviceAutomationTarget::HostInputGain, 0.25),
        (DeviceAutomationTarget::HostOutputGain, 0.7),
    ] {
        draw(&mut visual, &editor, device, target, y);
    }
    let authored = editor.read_with(&visual, |view, _| {
        let state = view
            .state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state();
        assert_eq!(state.input_gain_millidb(), 0);
        assert_eq!(state.output_gain_millidb(), 0);
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .automation()
            .to_vec()
    });
    let bounds = plot(&mut visual, device, DeviceAutomationTarget::HostInputGain);
    visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::default());
    choose_label(&mut visual, &editor, "clear automation");
    pump(&mut visual, &editor);
    header_menu(&mut visual, owner);
    choose_label(&mut visual, &editor, "show used automation lanes");
    editor.read_with(&visual, |view, _| {
        assert!(
            !view
                .automation_view
                .devices
                .extra
                .iter()
                .any(|entry| entry.target.kind == DeviceAutomationTarget::HostInputGain)
        )
    });
    header_menu(&mut visual, owner);
    choose_label(&mut visual, &editor, "show all automation lanes");
    editor.update(&mut visual, |view, _| {
        let current = view.state.as_ref().unwrap().project.device(device).unwrap();
        assert!(
            current
                .automation_target(DeviceAutomationTarget::HostInputGain)
                .is_none()
        );
        assert_eq!(current.automation().len(), 2);
        assert!(
            view.automation_view
                .devices
                .extra
                .iter()
                .any(|entry| entry.target.kind == DeviceAutomationTarget::HostInputGain)
        );
        assert!(view.enqueue(Request::Undo));
    });
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .automation(),
            authored
        )
    });
}

#[gpui::test]
fn moved_host_target_refusal_retires_only_its_own_receipt_without_new_history(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, owner, _, device) = window(cx, "hex.overdrive", Slot::Track);
    let target = DeviceAutomationTarget::HostInputGain;
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "destination".into()
        })))
    });
    pump(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let revision = state.revision;
        let destination = state.project.tracks().last().unwrap().id();
        let entry = Entry::read(
            &state.project,
            view.project_generation,
            owner,
            device,
            target,
        )
        .unwrap();
        let frozen = entry.target;
        let points = vec![AutomationPoint {
            beat: BeatTime::ZERO,
            value: AutomationValue::new(HostGain::from_db(-6.0).unwrap().normalized()).unwrap(),
            outgoing: AutomationInterpolation::Linear,
        }];
        view.automation_view.devices.pending = Some(Pending {
            target: frozen,
            revision,
            points: points.clone(),
            receipt: Receipt::new(view.project_generation),
        });
        assert!(view.enqueue(Request::Edit(Command::MoveDevice {
            device,
            track: destination,
            before: None
        })));
        assert!(view.enqueue(Request::DeviceAutomation {
            project_generation: view.project_generation,
            revision,
            device,
            target,
            points
        }));
        let earlier = event(&view.service);
        assert!(earlier.error.is_none());
        view.receive(earlier);
        assert!(view.automation_view.devices.pending.is_some());
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        let own = event(&view.service);
        assert!(own.error.as_ref().unwrap().contains("project changed"));
        view.receive(own);
        assert!(view.automation_view.devices.pending.is_none());
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .automation()
                .is_empty()
        );
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump(&mut visual, &editor);
    editor.read_with(&visual,|view,_|assert!(matches!(owner,Owner::Track(id) if view.state.as_ref().unwrap().project.track(id).unwrap().devices().iter().any(|candidate|candidate.id()==device))));
}
