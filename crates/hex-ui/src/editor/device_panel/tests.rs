use super::*;
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{Command, Created, Editor, Project};
use hex_session::service::{Event, Request, Service, ViewState};
use std::sync::Arc;

fn receive_service_event(service: &Service) -> Event {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => panic!("service response unavailable: {error}"),
        }
    }
}

fn actor_utility_fixture() -> (
    Service,
    Arc<ViewState>,
    TrackId,
    DeviceId,
    hex_lattice_contract::ParameterId,
    bool,
) {
    let entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _initial = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "device track".into(),
        }))
        .expect("add track request");
    let track_event = receive_service_event(&service);
    let Created::Track(track) = track_event.created.expect("created track") else {
        panic!("created track result")
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(entry.type_id),
            before: None,
        })
        .expect("add native device request");
    let device_event = receive_service_event(&service);
    let state = device_event.state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let device_view = hex_engine::native_device_view(
        state.project.track(track).expect("track").devices()[0].state(),
    )
    .expect("device view");
    let bool_parameter = device_view
        .parameters
        .iter()
        .find(|parameter| matches!(parameter.descriptor.kind, ParameterKind::Bool))
        .expect("utility boolean parameter");
    let ParameterValue::Bool(original) = bool_parameter.value else {
        panic!("boolean parameter value")
    };
    (
        service,
        state,
        track,
        device,
        bool_parameter.descriptor.id,
        original,
    )
}

fn actor_editor_window(
    cx: &mut TestAppContext,
    service: Service,
    state: Arc<ViewState>,
    track: TrackId,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let bootstrap = cx.add_window(|_, _| gpui::Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                view.selected_track = Some(track);
                view.device_panel_active = true;
                view.clip_view_open = true;
                view.mode = super::super::EditorMode::Session;
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor)
}

fn pump_until(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    mut done: impl FnMut(&EditorView) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        if editor.read_with(cx, |view, _| done(view)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker-backed device action did not complete"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[gpui::test]
fn settled_device_removal_has_no_stale_warning_or_empty_instrument_block(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("default instrument")
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let panel_before = visual.debug_bounds("device-panel").expect("device panel");
    let remove_effect = Box::leak(format!("device-remove-{}", device.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(remove_effect)
        .expect("effect remove action");
    let click_point = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.devices().is_empty())
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.message, None);
    });
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.has_control_for(device));
        assert!(!view.device_panel.views.contains_key(&device));
    });
    let panel_after_effect = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after_effect.size.height, panel_before.size.height);

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.devices().iter().any(|item| item.id() == device))
        })
    });
    visual.run_until_parked();

    let remove_instrument =
        Box::leak(format!("instrument-remove-{}", instrument.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(remove_instrument)
        .expect("instrument remove action");
    let click_point = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.instrument().is_none())
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.message, None);
    });
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.views.contains_key(&instrument));
    });
    assert!(
        visual.debug_bounds("instrument-empty").is_none(),
        "removing the instrument leaves the chain without a placeholder block"
    );
    let panel_after_instrument = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after_instrument.size.height, px(188.0));
    assert_eq!(panel_after_instrument.size.height, panel_before.size.height);

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                track
                    .instrument()
                    .is_some_and(|item| item.id() == instrument)
            })
        })
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("instrument-empty").is_none());
}

fn utility_fixture() -> (
    hex_project::Project,
    TrackId,
    DeviceId,
    hex_lattice_contract::ParameterId,
) {
    let entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let mut project_editor =
        Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Track(track) = project_editor
        .execute(Command::AddTrack {
            name: "device track".into(),
        })
        .expect("track")
    else {
        panic!("track result")
    };
    project_editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(entry.type_id).expect("default"),
        })
        .expect("device");
    let device = project_editor
        .project()
        .track(track)
        .expect("track")
        .devices()[0]
        .id();
    (
        project_editor.project().clone(),
        track,
        device,
        entry.parameters[0].id,
    )
}

fn enter_typed_value(
    visual: &mut VisualTestContext,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
    text: &str,
) {
    let selector = Box::leak(
        format!(
            "device-{}-parameter-{}-value",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let value = visual.debug_bounds(selector).expect("device numeric field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input(text);
    visual.run_until_parked();
}

#[gpui::test]
fn same_update_typed_value_finishes_before_save_barrier(cx: &mut TestAppContext) {
    let (project, track, device, parameter) = utility_fixture();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        let current = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: current.project.clone(),
            waveforms: current.waveforms.clone(),
            midi_preview: current.midi_preview.clone(),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision,
            unsaved: current.unsaved,
            document_path: Some(std::path::PathBuf::from("/tmp/hex-device-save.hex")),
        }));
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "2.5");
    editor.update(cx, |view, cx| view.save_project(cx));
    editor.read_with(cx, |view, _| {
        let snapshot = view.device_gesture.snapshot().expect("synchronous begin");
        assert_eq!(snapshot.target.device, device);
        assert_eq!(snapshot.target.parameter, parameter);
        assert_eq!(snapshot.value, ParameterValue::Float(2.5));
        assert!(view.device_gesture.has_pending_terminal());
        assert!(matches!(
            view.device_gesture_barriers.front(),
            Some(Request::SaveCurrent)
        ));
        assert!(view.saving_project);
    });
}

#[gpui::test]
fn typed_numeric_owner_finalization_saves_actor_state_and_undo_restores_it(
    cx: &mut TestAppContext,
) {
    let (service, initial_state, track, device, _, _) = actor_utility_fixture();
    let numeric_parameter = hex_engine::native_device_view(
        initial_state.project.track(track).expect("track").devices()[0].state(),
    )
    .expect("device view")
    .parameters
    .into_iter()
    .find(|item| matches!(item.descriptor.kind, ParameterKind::Float { .. }))
    .expect("utility numeric parameter")
    .descriptor
    .id;
    let initial_value = parameter_value(&initial_state, track, device, numeric_parameter);
    let path = std::env::temp_dir().join(format!(
        "hex-device-owner-save-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
    ));
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save initial fixture");
    let saved = receive_service_event(&service);
    assert!(saved.save_complete);
    assert!(saved.error.is_none());
    let (handle, editor) = actor_editor_window(cx, service, saved.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, numeric_parameter, "2.5");
    editor.update(cx, |view, cx| view.save_project(cx));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && !view.saving_project
            && view.state.as_ref().is_some_and(|state| !state.unsaved)
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            parameter_value(
                view.state.as_ref().expect("saved state"),
                track,
                device,
                numeric_parameter,
            ),
            ParameterValue::Float(2.5)
        );
        assert!(view.device_panel.pending_values.is_empty());
    });
    let reload = Service::new(48_000, 8, 8_000_000).expect("reload service");
    let _initial = receive_service_event(&reload);
    reload
        .submit(Request::Open(path.clone()))
        .expect("open saved project");
    let disk_state = receive_service_event(&reload).state;
    assert_eq!(
        parameter_value(&disk_state, track, device, numeric_parameter),
        ParameterValue::Float(2.5),
        "owner-finalized value is persisted before SaveCurrent completes"
    );

    let revision = editor.read_with(cx, |view, _| view.state.as_ref().expect("state").revision);
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.revision > revision)
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            parameter_value(
                view.state.as_ref().expect("undo state"),
                track,
                device,
                numeric_parameter,
            ),
            initial_value,
            "one Undo restores the pre-draft device value"
        );
    });
    let _ = std::fs::remove_file(path);
}

#[gpui::test]
fn hiding_a_returned_pointer_gesture_finishes_the_overlay_without_history(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let device_view = hex_engine::native_device_view(
        state.project.track(track).expect("track").devices()[0].state(),
    )
    .expect("device view");
    let balance = device_view
        .parameters
        .iter()
        .find(|item| item.descriptor.label == "balance")
        .expect("balance parameter");
    let ParameterValue::Float(initial) = balance.value else {
        panic!("balance parameter is numeric")
    };
    let parameter = balance.descriptor.id;
    let initial_revision = state.revision;
    let initial_undo = state.can_undo;
    let initial_project = state.project.clone();
    let initial_device_state = state
        .project
        .device(device)
        .expect("canonical device")
        .state()
        .clone();
    assert_eq!(
        hex_engine::patch_native_device_parameter(
            &initial_device_state,
            parameter,
            ParameterValue::Float(initial),
        )
        .expect("patch to original value"),
        initial_device_state,
        "setting the authored parameter to its existing value is canonical no-op"
    );
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let slider_selector = Box::leak(
        format!(
            "device-{}-parameter-{}-slider",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let slider = visual
        .debug_bounds(slider_selector)
        .expect("rendered balance slider");
    let start = point(slider.center().x, slider.center().y);
    let moved = point(start.x + gpui::px(60.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let snapshot = view
            .device_gesture
            .snapshot()
            .expect("active pointer gesture");
        assert_eq!(snapshot.target.parameter, parameter);
        assert_ne!(snapshot.value, ParameterValue::Float(initial));
    });

    visual.simulate_mouse_move(start, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_gesture
                .snapshot()
                .expect("returned gesture")
                .value,
            ParameterValue::Float(initial)
        );
        assert!(!view.device_gesture.has_pending_terminal());
    });

    editor.update(cx, |view, cx| {
        assert!(finalize_controls(view, cx));
        view.device_panel_active = false;
        cx.notify();
        assert!(view.device_gesture.has_pending_terminal());
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let current = view.state.as_ref().expect("canonical state");
        assert_eq!(current.revision, initial_revision);
        assert_eq!(current.can_undo, initial_undo);
        assert_eq!(&*current.project, &*initial_project);
        assert_eq!(
            parameter_value(current, track, device, parameter),
            ParameterValue::Float(initial)
        );
        assert!(view.device_gesture.snapshot().is_none());
    });
}

#[gpui::test]
fn same_update_typed_value_on_clean_close_opens_prompt_and_cancel_keeps_it(
    cx: &mut TestAppContext,
) {
    let (project, track, device, parameter) = utility_fixture();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "3.0");
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.request_window_close(window, cx));
            assert!(view.close_prompt_open);
            let snapshot = view.device_gesture.snapshot().expect("synchronous begin");
            assert_eq!(snapshot.value, ParameterValue::Float(3.0));
            assert!(view.device_gesture.has_pending_terminal());
        });
    })
    .expect("request close");
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.state.as_ref().is_some_and(|state| !state.unsaved));
        assert!(!view.close_prompt_open);
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("numeric control remains mounted after close cancel")
        };
        entity.read_with(cx, |control, _| {
            assert!(!control.is_editing());
            assert_eq!(control.value(), 3.0);
        });
    });
}

#[gpui::test]
fn full_device_barrier_refusal_keeps_numeric_draft_and_retry_commits_once(cx: &mut TestAppContext) {
    let (project, track, first_device, parameter) = utility_fixture();
    let catalog_entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let mut project_editor = Editor::new(project, 8).expect("editor");
    project_editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(catalog_entry.type_id).expect("default"),
        })
        .expect("second device");
    let second_device = project_editor
        .project()
        .track(track)
        .expect("track")
        .devices()[1]
        .id();
    let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        project_editor.project().clone(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        let current = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: current.project.clone(),
            waveforms: current.waveforms.clone(),
            midi_preview: current.midi_preview.clone(),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision,
            unsaved: current.unsaved,
            document_path: Some(std::path::PathBuf::from("/tmp/device-retry.hex")),
        }));
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        view.device_gesture
            .begin(
                view.project_generation,
                GestureTarget {
                    device: first_device,
                    parameter,
                },
                ParameterValue::Float(0.0),
            )
            .expect("hold another control's edit");
        for _ in 0..super::super::UI_REQUEST_CAPACITY {
            view.device_gesture_barriers.push_back(Request::Undo);
        }
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, second_device, parameter, "2.5");

    editor.update(cx, |view, cx| view.save_project(cx));
    editor.read_with(cx, |view, _| {
        assert!(!view.saving_project, "full barrier queue refuses the save");
        assert_eq!(
            view.device_gesture
                .snapshot()
                .expect("other edit")
                .target
                .device,
            first_device
        );
        assert!(!view.device_gesture.has_pending_terminal());
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: second_device,
                parameter,
            })
        else {
            panic!("second numeric control remains mounted")
        };
        entity.read_with(cx, |control, _| {
            assert!(control.is_editing(), "refused draft remains editable")
        });
    });

    editor.update(cx, |view, cx| {
        view.device_gesture_barriers.clear();
        view.device_gesture.reset();
        view.save_project(cx);
    });
    editor.read_with(cx, |view, _| {
        let snapshot = view.device_gesture.snapshot().expect("retried gesture");
        assert_eq!(snapshot.target.device, second_device);
        assert_eq!(snapshot.target.parameter, parameter);
        assert_eq!(snapshot.value, ParameterValue::Float(2.5));
        assert!(view.device_gesture.has_pending_terminal());
        assert_eq!(
            view.device_panel.pending_values.get(&ParameterKey {
                device: second_device,
                parameter,
            }),
            Some(&ParameterValue::Float(2.5)),
        );
        assert!(matches!(
            view.device_gesture_barriers.front(),
            Some(Request::SaveCurrent)
        ));
        assert!(view.saving_project);
    });
}

#[gpui::test]
fn invalid_numeric_draft_is_cancelled_on_project_replacement_and_target_removal(
    cx: &mut TestAppContext,
) {
    let entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let mut project_editor =
        Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Track(track) = project_editor
        .execute(Command::AddTrack {
            name: "device track".into(),
        })
        .expect("track")
    else {
        panic!("track result")
    };
    project_editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(entry.type_id).expect("default"),
        })
        .expect("device");
    let initial_project = project_editor.project().clone();
    let device = initial_project.track(track).expect("track").devices()[0].id();
    let parameter = entry.parameters[0].id;

    let bootstrap = cx.add_window(|_, _| gpui::Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 8, 8_000_000).expect("service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(Arc::new(ViewState {
                    project: Arc::new(initial_project.clone()),
                    waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                view.selected_track = Some(track);
                view.device_panel_active = true;
                view.clip_view_open = true;
                view.mode = super::super::EditorMode::Session;
                cx.notify();
            });
            editor
        })
        .expect("window");

    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(
        format!(
            "device-{}-parameter-{}-value",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let value = visual.debug_bounds(selector).expect("numeric field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.run_until_parked();

    let old_control = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("numeric field entity should be cached")
        };
        entity.clone()
    });
    old_control.read_with(cx, |control, _| assert!(control.is_editing()));
    editor.update(cx, |view, cx| view.save_project(cx));
    editor.read_with(cx, |view, _| {
        assert!(!view.saving_project);
        assert!(
            !view.save_dialog_open,
            "invalid draft blocks save admission"
        );
    });
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.request_window_close(window, cx))
        });
    })
    .expect("close request");
    assert!(
        cx.has_pending_prompt(),
        "invalid draft still permits explicit discard"
    );
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    old_control.read_with(cx, |control, _| {
        assert!(
            control.is_editing(),
            "cancel keeps the invalid draft visible"
        );
    });
    editor.update(cx, |view, cx| {
        view.project_generation = view.project_generation.saturating_add(1);
        cx.notify();
    });
    visual.run_until_parked();
    old_control.read_with(cx, |control, _| {
        assert!(!control.is_editing(), "replacement cancels the old draft");
    });

    let value = visual
        .debug_bounds(selector)
        .expect("rebuilt numeric field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.run_until_parked();
    let stale_control = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("rebuilt numeric field entity should be cached")
        };
        entity.clone()
    });

    let mut without_device = Editor::new(initial_project, 8).expect("replacement editor");
    without_device
        .execute(Command::RemoveDevice(device))
        .expect("remove target");
    let next_project = without_device.project().clone();
    editor.update(cx, |view, cx| {
        let current = view.state.as_ref().expect("view state");
        view.state = Some(Arc::new(ViewState {
            project: Arc::new(next_project.clone()),
            waveforms: current.waveforms.clone(),
            midi_preview: current.midi_preview.clone(),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision,
            unsaved: current.unsaved,
            document_path: current.document_path.clone(),
        }));
        cx.notify();
    });
    visual.run_until_parked();
    stale_control.read_with(cx, |control, _| {
        assert!(!control.is_editing(), "removed target cancels its draft");
    });
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.controls.is_empty());
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("device target is no longer available")
        );
    });
}

fn click_toggle(
    visual: &mut gpui::VisualTestContext,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
) {
    let selector = Box::leak(
        format!("device-{}-parameter-{}", device.value(), parameter.value()).into_boxed_str(),
    );
    let bounds = visual.debug_bounds(selector).expect("boolean control");
    let point = gpui::point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
}

fn bool_value(
    state: &ViewState,
    track: TrackId,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
) -> bool {
    let device = state
        .project
        .track(track)
        .and_then(|track| track.devices().iter().find(|item| item.id() == device))
        .expect("canonical device");
    let view = hex_engine::native_device_view(device.state()).expect("device view");
    match view
        .parameters
        .iter()
        .find(|item| item.descriptor.id == parameter)
        .expect("boolean parameter")
        .value
    {
        ParameterValue::Bool(value) => value,
        _ => panic!("boolean value expected"),
    }
}

fn parameter_value(
    state: &ViewState,
    track: TrackId,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
) -> ParameterValue {
    let device = state
        .project
        .track(track)
        .and_then(|track| track.devices().iter().find(|item| item.id() == device))
        .expect("canonical device");
    hex_engine::native_device_view(device.state())
        .expect("device view")
        .parameters
        .into_iter()
        .find(|item| item.descriptor.id == parameter)
        .expect("parameter")
        .value
}

#[gpui::test]
fn discrete_device_edit_acknowledges_then_undo_restores_canonical_value(cx: &mut TestAppContext) {
    let (service, state, track, device, parameter, original) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click_toggle(&mut visual, device, parameter);
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            bool_value(
                view.state.as_ref().expect("state"),
                track,
                device,
                parameter
            ),
            !original
        );
    });
    visual.run_until_parked();

    let prior_revision =
        editor.read_with(cx, |view, _| view.state.as_ref().expect("state").revision);
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().expect("state").revision > prior_revision
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.pending_values.is_empty())
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        assert_eq!(bool_value(state, track, device, parameter), original);
        assert!(view.device_panel.pending_values.is_empty());
        let Some(ParameterControl::Bool { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("boolean control remains mounted")
        };
        entity.read_with(cx, |toggle, _| assert_eq!(toggle.checked(), original));
    });
}

#[gpui::test]
fn rejected_discrete_device_edit_retires_optimistic_value(cx: &mut TestAppContext) {
    let (service, state, track, device, parameter, original) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        // Occupy the UI FIFO so the click stays queued and can be refused
        // by the real actor after this test substitutes an invalid type.
        view.pending_requests
            .push_back(Request::SetDeviceParameter {
                device,
                parameter,
                value: ParameterValue::Float(1.0),
            });
        cx.notify();
    });
    click_toggle(&mut visual, device, parameter);
    editor.update(cx, |view, cx| {
        let request = view
            .pending_requests
            .back_mut()
            .expect("queued toggle request");
        *request = Request::SetDeviceParameter {
            device,
            parameter,
            value: ParameterValue::Float(1.0),
        };
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.pending_values.is_empty())
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            bool_value(
                view.state.as_ref().expect("state"),
                track,
                device,
                parameter
            ),
            original
        );
        assert!(view.error.is_some(), "actor refusal remains visible");
        let Some(ParameterControl::Bool { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("boolean control remains mounted")
        };
        entity.read_with(cx, |toggle, _| assert_eq!(toggle.checked(), original));
    });
}

#[gpui::test]
fn matching_first_ack_does_not_retire_latest_value_while_same_parameter_edits_are_queued(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, parameter, original) = actor_utility_fixture();
    let first_ack_value = !original;
    let original_device_state = state
        .project
        .device(device)
        .expect("device")
        .state()
        .clone();
    let acknowledged_state = hex_engine::patch_native_device_parameter(
        &original_device_state,
        parameter,
        ParameterValue::Bool(first_ack_value),
    )
    .expect("apply first acknowledged value");
    let mut project_editor = Editor::new((*state.project).clone(), 8).expect("project editor");
    project_editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: acknowledged_state,
        })
        .expect("first acknowledged edit");
    let changed_state = Arc::new(ViewState {
        project: Arc::new(project_editor.project().clone()),
        waveforms: state.waveforms.clone(),
        midi_preview: state.midi_preview.clone(),
        timeline_frames: state.timeline_frames,
        can_undo: project_editor.can_undo(),
        can_redo: project_editor.can_redo(),
        revision: state.revision + 1,
        unsaved: true,
        document_path: None,
    });
    let (handle, editor) = actor_editor_window(cx, service, changed_state, track);
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        let key = ParameterKey { device, parameter };
        view.device_panel
            .pending_values
            .insert(key, ParameterValue::Bool(first_ack_value));
        // The first `true` has been acknowledged; `false` then `true` are still
        // queued, so the newest optimistic value must survive that matching ack.
        view.pending_requests
            .push_back(Request::SetDeviceParameter {
                device,
                parameter,
                value: ParameterValue::Bool(original),
            });
        view.pending_requests
            .push_back(Request::SetDeviceParameter {
                device,
                parameter,
                value: ParameterValue::Bool(first_ack_value),
            });
        let _panel = super::view(view, cx);
        assert_eq!(
            view.device_panel.pending_values.get(&key),
            Some(&ParameterValue::Bool(first_ack_value))
        );
        assert_eq!(view.pending_requests.len(), 2);
        let Some(ParameterControl::Bool { entity, .. }) = view.device_panel.controls.get(&key)
        else {
            panic!("toggle control should render")
        };
        entity.read_with(cx, |toggle, _| {
            assert_eq!(toggle.checked(), first_ack_value)
        });
    });
    let _ = handle;
}
