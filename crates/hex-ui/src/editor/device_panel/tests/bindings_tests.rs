use super::*;

#[gpui::test]
fn late_numeric_phases_cannot_change_finish_or_cancel_another_parameter_gesture(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let metadata =
        hex_engine::native_device_view(state.project.device(device).unwrap().state()).unwrap();
    let mut floats = metadata
        .parameters
        .iter()
        .filter(|p| matches!(p.value, ParameterValue::Float(_)));
    let first = floats.next().unwrap();
    let second = floats.next().unwrap();
    let target = |parameter| GestureTarget::DeviceParameter { device, parameter };
    let stale = target(first.descriptor.id);
    let current = target(second.descriptor.id);
    let initial = second.value;
    let late = if initial == ParameterValue::Float(0.25) {
        ParameterValue::Float(0.5)
    } else {
        ParameterValue::Float(0.25)
    };
    let (_, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, _| {
        let revision = view.state.as_ref().unwrap().revision;
        view.begin_device_gesture(current, initial).unwrap();
        let before = view.device_gesture.snapshot().unwrap();
        for phase in [
            NumericEventPhase::Change,
            NumericEventPhase::Commit,
            NumericEventPhase::Cancel,
        ] {
            dispatch_numeric_gesture(view, stale, phase, late).unwrap();
            assert_eq!(view.device_gesture.snapshot(), Some(before));
            assert!(!view.device_gesture.has_pending_terminal());
        }
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
    });
}

#[gpui::test]
fn retiring_numeric_control_does_not_cancel_an_unowned_same_parameter_gesture(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let metadata =
        hex_engine::native_device_view(state.project.device(device).unwrap().state()).unwrap();
    let parameter = metadata
        .parameters
        .iter()
        .find(|p| matches!(p.value, ParameterValue::Float(_)))
        .unwrap();
    let key = ParameterKey {
        device,
        parameter: parameter.descriptor.id,
    };
    let (_, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        let _ = parameter_control_with_presentation(
            view,
            cx,
            device,
            DeviceControlIdentity {
                class: NativeDeviceClass::Effect,
                type_id: metadata.type_id,
            },
            parameter,
            ControlPresentation::Standard,
            ControlText::default(),
        );
        let mut old = view.device_panel.controls.remove(&key).unwrap();
        view.begin_device_gesture(
            GestureTarget::DeviceParameter {
                device,
                parameter: key.parameter,
            },
            parameter.value,
        )
        .unwrap();
        let current = view.device_gesture.snapshot().unwrap();
        cancel_control(view, cx, key, &mut old);
        assert_eq!(view.device_gesture.snapshot(), Some(current));
        assert!(!view.device_gesture.has_pending_terminal());
    });
}

#[gpui::test]
fn numeric_binding_lease_cannot_adopt_a_newer_same_parameter_gesture(cx: &mut TestAppContext) {
    use super::super::gesture_adapter::dispatch_bound_numeric_gesture;
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let metadata =
        hex_engine::native_device_view(state.project.device(device).unwrap().state()).unwrap();
    let parameter = metadata
        .parameters
        .iter()
        .find(|p| matches!(p.value, ParameterValue::Float(_)))
        .unwrap();
    let target = GestureTarget::DeviceParameter {
        device,
        parameter: parameter.descriptor.id,
    };
    let (_, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, _| {
        let lease = std::rc::Rc::new(std::cell::Cell::new(None));
        dispatch_bound_numeric_gesture(
            view,
            target,
            &lease,
            NumericEventPhase::Begin,
            parameter.value,
        )
        .unwrap();
        let old_key = lease.get().unwrap();
        // Simulate the acknowledged retirement of this edit followed by a new
        // control beginning the same parameter in the same project generation.
        view.device_gesture.reset();
        view.begin_device_gesture(target, parameter.value).unwrap();
        assert_ne!(view.device_gesture.snapshot().unwrap().key, old_key);
        let before = view.device_gesture.snapshot();
        for phase in [
            NumericEventPhase::Change,
            NumericEventPhase::Commit,
            NumericEventPhase::Cancel,
        ] {
            dispatch_bound_numeric_gesture(view, target, &lease, phase, parameter.value).unwrap();
            assert_eq!(view.device_gesture.snapshot(), before);
            assert!(!view.device_gesture.has_pending_terminal());
        }
    });
}
