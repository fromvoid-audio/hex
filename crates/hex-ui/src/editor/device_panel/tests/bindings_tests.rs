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
    use super::super::gesture_adapter::{NumericGestureKey, dispatch_bound_numeric_gesture};
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
        assert_ne!(
            NumericGestureKey::Canonical(view.device_gesture.snapshot().unwrap().key),
            old_key
        );
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

#[gpui::test]
fn retired_numeric_emitter_cannot_address_a_replacement_binding(cx: &mut TestAppContext) {
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
    let (retired, old_entity, current_entity) = editor.update(cx, |view, cx| {
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        let identity = DeviceControlIdentity {
            class: NativeDeviceClass::Effect,
            type_id: metadata.type_id,
        };
        let _ = parameter_control_with_presentation(
            view,
            cx,
            device,
            identity,
            parameter,
            ControlPresentation::Standard,
            ControlText::default(),
        );
        let retired = view.device_panel.controls.remove(&key).unwrap();
        let ParameterControl::Float {
            entity: old_entity, ..
        } = &retired
        else {
            panic!("float");
        };
        let old_entity = old_entity.clone();
        let _ = parameter_control_with_presentation(
            view,
            cx,
            device,
            identity,
            parameter,
            ControlPresentation::Standard,
            ControlText::default(),
        );
        let ParameterControl::Float { entity, .. } = view.device_panel.controls.get(&key).unwrap()
        else {
            panic!("float");
        };
        (retired, old_entity, entity.clone())
    });
    current_entity.update(cx, |_, cx| {
        cx.emit(NumericControlEvent {
            phase: NumericEventPhase::Begin,
            value: 0.0,
            gesture: None,
        })
    });
    let before = editor.read_with(cx, |view, _| view.device_gesture.snapshot().unwrap());
    for phase in [
        NumericEventPhase::Begin,
        NumericEventPhase::Change,
        NumericEventPhase::Commit,
        NumericEventPhase::Cancel,
    ] {
        old_entity.update(cx, |_, cx| {
            cx.emit(NumericControlEvent {
                phase,
                value: 0.25,
                gesture: None,
            })
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(view.device_gesture.snapshot(), Some(before));
            assert!(!view.device_gesture.has_pending_terminal());
        });
    }
    drop(retired);
}

#[gpui::test]
fn synchronous_numeric_owner_finish_does_not_reopen_queued_noop_drag(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector =
        Box::leak(format!("device-{}-parameter-2-knob", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).expect("utility balance dial");
    let start = bounds.center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    // Horizontal motion crosses the drag threshold but leaves this vertical
    // dial's value unchanged. Finish in the same App update, before Begin is
    // delivered to its owner.
    visual.update(|window, cx| {
        window.dispatch_input(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: point(start.x + px(12.0), start.y),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        editor.update(cx, |view, cx| {
            assert!(view.device_gesture.snapshot().is_none());
            assert!(finalize_controls(view, cx));
        });
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_gesture.snapshot().is_none(),
            "queued Begin reopened the finalized drag"
        );
        assert!(!view.device_gesture.has_pending_terminal());
    });
    // The fence is one delivery cycle, not a permanently disabled binding.
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x, start.y - px(12.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_gesture.snapshot().is_some())
    });
}

#[gpui::test]
fn owner_finalization_retires_a_draft_from_an_old_project_generation(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let revision = state.revision;
    let parameter = hex_lattice_contract::ParameterId::from_static(3);
    let key = ParameterKey { device, parameter };
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "250%");
    editor.update(cx, |view, cx| {
        // A replacement lifecycle can advance before the next render clears
        // the old panel. The same numeric IDs must not confer edit ownership.
        view.project_generation += 1;
        assert!(finalize_controls(view, cx));
        assert!(!view.device_panel.controls.contains_key(&key));
        assert!(view.device_gesture.snapshot().is_none());
        assert!(view.device_gesture_barriers.is_empty());
        assert!(view.device_panel.pending_values.is_empty());
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
    });
}
