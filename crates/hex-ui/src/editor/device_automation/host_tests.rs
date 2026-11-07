//! Real host-control and header assignment, canonical worker edits and receipt
//! refusal. Native binary/UI/listening acceptance remains a separate user check.
use super::*;
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{Command, Created, TrackId};
use hex_session::service::Service;
use std::time::Instant;

fn event(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
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
pub(in crate::editor) enum Slot {
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
    window_with_gain(cx, type_id, slot, false, None)
}

pub(in crate::editor) fn window_with_gain(
    cx: &mut TestAppContext,
    type_id: &str,
    slot: Slot,
    with_gain: bool,
    sampler_path: Option<&std::path::Path>,
) -> (
    VisualTestContext,
    Entity<EditorView>,
    Owner,
    TrackId,
    DeviceId,
) {
    cx.update(super::super::bind_editor_keys);
    let service = if sampler_path.is_some() {
        Service::with_native_reservation(
            48_000,
            32,
            8_000_000,
            hex_session::ResidentNativeReservation {
                storage: hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 64,
                    root_rows: 64,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 30_000_000,
                },
                max_scan_frames: 1024,
            },
        )
        .unwrap()
    } else {
        Service::new(48000, 32, 8_000_000).unwrap()
    };
    event(&service);
    let track_command = if sampler_path.is_some() {
        Command::AddMidiTrack {
            name: "host automation".into(),
        }
    } else {
        Command::AddTrack {
            name: "host automation".into(),
        }
    };
    service.submit(Request::Edit(track_command)).unwrap();
    let track_event = event(&service);
    let revision = track_event.state.revision;
    let Created::Track(track) = track_event.created.unwrap() else {
        panic!("track")
    };
    if with_gain {
        service
            .submit(Request::Edit(Command::UpsertTrackGainPoint {
                track,
                point: hex_project::GainPoint {
                    incoming_gain: None,
                    beat: BeatTime::from_raw(2),
                    gain: 1.0,
                    outgoing: hex_project::GainInterpolation::Linear,
                },
            }))
            .unwrap();
        assert!(event(&service).error.is_none());
    }
    if let Some(path) = sampler_path {
        service
            .submit(Request::LoadFirstSampler {
                request_id: 1,
                project_generation: 0,
                revision,
                track,
                empty_sampler: None,
                path: path.to_path_buf(),
                cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            })
            .unwrap();
        let loaded = event(&service);
        assert!(loaded.error.is_none(), "{:?}", loaded.error);
    }
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
    let initial_state = added.state;
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
                view.state = Some(initial_state);
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
pub(in crate::editor) fn header_assign(
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
pub(in crate::editor) fn pump(visual: &mut VisualTestContext, editor: &Entity<EditorView>) {
    let deadline = crate::test_wait::hang_deadline();
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
fn host_header_picker_assigns_real_lanes_for_utility_and_other_native_owners(
    cx: &mut TestAppContext,
) {
    for type_id in ["hex.utility", "hex.overdrive"] {
        for slot in [Slot::Track, Slot::Main, Slot::Return] {
            let (mut visual, editor, owner, _, device) = window(cx, type_id, slot);
            // Host trim has no chain control; only the passive output meter follows the device.
            assert!(
                visual
                    .debug_bounds(Box::leak(
                        format!("device-{}-output-meter", device.value()).into_boxed_str()
                    ))
                    .is_some()
            );
            let before = editor.read_with(&visual, |view, _| {
                view.state.as_ref().unwrap().project.snapshot()
            });
            header_assign(
                &mut visual,
                &editor,
                owner,
                DeviceAutomationTarget::HostInputGain,
            );
            header_assign(
                &mut visual,
                &editor,
                owner,
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
    header_assign(
        &mut visual,
        &editor,
        owner,
        DeviceAutomationTarget::HostInputGain,
    );
    header_assign(
        &mut visual,
        &editor,
        owner,
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
            vst3_class: None,
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

#[gpui::test]
fn explicit_equal_numeric_text_and_real_reenable_menus_preserve_curve_and_receipt(
    cx: &mut TestAppContext,
) {
    use hex_project::AutomationReadSelection as Read;
    let (mut visual, editor, owner, _, device) = window(cx, "hex.utility", Slot::Track);
    let kind = DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(3));
    let points = vec![AutomationPoint {
        beat: BeatTime::ZERO,
        value: AutomationValue::new(0.3).unwrap(),
        outgoing: AutomationInterpolation::Step,
    }];
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let revision = view.state.as_ref().unwrap().revision;
            assert!(view.enqueue(Request::DeviceAutomation {
                project_generation: view.project_generation,
                revision,
                device,
                target: kind,
                vst3_class: None,
                points: points.clone()
            }));
            let entry = Entry::read(
                &view.state.as_ref().unwrap().project,
                view.project_generation,
                owner,
                device,
                kind,
            )
            .unwrap();
            view.automation_menu_action(Action::Show(entry, true), window, cx);
        })
    });
    pump(&mut visual, &editor);
    let original = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.clone()
    });
    let selector: &'static str =
        Box::leak(format!("device-{}-parameter-3-value", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("tab");
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project,
            original,
            "passive focus/blur is not an override"
        )
    });
    let bounds = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("100%");
    visual.simulate_keystrokes("enter");
    pump(&mut visual, &editor);
    let manual = editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = view.state.as_ref().unwrap().project.clone();
        let lane = project
            .device(device)
            .unwrap()
            .automation_target(kind)
            .unwrap();
        assert_eq!(lane.read_selection, Read::Base);
        assert_eq!(lane.points, points);
        assert_eq!(
            project.device(device).unwrap().state(),
            original.device(device).unwrap().state(),
            "equal base text still overrides"
        );
        project
    });
    assert!(visual.debug_bounds("reenable-device-automation").is_some());
    let bounds = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::default());
    choose_label(&mut visual, &editor, "re-enable automation");
    for _ in 0..2 {
        editor.update(&mut visual, |view, cx| {
            assert!(view.automation_view.devices.read_pending.is_some());
            assert_eq!(view.state.as_ref().unwrap().project, manual);
            let target = view.automation_view.devices.extra[0].target;
            assert_eq!(
                view.device_automation_read_selection(target),
                Some(Read::Authored)
            );
            cx.notify();
        });
        visual.run_until_parked();
    }
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert!(view.automation_view.devices.read_pending.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, manual)
    });
    let bounds = plot(&mut visual, device, kind);
    visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::default());
    choose_label(&mut visual, &editor, "re-enable automation");
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&mut visual, &editor);
    let bounds = visual.debug_bounds("reenable-device-automation").unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
}

#[gpui::test]
fn stale_reenable_keeps_pending_selection_until_its_terminal_then_restores_canonical(
    cx: &mut TestAppContext,
) {
    use hex_project::AutomationReadSelection as Read;
    use hex_session::service::DeviceAutomationControlEdit as Edit;
    let (mut visual, editor, owner, track, device) = window(cx, "hex.utility", Slot::Track);
    let target = DeviceAutomationTarget::HostInputGain;
    editor.update(&mut visual, |view, _| {
        let revision = view.state.as_ref().unwrap().revision;
        assert!(view.enqueue(Request::DeviceAutomation {
            project_generation: view.project_generation,
            revision,
            device,
            target,
            vst3_class: None,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Step
            }]
        }));
    });
    pump(&mut visual, &editor);
    editor.update(&mut visual, |view, _| {
        let request = view
            .manual_device_parameter_request(
                device,
                hex_engine::DEVICE_INPUT_GAIN.id,
                ParameterValue::Float(-6.0),
            )
            .unwrap();
        assert!(view.enqueue(request));
    });
    pump(&mut visual, &editor);
    editor.update(&mut visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        let generation = view.project_generation;
        let revision = state.revision;
        let entry = Entry::read(&state.project, generation, owner, device, target).unwrap();
        assert_eq!(
            view.device_automation_read_selection(entry.target),
            Some(Read::Base)
        );
        assert!(view.enqueue(Request::Edit(Command::Gain { track, gain: 0.5 })));
        let edit = Edit::ReadSelection {
            device,
            target,
            vst3_class: None,
            selection: Read::Authored,
        };
        view.automation_view.devices.read_pending = Some(super::read_selection::PendingRead {
            generation,
            revision,
            edit: edit.clone(),
            receipt: super::Receipt::new(generation),
        });
        assert!(view.enqueue(Request::DeviceAutomationControl {
            project_generation: generation,
            revision,
            edit
        }));
    });
    for _ in 0..2 {
        editor.update(&mut visual, |view, cx| {
            assert!(view.automation_view.devices.read_pending.is_some());
            cx.notify();
        });
        visual.run_until_parked();
    }
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert!(view.automation_view.devices.read_pending.is_none());
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("project changed"))
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .automation_target(target)
                .unwrap()
                .read_selection,
            Read::Base
        );
    });
}

pub(in crate::editor) const CMD: Modifiers = Modifiers {
    control: false,
    alt: false,
    shift: false,
    platform: true,
    function: false,
};
const SHIFT: Modifiers = Modifiers {
    control: false,
    alt: false,
    shift: true,
    platform: false,
    function: false,
};
const SHIFT_CMD: Modifiers = Modifiers {
    platform: true,
    ..SHIFT
};

pub(in crate::editor) fn lane_point(
    visual: &mut VisualTestContext,
    device: DeviceId,
    target: DeviceAutomationTarget,
    [x, y]: [f32; 2],
) -> gpui::Point<Pixels> {
    let bounds = plot(visual, device, target);
    point(
        bounds.left() + bounds.size.width * x,
        bounds.top() + bounds.size.height * y,
    )
}

pub(in crate::editor) fn lane_points(
    visual: &VisualTestContext,
    editor: &Entity<EditorView>,
    device: DeviceId,
    target: DeviceAutomationTarget,
) -> Vec<AutomationPoint> {
    editor.read_with(visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .map(|lane| lane.points.clone())
            .unwrap_or_default()
    })
}

fn selected_beats(visual: &VisualTestContext, editor: &Entity<EditorView>) -> Vec<BeatTime> {
    editor.read_with(visual, |view, _| {
        view.automation_view
            .devices
            .selected
            .as_ref()
            .map(|selected| selected.beats.clone())
            .unwrap_or_default()
    })
}

fn lane_drag(
    visual: &mut VisualTestContext,
    from: gpui::Point<Pixels>,
    to: gpui::Point<Pixels>,
    modifiers: Modifiers,
) {
    visual.simulate_mouse_down(from, MouseButton::Left, modifiers);
    visual.simulate_mouse_move(to, Some(MouseButton::Left), modifiers);
    visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
    visual.run_until_parked();
}

/// Three unsnapped points (Cmd) at known pixels, so presses hit them exactly.
fn three_point_lane(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    Entity<EditorView>,
    DeviceId,
    DeviceAutomationTarget,
    Vec<AutomationPoint>,
) {
    let (mut visual, editor, owner, _, device) = window(cx, "hex.utility", Slot::Track);
    let target =
        DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1));
    header_assign(&mut visual, &editor, owner, target);
    for x in [0.2, 0.4, 0.8] {
        let at = lane_point(&mut visual, device, target, [x, 0.5]);
        visual.simulate_click(at, CMD);
        pump(&mut visual, &editor);
    }
    let points = lane_points(&visual, &editor, device, target);
    assert_eq!(points.len(), 3, "blank clicks still add points");
    (visual, editor, device, target, points)
}

#[gpui::test]
fn lane_shift_marquee_selects_inside_points_while_shift_cmd_toggles(cx: &mut TestAppContext) {
    let (mut visual, editor, device, target, points) = three_point_lane(cx);
    let beats: Vec<_> = points.iter().map(|point| point.beat).collect();
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    // Drag right-to-left over the first two points.
    let from = lane_point(&mut visual, device, target, [0.5, 0.95]);
    let to = lane_point(&mut visual, device, target, [0.1, 0.05]);
    lane_drag(&mut visual, from, to, SHIFT);
    assert_eq!(selected_beats(&visual, &editor), beats[..2]);
    let from = lane_point(&mut visual, device, target, [0.7, 0.05]);
    let to = lane_point(&mut visual, device, target, [0.9, 0.95]);
    lane_drag(&mut visual, from, to, SHIFT_CMD);
    assert_eq!(selected_beats(&visual, &editor), beats);
    let from = lane_point(&mut visual, device, target, [0.1, 0.05]);
    let to = lane_point(&mut visual, device, target, [0.3, 0.95]);
    lane_drag(&mut visual, from, to, SHIFT_CMD);
    assert_eq!(selected_beats(&visual, &editor), beats[1..]);
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().revision,
            before,
            "marquee selection is view state and authors nothing"
        );
    });
    assert_eq!(lane_points(&visual, &editor, device, target), points);
}

#[gpui::test]
fn lane_group_move_and_delete_each_undo_in_one_step(cx: &mut TestAppContext) {
    let (mut visual, editor, device, target, points) = three_point_lane(cx);
    let from = lane_point(&mut visual, device, target, [0.1, 0.05]);
    let to = lane_point(&mut visual, device, target, [0.5, 0.95]);
    lane_drag(&mut visual, from, to, SHIFT);
    assert_eq!(selected_beats(&visual, &editor).len(), 2);

    // Dragging one selected point moves both by the same value delta.
    let first = lane_point(&mut visual, device, target, [0.2, 0.5]);
    let lower = point(first.x, first.y + px(12.0));
    lane_drag(&mut visual, first, lower, CMD);
    pump(&mut visual, &editor);
    let moved = lane_points(&visual, &editor, device, target);
    assert_eq!(moved.len(), 3);
    let deltas: Vec<f32> = moved
        .iter()
        .zip(&points)
        .map(|(moved, original)| {
            assert_eq!(moved.beat, original.beat, "a vertical drag keeps beats");
            moved.value.get() - original.value.get()
        })
        .collect();
    assert!(deltas[0] < -0.05, "{deltas:?}");
    assert!((deltas[0] - deltas[1]).abs() < 1e-4, "{deltas:?}");
    assert_eq!(deltas[2], 0.0, "unselected points stay put");
    assert_eq!(selected_beats(&visual, &editor).len(), 2);
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&mut visual, &editor);
    assert_eq!(lane_points(&visual, &editor, device, target), points);

    let from = lane_point(&mut visual, device, target, [0.3, 0.05]);
    let to = lane_point(&mut visual, device, target, [0.9, 0.95]);
    lane_drag(&mut visual, from, to, SHIFT);
    assert_eq!(selected_beats(&visual, &editor).len(), 2);
    editor.update(&mut visual, |view, cx| {
        assert!(view.delete_device_automation_point(cx))
    });
    pump(&mut visual, &editor);
    assert_eq!(lane_points(&visual, &editor, device, target), points[..1]);
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&mut visual, &editor);
    assert_eq!(lane_points(&visual, &editor, device, target), points);
}

#[gpui::test]
fn lane_plain_blank_drag_adds_a_point_and_drags_it_without_a_marquee(cx: &mut TestAppContext) {
    let (mut visual, editor, device, target, points) = three_point_lane(cx);
    // Unsnapped (Cmd) so the dragged point lands exactly under the pointer.
    let from = lane_point(&mut visual, device, target, [0.6, 0.5]);
    let to = lane_point(&mut visual, device, target, [0.6, 0.2]);
    visual.simulate_mouse_down(from, MouseButton::Left, CMD);
    editor.read_with(&visual, |view, _| {
        assert!(view.automation_view.devices.marquee.is_none());
        assert!(view.automation_view.devices.draft.is_some());
    });
    // Shift arriving after the press must not turn the gesture into a band.
    visual.simulate_mouse_move(to, Some(MouseButton::Left), SHIFT_CMD);
    visual.simulate_mouse_up(to, MouseButton::Left, SHIFT_CMD);
    visual.run_until_parked();
    pump(&mut visual, &editor);
    let after = lane_points(&visual, &editor, device, target);
    assert_eq!(after.len(), points.len() + 1, "the press added one point");
    let added = after
        .iter()
        .find(|point| !points.contains(point))
        .expect("new point");
    assert!(added.value.get() > 0.7, "the drag raised the new point");
    assert_eq!(selected_beats(&visual, &editor), [added.beat]);
}

#[gpui::test]
fn unrelated_edit_refuses_during_automation_gesture_but_release_commits(cx: &mut TestAppContext) {
    let (mut visual, editor, owner, track, device) = window(cx, "hex.utility", Slot::Track);
    let target =
        DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1));
    header_assign(&mut visual, &editor, owner, target);
    let at = lane_point(&mut visual, device, target, [0.6, 0.5]);
    visual.simulate_mouse_down(at, MouseButton::Left, CMD);
    editor.update(&mut visual, |view, _| {
        assert!(view.automation_view.devices.draft.is_some());
        assert!(!view.enqueue(Request::Edit(Command::Mute { track, muted: true })));
        assert!(view.automation_view.devices.draft.is_some());
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("automation point gesture")
        );
        let generation = view.project_generation;
        let revision = view.state.as_ref().unwrap().revision;
        assert!(!view.enqueue(Request::GainLane {
            project_generation: generation,
            revision,
            track,
            edit: hex_session::service::GainLaneEdit::Clear,
        }));
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.error.as_deref().unwrap().contains("local draft"));
    });
    visual.simulate_mouse_up(at, MouseButton::Left, CMD);
    visual.run_until_parked();
    pump(&mut visual, &editor);
    assert_eq!(lane_points(&visual, &editor, device, target).len(), 1);
}

#[gpui::test]
fn selection_marquee_does_not_block_an_unrelated_edit(cx: &mut TestAppContext) {
    let (mut visual, editor, owner, track, device) = window(cx, "hex.utility", Slot::Track);
    let target =
        DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1));
    header_assign(&mut visual, &editor, owner, target);
    let at = lane_point(&mut visual, device, target, [0.6, 0.5]);
    visual.simulate_mouse_down(at, MouseButton::Left, SHIFT);
    editor.update(&mut visual, |view, _| {
        assert!(view.automation_view.devices.marquee.is_some());
        assert!(view.automation_view.devices.draft.is_none());
        assert!(view.enqueue(Request::Edit(Command::Mute { track, muted: true })));
    });
    visual.simulate_mouse_up(at, MouseButton::Left, SHIFT);
}

#[gpui::test]
fn invalid_gain_text_blocks_starting_an_automation_gesture(cx: &mut TestAppContext) {
    let (mut visual, editor, owner, track, device) =
        window_with_gain(cx, "hex.utility", Slot::Track, true, None);
    let target =
        DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1));
    editor.update(&mut visual, |view, cx| {
        view.clip_view_open = false;
        view.gain_lane.track = Some(track);
        view.gain_lane.selected = Some(BeatTime::from_raw(2));
        cx.notify();
    });
    visual.run_until_parked();
    let gain = visual
        .debug_bounds("gain-point-value-value")
        .unwrap()
        .center();
    visual.simulate_click(gain, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    header_assign(&mut visual, &editor, owner, target);
    let at = lane_point(&mut visual, device, target, [0.6, 0.5]);
    visual.simulate_mouse_down(at, MouseButton::Left, CMD);
    editor.read_with(&visual, |view, cx| {
        assert!(view.gain_numeric_editing(cx));
        assert!(view.automation_view.devices.draft.is_none());
    });
    visual.simulate_mouse_up(at, MouseButton::Left, CMD);
    editor.update(&mut visual, |view, cx| {
        assert!(view.cancel_gain_draft(cx));
    });
    visual.simulate_mouse_down(at, MouseButton::Left, CMD);
    visual.simulate_mouse_up(at, MouseButton::Left, CMD);
    editor.read_with(&visual, |view, _| {
        assert!(view.automation_view.devices.pending.is_some())
    });
}

pub(in crate::editor) struct TestWave(pub(in crate::editor) std::path::PathBuf);
impl TestWave {
    pub(in crate::editor) fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-draft-test-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&44_u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&96_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8_u32.to_le_bytes());
        wav.extend_from_slice(&[0_u8; 8]);
        use std::io::Write;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap()
            .write_all(&wav)
            .unwrap();
        Self(path)
    }
}
impl Drop for TestWave {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The bound control displays the in-flight gesture value before canonical state.
fn utility_gain_value(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    device: DeviceId,
) -> (Option<ParameterValue>, ParameterValue) {
    let parameter = hex_lattice_contract::ParameterId::from_static(1);
    editor.read_with(visual, |view, _| {
        let canonical = hex_engine::native_device_parameter(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state(),
            parameter,
        )
        .unwrap();
        (
            view.device_gesture
                .snapshot()
                .map(|snapshot| snapshot.value),
            canonical,
        )
    })
}

fn dotted_top(visual: &mut VisualTestContext, device: DeviceId) -> Option<Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("automation-dotted-{}-1-true", device.value()).into_boxed_str(),
        ))
        .map(|bounds| bounds.top())
}

#[gpui::test]
fn last_touched_dotted_line_follows_gesture_preview_through_delayed_ack(cx: &mut TestAppContext) {
    let (mut visual, editor, owner, _, device) = window(cx, "hex.utility", Slot::Track);
    let kind = DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1));
    editor.update(&mut visual, |view, cx| {
        view.automation_view.enabled = true;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(editor.read_with(&visual, |view, _| {
        !view.automation_view.devices.has_main(owner)
    }));
    assert!(dotted_top(&mut visual, device).is_none());
    let (_, initial) = utility_gain_value(&mut visual, &editor, device);
    let knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-knob", device.value()).into_boxed_str(),
        ))
        .unwrap();
    let start = knob.center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    let mut previous = None;
    let mut end = start;
    for step in 1..=3 {
        end = point(start.x, start.y - px(20.0 * step as f32));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        let (control, canonical) = utility_gain_value(&mut visual, &editor, device);
        assert_eq!(canonical, initial, "drag frames stay uncommitted");
        let expected = editor.read_with(&visual, |view, _| {
            let entry = view
                .automation_view
                .devices
                .main
                .iter()
                .find(|entry| entry.target.owner == owner)
                .expect("touching the control selects its lane");
            assert_eq!(entry.target.kind, kind);
            entry.normalized(control.expect("gesture preview")).unwrap()
        });
        let plot = visual
            .debug_bounds(Box::leak(
                format!("automation-plot-{}-1-true", device.value()).into_boxed_str(),
            ))
            .unwrap();
        let top = dotted_top(&mut visual, device).expect("unautomated lane is dotted");
        let wanted =
            f32::from(plot.top()) + 4.0 + (1.0 - expected) * (f32::from(plot.size.height) - 8.0);
        assert!((f32::from(top) - wanted).abs() < 0.5, "{top:?} vs {wanted}");
        assert!(
            previous.is_none_or(|previous| top < previous),
            "line follows each frame"
        );
        previous = Some(top);
    }
    let released = previous.unwrap();
    let (final_value, _) = utility_gain_value(&mut visual, &editor, device);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(&mut visual, |view, cx| {
            assert!(
                view.device_gesture.snapshot().is_some(),
                "ack still delayed"
            );
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(dotted_top(&mut visual, device), Some(released));
        assert_eq!(utility_gain_value(&mut visual, &editor, device).1, initial);
    }
    pump(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert!(view.device_gesture.snapshot().is_none())
    });
    assert_eq!(
        utility_gain_value(&mut visual, &editor, device).1,
        final_value.unwrap()
    );
    let installed = dotted_top(&mut visual, device).unwrap();
    assert!((f32::from(installed) - f32::from(released)).abs() < 0.5);
}

#[gpui::test]
fn touching_an_automated_parameter_shows_its_envelope_unmoved(cx: &mut TestAppContext) {
    let (mut visual, editor, owner, _, device) = window(cx, "hex.utility", Slot::Track);
    let kind = DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1));
    let points = vec![AutomationPoint {
        beat: BeatTime::ZERO,
        value: AutomationValue::new(0.75).unwrap(),
        outgoing: AutomationInterpolation::Step,
    }];
    editor.update(&mut visual, |view, cx| {
        let revision = view.state.as_ref().unwrap().revision;
        assert!(view.enqueue(Request::DeviceAutomation {
            project_generation: view.project_generation,
            revision,
            device,
            target: kind,
            vst3_class: None,
            points: points.clone()
        }));
        view.automation_view.enabled = true;
        cx.notify();
    });
    pump(&mut visual, &editor);
    let knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-knob", device.value()).into_boxed_str(),
        ))
        .unwrap();
    let start = knob.center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x, start.y - px(16.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        let entry = view
            .automation_view
            .devices
            .main
            .iter()
            .find(|entry| entry.target.owner == owner)
            .unwrap();
        assert_eq!(entry.target.kind, kind);
        assert_eq!(
            view.automation_view.devices.points(state, entry.target),
            points
        );
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("automation-plot-{}-1-true", device.value()).into_boxed_str(),
            ))
            .is_some()
    );
    assert!(
        dotted_top(&mut visual, device).is_none(),
        "envelope replaces the dotted line"
    );
    visual.simulate_mouse_up(
        point(start.x, start.y - px(16.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump(&mut visual, &editor);
}

#[gpui::test]
fn clearing_a_recorded_only_lane_removes_the_take_with_one_undo(cx: &mut TestAppContext) {
    use hex_project::{DeviceAutomationTouchSpan, DeviceAutomationTouchStep};
    let (mut visual, editor, owner, _, device) = window(cx, "hex.utility", Slot::Track);
    let kind = DeviceAutomationTarget::HostOutputGain;
    let start = BeatTime::from_ratio(1, 64).unwrap();
    let touch = vec![DeviceAutomationTouchSpan {
        start,
        end: BeatTime::from_ratio(1, 32).unwrap(),
        steps: vec![DeviceAutomationTouchStep {
            beat: start,
            value: AutomationValue::new(0.25).unwrap(),
        }],
    }];
    editor.update(&mut visual, |view, _| {
        assert!(
            view.enqueue(Request::Edit(Command::SpliceDeviceAutomationTouch {
                device,
                target: kind,
                spans: touch.clone(),
            }))
        );
    });
    pump(&mut visual, &editor);
    let target = visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let state = view.state.as_ref().unwrap();
            let lane = state
                .project
                .device(device)
                .unwrap()
                .automation_target(kind)
                .unwrap();
            assert!(lane.points.is_empty());
            assert_eq!(lane.touch, touch);
            let entry =
                Entry::read(&state.project, view.project_generation, owner, device, kind).unwrap();
            assert_eq!(entry.end, Some(touch[0].end));
            let target = entry.target;
            view.automation_view.enabled = true;
            view.automation_menu_action(Action::Show(entry, true), window, cx);
            target
        })
    });
    pump(&mut visual, &editor);
    let dotted: &'static str = Box::leak(
        format!(
            "automation-dotted-{}-{}-false",
            device.value(),
            target.key()
        )
        .into_boxed_str(),
    );
    let center = plot(&mut visual, device, kind).center();
    assert!(visual.debug_bounds(dotted).is_none());
    visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
    editor.update(&mut visual, |view, cx| {
        assert_eq!(
            view.error.as_deref(),
            Some("clear recorded automation before editing this lane"),
        );
        assert!(!view.automation_view.devices.pointer_active());
        view.commit_device_automation(target, Vec::new(), cx);
    });
    pump(&mut visual, &editor);
    assert!(visual.debug_bounds(dotted).is_some());
    editor.update(&mut visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert!(
            state
                .project
                .device(device)
                .unwrap()
                .automation_target(kind)
                .is_none()
        );
        assert!(view.enqueue(Request::Undo));
    });
    pump(&mut visual, &editor);
    assert!(visual.debug_bounds(dotted).is_none());
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        let lane = state
            .project
            .device(device)
            .unwrap()
            .automation_target(kind)
            .unwrap();
        assert!(lane.points.is_empty());
        assert_eq!(lane.touch, touch);
    });
}
