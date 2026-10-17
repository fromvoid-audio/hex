use super::*;

fn boolean_value(
    view: &EditorView,
    track: TrackId,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
) -> bool {
    let state = view.state.as_ref().expect("state");
    let device = state
        .project
        .track(track)
        .expect("track")
        .devices()
        .iter()
        .find(|item| item.id() == device)
        .expect("device");
    let device = hex_engine::native_device_view(device.state()).expect("parameters");
    match device
        .parameters
        .iter()
        .find(|item| item.descriptor.id == parameter)
        .expect("boolean")
        .value
    {
        ParameterValue::Bool(value) => value,
        _ => panic!("boolean"),
    }
}

#[gpui::test]
fn accessibility_boolean_routes_real_worker_single_undo_and_rejects_old_revision(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, parameter, original) = actor_utility_fixture();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let id = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&ParameterKey { device, parameter }] {
            ParameterControl::Bool { entity, .. } => {
                gpui::AccessibilityId(entity.entity_id().as_u64())
            }
            _ => panic!("boolean"),
        }
    });
    let node = visual.update(|window, _| {
        window
            .accessibility_nodes()
            .find(|node| node.descriptor.id == id)
            .expect("mounted semantic node")
            .clone()
    });
    assert_eq!(node.descriptor.revision, revision);
    assert_eq!(
        node.descriptor.value,
        gpui::AccessibilityValue::Toggle(original)
    );
    assert!(!node.descriptor.name.is_empty());
    assert!(node.bounds.size.width > px(0.) && node.bounds.size.height > px(0.));
    assert!(
        visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision,
                action: gpui::AccessibilityAction::Focus
            },
            app
        ))
    );
    assert_eq!(
        editor.read_with(cx, |view, _| view.state.as_ref().unwrap().revision),
        revision
    );
    assert!(
        visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision,
                action: gpui::AccessibilityAction::Press
            },
            app
        ))
    );
    assert!(
        !visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision,
                action: gpui::AccessibilityAction::Press
            },
            app
        )),
        "second edit before ACK must refuse"
    );
    pump_until(&editor, cx, |view| {
        !view.edit_pending() && boolean_value(view, track, device, parameter) != original
    });
    visual.run_until_parked();
    assert!(
        !visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision,
                action: gpui::AccessibilityAction::Press
            },
            app
        ))
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        !view.edit_pending() && boolean_value(view, track, device, parameter) == original
    });
    visual.run_until_parked();
    assert_eq!(
        visual.update(|window, _| window
            .accessibility_nodes()
            .find(|node| node.descriptor.id == id)
            .expect("stable identity after undo")
            .descriptor
            .value
            .clone()),
        gpui::AccessibilityValue::Toggle(original)
    );
}

#[gpui::test]
fn accessibility_project_generation_and_pending_owner_refuse_before_next_paint(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, parameter, original) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let id = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&ParameterKey { device, parameter }] {
            ParameterControl::Bool { entity, .. } => {
                gpui::AccessibilityId(entity.entity_id().as_u64())
            }
            _ => panic!("boolean"),
        }
    });
    editor.update(cx, |view, _| {
        view.project_generation += 1;
    });
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::Press,
            app
        ))
    );
    assert_eq!(
        editor.read_with(cx, |view, _| boolean_value(view, track, device, parameter)),
        original
    );
    visual.update(|window, app| window.replace_root(app, |_, _| gpui::Empty));
    visual.run_until_parked();
    assert!(
        !visual.update(|window, app| window.perform_accessibility_action(
            id,
            &gpui::AccessibilityAction::Press,
            app
        ))
    );
}

#[gpui::test]
fn accessibility_numeric_uses_canonical_codec_worker_commit_and_one_undo(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let parameter = hex_lattice_contract::ParameterId::from_static(1);
    let original =
        hex_engine::native_device_view(state.project.track(track).unwrap().devices()[0].state())
            .unwrap()
            .parameters
            .iter()
            .find(|item| item.descriptor.id == parameter)
            .unwrap()
            .value;
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let id = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&ParameterKey { device, parameter }] {
            ParameterControl::Float { entity, .. } => {
                gpui::AccessibilityId(entity.entity_id().as_u64())
            }
            _ => panic!("number"),
        }
    });
    let snapshot = visual.update(|window, _| {
        window
            .accessibility_nodes()
            .find(|node| node.descriptor.id == id)
            .unwrap()
            .clone()
    });
    let gpui::AccessibilityValue::Number {
        value,
        min,
        max,
        text,
    } = snapshot.descriptor.value
    else {
        panic!("number");
    };
    assert!(!text.is_empty());
    let next = min + (max - min) * 0.25;
    assert_ne!(next, value);
    assert!(
        visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision,
                action: gpui::AccessibilityAction::SetNumber(next)
            },
            app
        ))
    );
    assert!(
        !visual.update(|window, app| window.perform_accessibility_request(
            &gpui::AccessibilityRequest {
                id,
                revision,
                action: gpui::AccessibilityAction::Increment
            },
            app
        ))
    );
    pump_until(&editor, cx, |view| {
        !view.edit_pending() && view.state.as_ref().unwrap().revision > revision
    });
    let canonical = |view: &EditorView| {
        hex_engine::native_device_view(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()[0]
                .state(),
        )
        .unwrap()
        .parameters
        .iter()
        .find(|item| item.descriptor.id == parameter)
        .unwrap()
        .value
    };
    assert_eq!(
        editor.read_with(cx, |view, _| canonical(view)),
        ParameterValue::Float(next as f32)
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        !view.edit_pending() && canonical(view) == original
    });
    visual.run_until_parked();
    assert!(
        visual.update(|window, _| window
            .accessibility_nodes()
            .any(|node| node.descriptor.id == id)),
        "Undo must not retire the owner entity"
    );
}
