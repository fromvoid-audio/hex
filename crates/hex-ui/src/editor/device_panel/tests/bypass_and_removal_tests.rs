use super::*;

#[gpui::test]
fn utility_mode_popup_finalizes_numeric_draft_and_closes_on_escape_or_owner_removal(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let gain_value =
        Box::leak(format!("device-{}-parameter-1-value", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(gain_value).expect("gain readout");
    let click = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("6.00 db");
    visual.run_until_parked();

    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width dial");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_some());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                state.project.track(track).unwrap().devices()[0].state(),
                hex_lattice_contract::ParameterId::from_static(1),
            )
            .is_ok_and(|value| {
                matches!(value, ParameterValue::Float(value) if (value - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.0001)
            })
        })
    });

    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width dial after escape");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));

    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width dial after escape");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_down(
        point(px(5.0), px(100.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(px(5.0), px(100.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));

    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width dial after outside dismissal");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_some());

    editor.update(cx, |view, cx| {
        view.enqueue_device_barrier(Request::Edit(Command::RemoveDevice(device)));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.devices().is_empty())
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
}

#[gpui::test]
fn return_track_reuses_compact_utility_panel(cx: &mut TestAppContext) {
    let (service, state, track, return_track, device, _) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        view.session_grid.focused_return = Some(return_track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let panel = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel.size.height, px(229.0));
    let frame = visual
        .debug_bounds(Box::leak(
            format!("effect-device-{}", device.value()).into_boxed_str(),
        ))
        .expect("return Utility frame");
    assert_eq!(
        frame.size.width,
        px(144.0 + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH)
    );
    assert_eq!(frame.size.height, px(200.0));
    let body = visual
        .debug_bounds(Box::leak(
            format!("device-body-{}", device.value()).into_boxed_str(),
        ))
        .expect("return Utility body");
    assert_eq!(body.size.height, px(198.0));
    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .expect("return Utility title");
    for suffix in ["device-bypass", "return-device-remove"] {
        let action = visual
            .debug_bounds(Box::leak(
                format!("{suffix}-{}", device.value()).into_boxed_str(),
            ))
            .expect("return Utility header action");
        assert_eq!(action.size.height, px(12.0));
        assert!(action.origin.x >= frame.origin.x);
        assert!(action.origin.x + action.size.width <= frame.origin.x + frame.size.width);
        assert!(action.bottom() <= title.top());
    }
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("utility-input-{}", device.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("utility-output-{}", device.value()).into_boxed_str()
            ))
            .is_some()
    );
}

#[gpui::test]
fn existing_device_header_title_reorders_cards_by_stable_identity(cx: &mut TestAppContext) {
    let (service, _state, track, first, _, _) = actor_utility_fixture();
    let utility = hex_engine::native_device_catalog()
        .expect("registered utility")
        .remove(0);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(utility.type_id),
            before: None,
        })
        .expect("second utility");
    let second_state = receive_service_event(&service).state;
    let second = second_state
        .project
        .track(track)
        .expect("track")
        .devices()
        .iter()
        .find(|device| device.id() != first)
        .expect("second utility")
        .id();
    let (handle, editor) = actor_editor_window(cx, service, second_state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let title_selector = Box::leak(format!("device-drag-title-{}", first.value()).into_boxed_str());
    let target_selector =
        Box::leak(format!("device-drop-card-{}", second.value()).into_boxed_str());
    let title = visual
        .debug_bounds(title_selector)
        .expect("first device title");
    let target = visual
        .debug_bounds(target_selector)
        .expect("second device receiver");
    let down = point(
        title.origin.x + title.size.width / 2.0,
        title.origin.y + title.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let destination = point(
        target.origin.x + target.size.width - px(8.0),
        target.origin.y + target.size.height / 2.0,
    );
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .device_panel
            .active_native_drag
            .as_ref()
            .expect("existing device drag");
        assert_eq!(
            drag.destination.get(),
            Some(drag::NativeDeviceDropTarget {
                track,
                before: None,
                replace: None,
            })
        );
    });
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                track
                    .devices()
                    .iter()
                    .map(|device| device.id())
                    .collect::<Vec<_>>()
                    == vec![second, first]
            })
        })
    });
}

#[gpui::test]
fn device_drag_clears_preview_on_outside_release_and_escape(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.active_native_drag.is_none());
        assert!(view.test_probes.last_enqueued_native_device_drop.is_none());
    });

    let source = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .expect("registered utility row");
    let card_selector = Box::leak(format!("device-drop-card-{}", device.value()).into_boxed_str());
    let card = visual.debug_bounds(card_selector).expect("device card");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let card_point = point(
        card.origin.x + card.size.width / 2.0,
        card.origin.y + card.size.height / 2.0,
    );
    visual.simulate_mouse_move(card_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .active_native_drag
                .as_ref()
                .and_then(|drag| drag.destination.get())
                .is_some()
        );
    });

    let outside_rail = point(px(500.0), px(360.0));
    visual.simulate_mouse_move(outside_rail, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .active_native_drag
                .as_ref()
                .is_some_and(|drag| drag.destination.get().is_none())
        );
    });
    visual.simulate_mouse_up(outside_rail, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.active_native_drag.is_none());
        assert!(view.test_probes.last_enqueued_native_device_drop.is_none());
    });

    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(card_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.active_native_drag.is_some())
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.active_native_drag.is_none());
        assert!(view.test_probes.last_enqueued_native_device_drop.is_none());
    });
}

#[gpui::test]
fn same_update_typed_value_finishes_before_save_barrier(cx: &mut TestAppContext) {
    let (project, track, device, parameter) = utility_fixture();
    let (handle, editor) =
        super::super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        let current = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: current.project.clone(),
            waveforms: current.waveforms.clone(),
            source_overviews: current.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: current.midi_preview.clone(),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision,
            unsaved: current.unsaved,
            document_path: Some(std::path::PathBuf::from("/tmp/hex-device-save.hex")),
        }));
        view.selected_track = Some(track);
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "250%");
    editor.update(cx, |view, cx| view.save_project(cx));
    editor.read_with(cx, |view, _| {
        let snapshot = view
            .device_panel
            .gesture
            .snapshot()
            .expect("synchronous begin");
        assert_eq!(
            snapshot.target,
            GestureTarget::DeviceParameter { device, parameter }
        );
        assert_eq!(snapshot.value, ParameterValue::Float(2.5));
        assert!(view.device_panel.gesture.has_pending_terminal());
        assert!(matches!(
            view.device_panel.gesture_barriers.front(),
            Some(Request::SaveCurrent)
        ));
        assert!(view.persistence.saving_project);
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
    .find(|item| item.descriptor.label == "width")
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
    enter_typed_value(&mut visual, device, numeric_parameter, "250%");
    editor.update(cx, |view, cx| view.save_project(cx));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture_barriers.is_empty()
            && !view.persistence.saving_project
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

    let dial_selector = Box::leak(
        format!(
            "device-{}-parameter-{}-knob",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let dial = visual
        .debug_bounds(dial_selector)
        .expect("rendered balance dial");
    let start = point(dial.center().x, dial.center().y);
    let moved = point(start.x, start.y - gpui::px(60.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let snapshot = view
            .device_panel
            .gesture
            .snapshot()
            .expect("active pointer gesture");
        assert_eq!(
            snapshot.target,
            GestureTarget::DeviceParameter { device, parameter }
        );
        assert_ne!(snapshot.value, ParameterValue::Float(initial));
    });

    visual.simulate_mouse_move(start, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel
                .gesture
                .snapshot()
                .expect("returned gesture")
                .value,
            ParameterValue::Float(initial)
        );
        assert!(!view.device_panel.gesture.has_pending_terminal());
    });

    editor.update(cx, |view, cx| {
        assert!(finalize_controls(view, cx));
        view.device_panel.active = false;
        cx.notify();
        assert!(view.device_panel.gesture.has_pending_terminal());
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture_barriers.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
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
        assert!(view.device_panel.gesture.snapshot().is_none());
    });
}

#[gpui::test]
fn same_update_typed_value_on_clean_close_opens_prompt_and_cancel_keeps_it(
    cx: &mut TestAppContext,
) {
    let environment = super::super::super::test_environment::Environment::new().unwrap();
    let (service, _, track, device) =
        actor_native_effect_fixture(hex_devices::utility::UTILITY_TYPE_ID);
    service
        .submit(Request::SaveNew(environment.root().join("clean.hex")))
        .unwrap();
    let saved = receive_service_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert!(!saved.state.unsaved);
    let parameter = hex_engine::native_device_view(
        saved.state.project.track(track).unwrap().devices()[0].state(),
    )
    .unwrap()
    .parameters
    .iter()
    .find(|p| p.descriptor.label == "width")
    .unwrap()
    .descriptor
    .id;
    let (handle, editor) = actor_editor_window(cx, service, saved.state, track);
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "300%");
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.request_window_close(window, cx));
            assert!(view.persistence.automation_close_preflight.frozen());
            let snapshot = view
                .device_panel
                .gesture
                .snapshot()
                .expect("synchronous begin");
            assert_eq!(snapshot.value, ParameterValue::Float(3.0));
            assert!(view.device_panel.gesture.has_pending_terminal());
        });
    })
    .expect("request close");
    super::super::super::close::wait_for_close_prompt(&editor, cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.state.as_ref().is_some_and(|state| state.unsaved));
        assert_eq!(
            parameter_value(view.state.as_ref().unwrap(), track, device, parameter),
            ParameterValue::Float(3.0)
        );
        assert!(!view.persistence.close_prompt_open);
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
    let (handle, editor) = super::super::super::clip_drag_ui_test::editor_window(
        cx,
        project_editor.project().clone(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        let current = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: current.project.clone(),
            waveforms: current.waveforms.clone(),
            source_overviews: current.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: current.midi_preview.clone(),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision,
            unsaved: current.unsaved,
            document_path: Some(std::path::PathBuf::from("/tmp/device-retry.hex")),
        }));
        view.selected_track = Some(track);
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        view.device_panel
            .gesture
            .begin(
                view.project_generation,
                GestureTarget::DeviceParameter {
                    device: first_device,
                    parameter,
                },
                ParameterValue::Float(0.0),
            )
            .expect("hold another control's edit");
        for _ in 0..super::super::super::UI_REQUEST_CAPACITY {
            view.device_panel.gesture_barriers.push_back(Request::Undo);
        }
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, second_device, parameter, "250%");

    let rail = visual.debug_bounds("device-chain-scroll").expect("rail");
    visual.simulate_mouse_down(rail.center(), MouseButton::Middle, Modifiers::default());
    let panned = point(rail.center().x - px(50.0), rail.center().y);
    visual.simulate_mouse_move(panned, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(panned, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: second_device,
                parameter,
            })
        else {
            panic!("draft control");
        };
        assert!(
            entity.read(cx).is_editing(),
            "middle pan preserves typed draft"
        );
    });

    let toggle = visual
        .debug_bounds("device-panel-bottom-toggle")
        .expect("bottom toggle")
        .center();
    visual.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.audio_detail.clip_view_open,
            "full queue preserves pane and draft"
        )
    });

    editor.update(cx, |view, cx| view.save_project(cx));
    editor.read_with(cx, |view, _| {
        assert!(
            !view.persistence.saving_project,
            "full barrier queue refuses the save"
        );
        assert_eq!(
            view.device_panel
                .gesture
                .snapshot()
                .expect("other edit")
                .target,
            GestureTarget::DeviceParameter {
                device: first_device,
                parameter,
            }
        );
        assert!(!view.device_panel.gesture.has_pending_terminal());
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
        view.device_panel.gesture_barriers.clear();
        view.device_panel.gesture.reset();
        view.save_project(cx);
    });
    editor.read_with(cx, |view, _| {
        let snapshot = view
            .device_panel
            .gesture
            .snapshot()
            .expect("retried gesture");
        assert_eq!(
            snapshot.target,
            GestureTarget::DeviceParameter {
                device: second_device,
                parameter,
            }
        );
        assert_eq!(snapshot.value, ParameterValue::Float(2.5));
        assert!(view.device_panel.gesture.has_pending_terminal());
        assert_eq!(
            view.device_panel.pending_values.get(&ParameterKey {
                device: second_device,
                parameter,
            }),
            Some(&ParameterValue::Float(2.5)),
        );
        assert!(matches!(
            view.device_panel.gesture_barriers.front(),
            Some(Request::SaveCurrent)
        ));
        assert!(view.persistence.saving_project);
    });
}

#[gpui::test]
fn command_f_preserves_invalid_device_draft_then_finalizes_repaired_value(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let parameter =
        hex_engine::native_device_view(state.project.track(track).unwrap().devices()[0].state())
            .unwrap()
            .parameters
            .into_iter()
            .find(|item| item.descriptor.label == "width")
            .unwrap()
            .descriptor
            .id;
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.open = false;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "invalid");
    let focus = visual.update(|window, cx| window.focused(cx).unwrap());
    for selector in ["clip-detail-tab", "device-detail-tab", "clip-view-toggle"] {
        let target = visual
            .debug_bounds(selector)
            .expect("footer control")
            .center();
        visual.simulate_click(target, Modifiers::default());
        visual.update(|window, cx| {
            assert!(
                focus.is_focused(window),
                "{selector} must retain invalid draft focus"
            );
            editor.read_with(cx, |view, _| {
                assert!(view.audio_detail.clip_view_open && view.device_panel.active);
                assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
            });
        });
    }
    visual.simulate_keystrokes("cmd-f");
    visual.run_until_parked();
    visual.update(|window, cx| {
        assert!(
            focus.is_focused(window),
            "invalid device draft must retain focus"
        );
        editor.read_with(cx, |view, cx| {
            assert!(!view.browser.open);
            assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("finish the device value edit")
            );
            let ParameterControl::Float { entity, .. } =
                &view.device_panel.controls[&ParameterKey { device, parameter }]
            else {
                panic!("numeric control")
            };
            assert!(entity.read(cx).is_editing());
        });
    });
    visual.simulate_keystrokes("escape");
    enter_typed_value(&mut visual, device, parameter, "250%");
    visual.simulate_keystrokes("cmd-f");
    visual.run_until_parked();
    visual.simulate_input("utility");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "utility");
        let ParameterControl::Float { entity, .. } =
            &view.device_panel.controls[&ParameterKey { device, parameter }]
        else {
            panic!("numeric control")
        };
        assert!(!entity.read(cx).is_editing());
    });
    pump_until(&editor, cx, |view| {
        parameter_value(view.state.as_ref().unwrap(), track, device, parameter)
            == ParameterValue::Float(2.5)
    });
}

#[gpui::test]
fn command_f_preserves_invalid_return_send_draft(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, return_track, _, _) = actor_return_utility_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::super::EditorMode::Arrangement;
        view.session_grid.send_menu_track = Some(track);
        view.browser.open = false;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_down(value.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(value.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("7 db");
    let focus = visual.update(|window, cx| window.focused(cx).unwrap());
    visual.simulate_keystrokes("cmd-f");
    visual.run_until_parked();
    visual.update(|window, cx| {
        assert!(
            focus.is_focused(window),
            "invalid return send must retain focus"
        );
        editor.read_with(cx, |view, cx| {
            assert!(!view.browser.open);
            assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("finish the device value edit")
            );
            assert!(
                view.device_panel.send_controls[&SendControlKey {
                    track,
                    return_track
                }]
                    .entity
                    .read(cx)
                    .is_editing()
            );
        });
    });
    visual.simulate_keystrokes("escape cmd-f");
    visual.simulate_input("utility");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "utility")
    });
}

#[gpui::test]
fn invalid_numeric_draft_is_cancelled_on_project_replacement_and_target_removal(
    cx: &mut TestAppContext,
) {
    let entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let (service, state, track, device) = actor_native_effect_fixture(entry.type_id);
    let initial_project = (*state.project).clone();
    let parameter = entry.parameters[0].id;
    let (handle, editor) = actor_editor_window(cx, service, state, track);

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
        assert!(!view.persistence.saving_project);
        assert!(
            !view.persistence.save_dialog_open,
            "invalid draft blocks save admission"
        );
    });
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.request_window_close(window, cx))
        });
    })
    .expect("close request");
    super::super::super::close::wait_for_close_prompt(&editor, cx);
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
        view.device_panel.operator_pages.insert(
            device,
            super::super::operator_panel::OperatorPanelState {
                selected_operator: 2,
                page: super::super::operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        view.project_generation = view.project_generation.saturating_add(1);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            !view.device_panel.operator_pages.contains_key(&device),
            "project replacement drops device-specific page state"
        );
    });
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
        view.device_panel.operator_pages.insert(
            device,
            super::super::operator_panel::OperatorPanelState {
                selected_operator: 3,
                page: super::super::operator_panel::OperatorPage::Envelope,
                ..Default::default()
            },
        );
        let current = view.state.as_ref().expect("view state");
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(next_project.clone()),
            waveforms: current.waveforms.clone(),
            source_overviews: current.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: current.midi_preview.clone(),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
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
        assert!(
            view.device_panel
                .controls
                .keys()
                .all(|key| key.device != device)
        );
        assert!(
            !view.device_panel.operator_pages.contains_key(&device),
            "removed device drops its page state"
        );
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("device target is no longer available")
        );
    });
}

#[gpui::test]
fn invalid_return_numeric_draft_is_cancelled_when_its_owner_is_removed(cx: &mut TestAppContext) {
    let (service, state, track, return_track, device, parameter) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = Some(return_track);
        cx.notify();
    });
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
    let value = visual
        .debug_bounds(selector)
        .expect("return Utility gain field");
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
            panic!("return numeric field entity should be cached")
        };
        entity.clone()
    });
    old_control.read_with(cx, |control, _| assert!(control.is_editing()));

    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Edit(Command::RemoveReturnTrack(return_track))));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.return_tracks().is_empty())
    });
    visual.run_until_parked();
    old_control.read_with(cx, |control, _| {
        assert!(
            !control.is_editing(),
            "removed return owner cancels the draft"
        )
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .controls
                .keys()
                .all(|key| key.device != device)
        );
        assert!(view.state.as_ref().unwrap().project.track(track).is_some());
        assert_eq!(view.device_panel.message, None);
    });
}

#[gpui::test]
fn return_send_numeric_entry_uses_one_undoable_canonical_edit(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, return_track, _, _) = actor_return_utility_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::super::EditorMode::Arrangement;
        view.session_grid.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return send value field");
    let click = point(value.center().x, value.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-8.519374645 db");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control.entity.read_with(cx, |control, _| {
            assert!(
                control.is_editing(),
                "send value field entered text editing"
            )
        });
    });
    editor.update(cx, |view, cx| {
        assert!(super::super::finalize_controls(view, cx))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        assert_eq!(
            state.revision,
            initial_revision + 1,
            "one send edit committed"
        );
        let send = state
            .project
            .track(track)
            .expect("source track")
            .sends()
            .iter()
            .find(|send| send.return_track() == return_track)
            .expect("canonical send");
        assert!(
            (send.gain() - 0.375).abs() < 0.000_01,
            "status: {:?}",
            view.status
        );
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().is_some_and(|state| {
                state.project.track(track).is_some_and(|track| {
                    track.sends().iter().any(|send| {
                        send.return_track() == return_track && (send.gain() - 0.25).abs() < 0.000_01
                    })
                })
            })
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Redo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().is_some_and(|state| {
                state.project.track(track).is_some_and(|track| {
                    track.sends().iter().any(|send| {
                        send.return_track() == return_track
                            && (send.gain() - 0.375).abs() < 0.000_01
                    })
                })
            })
    });
}

#[gpui::test]
fn return_send_popup_fits_minimum_window_and_retires_offscreen_or_removed_owner(
    cx: &mut TestAppContext,
) {
    let (service, mut state, track, return_track, _, _) = actor_return_utility_fixture();
    for index in 0..24 {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: format!("extra {index}"),
            }))
            .expect("add scroll fixture track");
        state = receive_service_event(&service).state;
    }
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::super::EditorMode::Arrangement;
        view.session_grid.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let menu_selector = Box::leak(format!("return-send-menu-{}", track.value()).into_boxed_str());
    let value_selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let menu = visual
        .debug_bounds(menu_selector)
        .expect("send popup at minimum size");
    let value = visual
        .debug_bounds(value_selector)
        .expect("send numeric field");
    let window = visual.update(|window, _| window.bounds());
    assert!(menu.origin.x >= window.origin.x && menu.origin.y >= window.origin.y);
    assert!(menu.origin.x + menu.size.width <= window.origin.x + window.size.width);
    assert!(menu.origin.y + menu.size.height <= window.origin.y + window.size.height);
    assert!(
        value.size.width >= px(46.0),
        "numeric value retains its minimum hit width"
    );
    assert!(value.left() >= menu.left() && value.right() <= menu.right());
    let bar_selector = Box::leak(
        format!(
            "return-send-{}-{}-slider",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    assert_eq!(
        value,
        visual.debug_bounds(bar_selector).unwrap(),
        "value spans the filled rectangle"
    );

    editor.update(cx, |view, cx| {
        view.track_scroll
            .scroll_to_item(24, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.update(|window, app| {
        editor.update(app, |view, app| {
            let _ = view.return_send_menu_overlay(window, app);
        });
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_ne!(
            view.track_scroll.offset().y,
            px(0.0),
            "the source row must actually be outside the current viewport"
        );
        assert_eq!(
            view.session_grid.send_menu_track, None,
            "popup closes when its source row scrolls out of view"
        );
    });
    assert!(
        visual.update(|window, app| editor.update(app, |view, _| view.focus.is_focused(window)))
    );

    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Edit(Command::RemoveTrack(track))));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.track(track).is_none())
    });
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        let _ = super::super::view(view, None, cx);
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.session_grid.send_menu_track, None,
            "removed source closes its send popup"
        );
        assert!(
            !view
                .device_panel
                .send_controls
                .contains_key(&SendControlKey {
                    track,
                    return_track,
                }),
            "removed source retires its cached send widget"
        );
    });
}

#[gpui::test]
fn valid_return_send_draft_commits_once_when_owner_scrolls_offscreen(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, mut state, track, return_track, _, _) = actor_return_utility_fixture();
    for index in 0..24 {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: format!("extra {index}"),
            }))
            .expect("add scroll fixture track");
        state = receive_service_event(&service).state;
    }
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::super::EditorMode::Arrangement;
        view.session_grid.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return send value field");
    let click = point(value.center().x, value.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-4.082399653 db");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(control.is_editing()));
    });

    editor.update(cx, |view, cx| {
        view.track_scroll
            .scroll_to_item(24, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.update(|window, app| {
        editor.update(app, |view, app| {
            let _ = view.return_send_menu_overlay(window, app);
        });
    });
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.session_grid.send_menu_track.is_none()
            && view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|state| {
                state.revision == initial_revision + 1
                    && state.project.track(track).is_some_and(|track| {
                        track.sends().iter().any(|send| {
                            send.return_track() == return_track
                                && (send.gain() - 0.625).abs() < 0.000_01
                        })
                    })
            })
    });
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(!control.is_editing()));
    });
    assert!(
        visual.update(|window, app| editor.update(app, |view, _| view.focus.is_focused(window)))
    );
}

#[gpui::test]
fn invalid_return_send_draft_cancels_and_restores_when_owner_scrolls_offscreen(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, mut state, track, return_track, _, _) = actor_return_utility_fixture();
    for index in 0..24 {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: format!("extra {index}"),
            }))
            .expect("add scroll fixture track");
        state = receive_service_event(&service).state;
    }
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::super::EditorMode::Arrangement;
        view.session_grid.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return send value field");
    let click = point(value.center().x, value.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("7 db");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(control.is_editing()));
    });

    editor.update(cx, |view, cx| {
        view.track_scroll
            .scroll_to_item(24, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.update(|window, app| {
        editor.update(app, |view, app| {
            let _ = view.return_send_menu_overlay(window, app);
        });
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.session_grid.send_menu_track, None);
        assert!(view.device_panel.gesture.snapshot().is_none());
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
        assert!(view.error.as_deref().is_some_and(|error| {
            error.contains("invalid send value") && error == error.to_lowercase()
        }));
        assert_eq!(
            view.state.as_ref().expect("state").revision,
            initial_revision
        );
        let send = view
            .state
            .as_ref()
            .expect("state")
            .project
            .track(track)
            .expect("source track")
            .sends()
            .iter()
            .find(|send| send.return_track() == return_track)
            .expect("canonical send");
        assert!((send.gain() - 0.25).abs() < 0.000_01);
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(!control.is_editing()));
    });
    assert!(
        visual.update(|window, app| editor.update(app, |view, _| view.focus.is_focused(window)))
    );
}

#[gpui::test]
fn invalid_return_send_draft_blocks_finalize_and_escape_restores_canonical(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, return_track, _, _) = actor_return_utility_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::super::EditorMode::Arrangement;
        view.session_grid.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return send value field");
    let click = point(value.center().x, value.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("7 db");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(control.is_editing()));
    });
    editor.update(cx, |view, cx| {
        assert!(!super::super::finalize_controls(view, cx));
        cx.notify();
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().expect("state").revision,
            initial_revision
        );
        assert!(view.error.is_some());
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        assert!(super::super::finalize_controls(view, cx));
        cx.notify();
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        assert_eq!(state.revision, initial_revision);
        let send = state
            .project
            .track(track)
            .expect("source track")
            .sends()
            .iter()
            .find(|send| send.return_track() == return_track)
            .expect("canonical send remains");
        assert!((send.gain() - 0.25).abs() < 0.000_01);
    });
}

#[gpui::test]
fn return_send_pointer_drag_previews_then_commits_once(cx: &mut TestAppContext) {
    let (service, state, track, return_track, _, _) = actor_return_utility_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::super::EditorMode::Arrangement;
        view.session_grid.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "return-send-{}-{}-slider",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let slider = visual.debug_bounds(selector).expect("return send slider");
    let start = point(slider.center().x, slider.center().y);
    let moved = point(start.x, start.y - px(36.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control.entity.read_with(cx, |control, _| {
            assert!(
                control.is_interacting(),
                "send slider is in a pointer gesture"
            )
        });
        let snapshot = view
            .device_panel
            .gesture
            .snapshot()
            .expect("active send gesture");
        assert_eq!(
            snapshot.target,
            GestureTarget::ReturnSend {
                track,
                return_track
            }
        );
        assert!(matches!(snapshot.value, ParameterValue::Float(value) if value > 0.25));
        let canonical = view
            .state
            .as_ref()
            .expect("state")
            .project
            .track(track)
            .unwrap();
        assert!(canonical.sends().iter().any(|send| {
            send.return_track() == return_track && (send.gain() - 0.25).abs() < 0.000_01
        }));
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|state| {
                state.revision == initial_revision + 1
                    && state.project.track(track).is_some_and(|track| {
                        track
                            .sends()
                            .iter()
                            .any(|send| send.return_track() == return_track && send.gain() > 0.25)
                    })
            })
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().is_some_and(|state| {
                state.project.track(track).is_some_and(|track| {
                    track.sends().iter().any(|send| {
                        send.return_track() == return_track && (send.gain() - 0.25).abs() < 0.000_01
                    })
                })
            })
    });
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
