use super::*;

fn save_button(visual: &mut VisualTestContext, device: DeviceId) -> Bounds<Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("device-preset-save-{}", device.value()).into_boxed_str(),
        ))
        .expect("discoverable save action")
}

#[gpui::test]
fn device_preset_rendered_save_cancel_and_stale_dialog_preserve_project(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let owner = hex_session::service::preset::Owner {
        project_generation: 0,
        revision,
        device,
        class: NativeDeviceClass::Effect,
        type_id: state.project.device(device).unwrap().state().type_id(),
    };
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let save = save_button(&mut visual, device);
    let load = visual
        .debug_bounds(Box::leak(
            format!("device-preset-load-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(load.size.width > px(0.0));
    let frame = visual
        .debug_bounds(Box::leak(
            format!("effect-device-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(save.origin.x >= frame.origin.x && save.right() <= frame.right());
    assert!(load.origin.x >= frame.origin.x && load.right() <= frame.right());
    visual.simulate_click(save.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.preset_dialog_open));
    cx.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(!view.preset_dialog_open);
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.worker_requests_pending, 0);
    });
    editor.update(cx, |view, cx| {
        view.choose_device_preset(
            hex_session::service::preset::Owner {
                project_generation: 99,
                ..owner
            },
            std::path::PathBuf::from("should-not-exist.hex-device-preset.json"),
            true,
            cx,
        );
        assert!(view.error.as_deref().unwrap().contains("project changed"));
        assert_eq!(view.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn device_preset_rendered_save_and_worker_load_one_history_step(cx: &mut TestAppContext) {
    let (service, state, track, device, parameter, original_bool) = actor_utility_fixture();
    let original = state.project.clone();
    let directory = std::env::temp_dir().join(format!(
        "hex-preset-ui-{}-{}",
        std::process::id(),
        device.value()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("user.hex-device-preset.json");
    let _ = std::fs::remove_file(&path);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = save_button(&mut visual, device);
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    assert!(path.exists());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    click_toggle(&mut visual, device, parameter);
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    let changed = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let owner = hex_session::service::preset::Owner {
            project_generation: view.project_generation,
            revision: state.revision,
            device,
            class: NativeDeviceClass::Effect,
            type_id: state.project.device(device).unwrap().state().type_id(),
        };
        // The test platform has no open-file prompt implementation. Exercise
        // the actual chosen-path callback and worker instead of faking a dialog.
        view.choose_device_preset(owner, path.clone(), false, cx);
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.error.is_none(), "{:?}", view.error);
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, changed)
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            bool_value(view.state.as_ref().unwrap(), track, device, parameter),
            original_bool
        )
    });
    std::fs::remove_dir_all(directory).unwrap();
}

#[gpui::test]
fn device_preset_return_owner_exposes_actions_and_cancel_keeps_sends(cx: &mut TestAppContext) {
    let (service, state, track, return_track, device, _) = actor_return_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = save_button(&mut visual, device);
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.preset_dialog_open));
    cx.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn device_preset_operator_header_saves_complete_native_state(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let device = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    service
        .submit(Request::ReplaceNativeDevice {
            device,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
        })
        .unwrap();
    let replaced = receive_service_event(&service);
    assert!(replaced.error.is_none());
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
            value: ParameterValue::Float(1.2345),
        })
        .unwrap();
    let replaced = receive_service_event(&service);
    assert!(replaced.error.is_none());
    let original_revision = replaced.state.revision;
    let original = replaced.state.project.clone();
    let directory =
        std::env::temp_dir().join(format!("hex-operator-preset-ui-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("operator.hex-device-preset.json");
    let _ = std::fs::remove_file(&path);
    let (handle, editor) = actor_editor_window(cx, service, replaced.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let save = save_button(&mut visual, device);
    let load = visual
        .debug_bounds(Box::leak(
            format!("device-preset-load-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    let frame = visual
        .debug_bounds(Box::leak(
            format!("instrument-device-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(save.right() <= frame.right() && load.right() <= frame.right());
    let oscillator = visual
        .debug_bounds(Box::leak(
            format!("operator-page-{}-oscillator", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(oscillator.center(), Modifiers::default());
    visual.run_until_parked();
    let ratio = hex_lattice_contract::ParameterId::from_static(1);
    let assert_draft = |visual: &mut VisualTestContext, focused, editing, text: &str| {
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                let ParameterControl::Float { entity, .. } = &view.device_panel.controls
                    [&ParameterKey {
                        device,
                        parameter: ratio,
                    }]
                else {
                    panic!("ratio control");
                };
                let control = entity.read(cx);
                assert_eq!(
                    gpui::Focusable::focus_handle(control, cx).is_focused(window),
                    focused
                );
                assert_eq!(control.is_editing(), editing);
                assert_eq!(control.display_text(), text);
                assert_eq!(control.value(), f64::from(1.2345_f32));
                assert_eq!(view.state.as_ref().unwrap().project, original);
                assert_eq!(view.state.as_ref().unwrap().revision, original_revision);
                assert_eq!(view.worker_requests_pending, 0);
                assert!(view.pending_requests.is_empty());
            });
        });
    };
    enter_typed_value(&mut visual, device, ratio, "--");
    assert_draft(&mut visual, true, true, "--");
    visual.simulate_click(save.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            !view.preset_dialog_open,
            "invalid current draft must refuse save"
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.state.as_ref().unwrap().revision, original_revision);
        assert_eq!(view.worker_requests_pending, 0);
    });
    assert!(!path.exists());
    assert_draft(&mut visual, false, true, "--");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_draft(&mut visual, false, true, "--");
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-value", device.value()).into_boxed_str(),
        ))
        .unwrap();
    // Save moved focus to its card. Reclick must retain the invalid draft;
    // only Escape delivered to this focused input may cancel it.
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    assert_draft(&mut visual, true, true, "--");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_draft(&mut visual, true, false, "1.23");
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: ratio,
        }] else {
            panic!("ratio control");
        };
        assert!(entity.read(cx).is_editing());
        assert_eq!(entity.read(cx).display_text(), "1.23");
    });
    visual.simulate_click(save.center(), Modifiers::default());
    visual.run_until_parked();
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    assert!(path.exists());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(
            view.state.as_ref().unwrap().revision,
            original_revision,
            "untouched rounded display must not author a preset-time edit"
        );
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
            value: ParameterValue::Float(0.27),
        }))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let owner = hex_session::service::preset::Owner {
            project_generation: view.project_generation,
            revision: state.revision,
            device,
            class: NativeDeviceClass::Instrument,
            type_id: state.project.device(device).unwrap().state().type_id(),
        };
        view.choose_device_preset(owner, path.clone(), false, cx);
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    std::fs::remove_dir_all(directory).unwrap();
}
