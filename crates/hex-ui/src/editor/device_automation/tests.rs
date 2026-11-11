use super::*;
use hex_lattice_contract::ParameterId;
use hex_project::{Command, Created, Editor};
fn project() -> (Project, hex_project::TrackId, DeviceId, ParameterId) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "automation".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    (
        editor.project().clone(),
        track,
        device,
        ParameterId::new(1).unwrap(),
    )
}
#[test]
fn parameter_identity_quantization_and_empty_presentation_preserve_canonical_state() {
    let (project, track, device, parameter) = project();
    let entry = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(parameter),
    )
    .unwrap();
    assert!(entry.target.current(&project, 5).is_none());
    assert!(
        Entry::read(
            &project,
            4,
            Owner::Main,
            device,
            DeviceAutomationTarget::Parameter(parameter)
        )
        .is_none()
    );
    assert!(entry.quantize(0.25).unwrap().is_finite());
    let boolean = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(ParameterId::from_static(5)),
    )
    .unwrap();
    assert_eq!(boolean.quantize(0.49), Some(0.0));
    assert_eq!(boolean.quantize(0.5), Some(1.0));
    let choice = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(ParameterId::from_static(4)),
    )
    .unwrap();
    let normalized = choice.quantize(0.4).unwrap();
    assert_eq!(choice.quantize(normalized), Some(normalized));
    assert!(entry.quantize(f32::NAN).is_none());
    let lanes = Lanes::default();
    assert!(!lanes.has_main(Owner::Track(track)));
    assert_eq!(lanes.extra_height(Owner::Track(track)), 0.0);
    assert!(project.device(device).unwrap().automation().is_empty());
    let mut model = Editor::new(project, 16).unwrap();
    model.execute(Command::RemoveDevice(device)).unwrap();
    assert!(entry.target.current(model.project(), 4).is_none());
}

#[test]
fn host_metadata_has_no_provider_id_and_keeps_identity_across_native_provider_change() {
    let (project, track, device, _) = project();
    let host = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::HostInputGain,
    )
    .unwrap();
    assert_eq!(host.base, HostGain::ZERO.normalized());
    assert_eq!(host.quantize(host.base), Some(host.base));
    assert_eq!(host.quantize(1.01), None);
    assert_eq!(host.quantize(f32::NAN), None);
    assert_eq!(host.format(host.base), "0.000 db");
    assert_eq!(host.target.provider, None);
    assert!(
        Entry::read(
            &project,
            4,
            Owner::Track(track),
            device,
            DeviceAutomationTarget::Parameter(hex_engine::DEVICE_INPUT_GAIN.id)
        )
        .is_none()
    );
    let mut editor = Editor::new(project, 16).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.overdrive",
            ))
            .unwrap(),
        })
        .unwrap();
    let refreshed = host.refreshed(editor.project(), 4).unwrap();
    assert_eq!(refreshed.target, host.target);
    assert!(refreshed.label.contains("overdrive"));
}

#[cfg(feature = "ui-test-support")]
mod native {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, px, size};
    use std::sync::Arc;
    #[gpui::test]
    fn old_painted_parameter_cannot_retarget_a_moved_device_before_repaint(
        cx: &mut TestAppContext,
    ) {
        cx.update(super::super::super::bind_editor_keys);
        let (project, track, device, parameter) = project();
        let mut model = Editor::new(project, 16).unwrap();
        let Created::Track(next) = model
            .execute(Command::AddTrack {
                name: "next".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let (handle, entity) = super::super::super::clip_drag_ui_test::editor_window(
            cx,
            model.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1100.0), px(760.0)));
        visual.update(|window, cx| {
            entity.update(cx, |view, cx| {
                view.select_track(track, window);
                view.device_panel_active = true;
                view.clip_view_open = true;
                cx.notify();
            })
        });
        visual.run_until_parked();
        let point = visual
            .debug_bounds(Box::leak(
                format!(
                    "device-{}-parameter-{}-knob",
                    device.value(),
                    parameter.value()
                )
                .into_boxed_str(),
            ))
            .unwrap()
            .center();
        entity.update(&mut visual, |view, _| {
            model
                .execute(Command::MoveDevice {
                    device,
                    track: next,
                    before: None,
                })
                .unwrap();
            let mut event = super::super::super::arrangement_main_track_ui_test::completed_event(
                view.state.as_ref().unwrap(),
            );
            Arc::get_mut(&mut event.state).unwrap().project = Arc::new(model.project().clone());
            // Publish new source/owner without notifying: the previous frame's
            // actual pointer listener must refuse instead of rebinding this ID.
            view.state = Some(event.state);
            view.selected_track = Some(next);
            view.device_panel.owner = Some(Owner::Track(next));
            assert!(
                Entry::read(
                    &view.state.as_ref().unwrap().project,
                    view.project_generation,
                    Owner::Track(next),
                    device,
                    DeviceAutomationTarget::Parameter(parameter)
                )
                .is_some()
            );
        });
        visual.simulate_mouse_down(point, MouseButton::Right, Modifiers::default());
        entity.read_with(&visual, |view, _| {
            assert!(
                view.context_menu.is_none(),
                "a stale painted binding must not adopt a moved device's new owner"
            )
        });
    }

    #[gpui::test]
    fn device_drop_in_attached_lane_keeps_track_owner_and_true_insertion_boundary(
        cx: &mut TestAppContext,
    ) {
        cx.update(super::super::super::bind_editor_keys);
        let (project, track, device, parameter) = project();
        let mut model = Editor::new(project, 16).unwrap();
        let Created::Track(next) = model
            .execute(Command::AddTrack {
                name: "next".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let (handle, entity) = super::super::super::clip_drag_ui_test::editor_window(
            cx,
            model.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1100.0), px(760.0)));
        visual.update(|window, cx| {
            entity.update(cx, |view, cx| {
                let entry = Entry::read(
                    &view.state.as_ref().unwrap().project,
                    view.project_generation,
                    Owner::Track(track),
                    device,
                    DeviceAutomationTarget::Parameter(parameter),
                )
                .unwrap();
                view.automation_menu_action(Action::Show(entry, true), window, cx);
                view.browser.section = super::super::super::browser::BrowserSection::Instruments;
                view.clip_view_open = false;
                cx.notify();
            })
        });
        visual.run_until_parked();
        let plot = visual
            .debug_bounds(Box::leak(
                format!(
                    "automation-plot-{}-{}-false",
                    device.value(),
                    parameter.value()
                )
                .into_boxed_str(),
            ))
            .unwrap();
        let landing = plot.center();
        entity.read_with(&visual,|view,_|{
            let body=view.drag_lanes[&track];
            let total=view.painted_track_drop_lane(track).unwrap();
            assert_eq!(body.height,super::super::super::tracks::TRACK_ROW_HEIGHT);
            assert_eq!(total.height,body.height+LANE_HEIGHT);
            assert!(view.hovered_drag_target(f32::from(landing.x),f32::from(landing.y)).is_none(),"clip editing retains its base body bounds");
            assert!(matches!(view.painted_arrangement_drop_target(f32::from(landing.x),f32::from(landing.y)),Some(super::super::super::arrangement_device_drop::ArrangementDropTarget::ExistingTrack { track:target,.. }) if target==track));
            let next_lane=view.painted_track_drop_lane(next).unwrap();
            assert_eq!(view.painted_track_insertion_target(f32::from(landing.x),next_lane.top).unwrap().0,Some(next));
        });
        let source = visual
            .debug_bounds("browser-device-effect-hex.utility")
            .unwrap()
            .center();
        visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            source + gpui::point(px(12.0), px(0.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        visual.simulate_mouse_move(landing, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
        entity.read_with(&visual, |view, _| {
            assert_eq!(
                view.last_enqueued_arrangement_device_drop,
                Some(
                    hex_session::service::ArrangementDeviceDestination::ExistingTrack {
                        track,
                        instrument: None
                    }
                )
            )
        });
    }

    #[gpui::test]
    fn track_main_and_return_rows_keep_compact_geometry_and_empty_reveal_is_view_only(
        cx: &mut TestAppContext,
    ) {
        cx.update(super::super::super::bind_editor_keys);
        let (project, track, device, parameter) = project();
        let mut model = Editor::new(project, 16).unwrap();
        let Created::ReturnTrack(return_id) = model
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        let state = || {
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                .unwrap()
        };
        let Created::Device(return_device) = model
            .execute(Command::InsertReturnDevice {
                return_track: return_id,
                before: None,
                state: state(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let Created::Device(main_device) = model
            .execute(Command::InsertMainDevice {
                before: None,
                state: state(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let (handle, entity) = super::super::super::clip_drag_ui_test::editor_window(
            cx,
            model.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1000.0), px(760.0)));
        visual.update(|window, cx| {
            entity.update(cx, |view, cx| {
                for (owner, device) in [
                    (Owner::Track(track), device),
                    (Owner::Main, main_device),
                    (Owner::Return(return_id), return_device),
                ] {
                    let entry = Entry::read(
                        &view.state.as_ref().unwrap().project,
                        view.project_generation,
                        owner,
                        device,
                        DeviceAutomationTarget::Parameter(parameter),
                    )
                    .unwrap();
                    view.automation_menu_action(Action::Show(entry, true), window, cx);
                }
                let target = view
                    .automation_view
                    .devices
                    .extra
                    .iter()
                    .find(|entry| entry.target.owner == Owner::Main)
                    .unwrap()
                    .target;
                view.automation_menu_action(Action::Hide(target), window, cx);
                view.automation_menu_action(
                    Action::Reveal {
                        generation: view.project_generation,
                        owner: Owner::Main,
                        used_only: true,
                    },
                    window,
                    cx,
                );
                assert_eq!(view.automation_view.devices.extra_height(Owner::Main), 0.0);
                view.automation_menu_action(
                    Action::Reveal {
                        generation: view.project_generation,
                        owner: Owner::Main,
                        used_only: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.automation_view.devices.extra_height(Owner::Main),
                    LANE_HEIGHT
                );
                assert!(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .automation_devices()
                        .all(|device| device.automation().is_empty())
                );
                assert_eq!(view.worker_requests_pending, 0);
                view.clip_view_open = false;
                cx.notify();
            })
        });
        visual.run_until_parked();
        let track_header = visual
            .debug_bounds(Box::leak(
                format!("arrangement-track-header-{}", track.value()).into_boxed_str(),
            ))
            .unwrap();
        assert_eq!(
            track_header.size.height,
            px(super::super::super::tracks::TRACK_ROW_HEIGHT)
        );
        let main_header = visual.debug_bounds("main-track-header").unwrap();
        for device in [device, return_device, main_device] {
            let plot = visual
                .debug_bounds(Box::leak(
                    format!(
                        "automation-plot-{}-{}-false",
                        device.value(),
                        parameter.value()
                    )
                    .into_boxed_str(),
                ))
                .unwrap();
            assert_eq!(plot.right(), main_header.left());
            let row = visual
                .debug_bounds(Box::leak(
                    format!("automation-lane-{}-{}", device.value(), parameter.value())
                        .into_boxed_str(),
                ))
                .unwrap();
            assert_eq!(row.size.height, px(LANE_HEIGHT));
            assert_eq!(plot.size.height, row.size.height - px(1.0));
        }
        visual.simulate_resize(size(px(880.0), px(540.0)));
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("arrangement-content")
                .unwrap()
                .size
                .height
                > px(1.0)
        );
        assert!(
            visual
                .debug_bounds("pinned-return-tracks")
                .unwrap()
                .size
                .height
                <= px(88.0)
        );
    }

    #[gpui::test]
    fn owned_parameter_labels_scroll_with_keyboard_and_deleted_target_menu_retires(
        cx: &mut TestAppContext,
    ) {
        cx.update(super::super::super::bind_editor_keys);
        let (project, track, device, parameter) = project();
        let (handle, entity) =
            super::super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(880.0), px(540.0)));
        visual.update(|window, cx| {
            entity.update(cx, |view, cx| {
                let entry = Entry::read(
                    &view.state.as_ref().unwrap().project,
                    view.project_generation,
                    Owner::Track(track),
                    device,
                    DeviceAutomationTarget::Parameter(parameter),
                )
                .unwrap();
                let actions = (0..64)
                    .map(|index| {
                        let mut entry = entry.clone();
                        entry.label = format!("real device {index} · parameter").into();
                        ("parameter", Action::Show(entry, true))
                    })
                    .collect();
                view.open_automation_menu(gpui::point(px(600.0), px(450.0)), actions, window, cx);
                let menu = view.context_menu.as_ref().unwrap();
                assert_eq!(
                    super::super::super::context_menu::context_item_label(&menu.items[0]).as_ref(),
                    "real device 0 · parameter"
                );
            })
        });
        visual.run_until_parked();
        let panel = visual.debug_bounds("context-menu-panel").unwrap();
        assert!(panel.bottom() <= px(540.0));
        visual.simulate_keystrokes("up");
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            let menu = view.context_menu.as_ref().unwrap();
            assert_eq!(menu.selected, 63);
            assert!(
                menu.scroll.offset().y < px(0.0),
                "keyboard navigation reveals the selected last row"
            );
            let mut model =
                Editor::new(view.state.as_ref().unwrap().project.as_ref().clone(), 16).unwrap();
            model.execute(Command::RemoveDevice(device)).unwrap();
            let mut event = super::super::super::arrangement_main_track_ui_test::completed_event(
                view.state.as_ref().unwrap(),
            );
            event.request_complete = false;
            Arc::get_mut(&mut event.state).unwrap().project = Arc::new(model.project().clone());
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        entity.read_with(&visual, |view, _| assert!(view.context_menu.is_none()));
    }

    #[gpui::test]
    fn shared_parameter_right_click_creates_an_empty_lane_without_a_project_edit(
        cx: &mut TestAppContext,
    ) {
        shared_parameter_right_click(DeviceTypeId::from_static("hex.utility"), cx);
    }
    #[gpui::test]
    fn saturator_dry_wet_uses_the_same_parameter_menu_and_empty_lane_path(cx: &mut TestAppContext) {
        shared_parameter_right_click(DeviceTypeId::from_static("hex.saturator"), cx);
    }
    #[gpui::test]
    fn auto_pan_phase_uses_the_shared_parameter_menu_and_empty_lane_path(cx: &mut TestAppContext) {
        shared_parameter_right_click(DeviceTypeId::from_static("hex.auto-pan"), cx);
    }
    fn shared_parameter_right_click(provider: DeviceTypeId, cx: &mut TestAppContext) {
        cx.update(super::super::super::bind_editor_keys);
        let (mut project, track, device, mut parameter) = project();
        if provider != DeviceTypeId::from_static("hex.utility") {
            let mut model = Editor::new(project, 16).unwrap();
            model
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_engine::default_native_device_state(provider).unwrap(),
                })
                .unwrap();
            project = model.project().clone();
            parameter = ParameterId::new(3).unwrap();
        }
        let (handle, entity) =
            super::super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1100.0), px(760.0)));
        visual.update(|window, cx| {
            entity.update(cx, |view, cx| {
                view.select_track(track, window);
                view.device_panel_active = true;
                view.clip_view_open = true;
                cx.notify();
            })
        });
        visual.run_until_parked();
        let selector = format!(
            "device-{}-parameter-{}-knob",
            device.value(),
            parameter.value()
        );
        let bounds = visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        entity.read_with(&visual, |view, _| {
            let menu = view.context_menu.as_ref().unwrap();
            assert_eq!(menu.items[0].label, "show automation");
            assert_eq!(menu.items[1].label, "add new automation lane");
        });
        visual.simulate_keystrokes("down enter");
        visual.run_until_parked();
        entity.read_with(&visual, |view, _| {
            assert!(view.context_menu.is_none());
            assert!(view.automation_view.enabled);
            assert_eq!(view.automation_view.devices.extra.len(), 1);
            assert_eq!(view.worker_requests_pending, 0);
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
        });
        let selector = format!(
            "automation-plot-{}-{}-false",
            device.value(),
            parameter.value()
        );
        assert!(
            visual
                .debug_bounds(Box::leak(selector.into_boxed_str()))
                .is_some()
        );
    }

    #[gpui::test]
    fn lanes_are_independent_and_released_curve_keeps_display_custody_until_exact_terminal(
        cx: &mut TestAppContext,
    ) {
        cx.update(super::super::super::bind_editor_keys);
        let (project, track, device, parameter) = project();
        let (handle, entity) =
            super::super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1000.0), px(700.0)));
        visual.update(|window, cx| {
            entity.update(cx, |view, cx| {
                let entry = Entry::read(
                    &view.state.as_ref().unwrap().project,
                    view.project_generation,
                    Owner::Track(track),
                    device,
                    DeviceAutomationTarget::Parameter(parameter),
                )
                .unwrap();
                view.automation_menu_action(Action::Show(entry.clone(), true), window, cx);
                view.automation_menu_action(Action::Show(entry, true), window, cx);
                assert_eq!(
                    view.automation_view.devices.extra.len(),
                    1,
                    "same parameter does not duplicate its lane"
                );
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
                view.clip_view_open = false;
                cx.notify();
            })
        });
        visual.run_until_parked();
        let selector = format!(
            "automation-plot-{}-{}-false",
            device.value(),
            parameter.value()
        );
        let bounds = visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .unwrap();
        let point = bounds.center();
        visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        entity.update(&mut visual, |view, _| {
            assert!(
                view.automation_view.devices.draft.is_some()
                    && view.automation_view.devices.marquee.is_none()
                    && view.automation_view.devices.pending.is_none(),
                "a plain blank press holds the new point; release commits it"
            )
        });
        visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            assert!(view.automation_view.devices.draft.is_none());
            assert!(view.automation_view.devices.pending.is_some());
            let pending = view.automation_view.devices.pending.as_ref().unwrap();
            let points = pending.points.clone();
            let target = pending.target;
            assert_eq!(
                view.automation_view
                    .devices
                    .points(view.state.as_ref().unwrap(), target),
                points
            );
            // Hide affects layout only, including while the admitted edit is waiting.
            view.automation_view.devices.extra.clear();
            assert_eq!(
                view.automation_view
                    .devices
                    .points(view.state.as_ref().unwrap(), target),
                points
            );
            cx.notify();
        });
        visual.run_until_parked();
        entity.update(&mut visual, |view, _| {
            let pending = view.automation_view.devices.pending.as_ref().unwrap();
            let target = pending.target;
            let points = pending.points.clone();
            let mut model =
                Editor::new(view.state.as_ref().unwrap().project.as_ref().clone(), 16).unwrap();
            model
                .execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter,
                    points: points.clone(),
                })
                .unwrap();
            let mut unrelated =
                super::super::super::arrangement_main_track_ui_test::completed_event(
                    view.state.as_ref().unwrap(),
                );
            unrelated.request_complete = false;
            view.receive(unrelated);
            assert!(
                view.automation_view.devices.pending.is_some(),
                "a state publication is not this request terminal"
            );
            assert_eq!(
                view.automation_view
                    .devices
                    .points(view.state.as_ref().unwrap(), target),
                points
            );
            let mut completed =
                super::super::super::arrangement_main_track_ui_test::completed_event(
                    view.state.as_ref().unwrap(),
                );
            Arc::get_mut(&mut completed.state).unwrap().project = Arc::new(model.project().clone());
            view.receive(completed);
            assert!(view.automation_view.devices.pending.is_none());
            assert_eq!(
                view.automation_view
                    .devices
                    .points(view.state.as_ref().unwrap(), target),
                points,
                "terminal installs the same curve in the canonical state"
            );
        });
        visual.run_until_parked();
    }
}

#[test]
fn receiving_fm4_exposes_granted_assignments_and_retires_unrouted_targets() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "carrier assignment".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Instrument,
                DeviceTypeId::from_static("hex.instrument.fm4"),
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let source = editor.project().track(track).unwrap().source_id().unwrap();
    let before = editor.project().clone();
    let mut targets = Vec::new();
    for id in [2, 5, 8, 11, 29, 30, 31, 32] {
        let entry = Entry::read(
            editor.project(),
            4,
            Owner::Track(track),
            device,
            DeviceAutomationTarget::Parameter(ParameterId::from_static(id)),
        )
        .unwrap();
        assert!(entry.target.current(editor.project(), 4).is_some());
        assert!(entry.target.current(editor.project(), 5).is_none());
        targets.push(entry.target);
    }
    for id in [1, 28, 33, 36] {
        assert!(
            Entry::read(
                editor.project(),
                4,
                Owner::Track(track),
                device,
                DeviceAutomationTarget::Parameter(ParameterId::from_static(id))
            )
            .is_none()
        );
    }
    assert_eq!(editor.project(), &before, "discovery never authors a curve");
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    for target in targets {
        assert!(target.current(editor.project(), 4).is_none());
    }
}
