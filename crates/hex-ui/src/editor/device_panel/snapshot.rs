//! Typed snapshot ownership; geometry and parameter semantics stay elsewhere.
use super::{DeviceId, EditorView, GestureTarget, HashSet};

pub(super) fn retire_invalid_owner(view: &mut EditorView, alive: &HashSet<DeviceId>) {
    if view.device_gesture.snapshot().is_some_and(|snapshot| {
        matches!(snapshot.target, GestureTarget::DeviceParameterSnapshot { device } | GestureTarget::EqNodeCreation { device }
            if !alive.contains(&device) || snapshot.key.generation != view.project_generation)
    }) && !view.device_gesture.has_pending_terminal()
    {
        let _ = view.cancel_device_gesture();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
