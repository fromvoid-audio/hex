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
