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
