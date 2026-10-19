//! Host numeric gesture adaptation shared by device parameters and Return sends.
use super::EditorView;
use hex_session::device_gesture::GestureKey;
use std::{cell::Cell, rc::Rc};

/// A widget owns only the gesture token it successfully began.
pub(in crate::editor) type NumericGestureLease = Rc<Cell<Option<GestureKey>>>;
use hex_lattice_contract::ParameterValue;
use hex_project::Command;
use hex_session::{device_gesture::GestureTarget, service::Request};
use hex_widgets::{NumericControlFinalValue, NumericEventPhase};

pub fn dispatch_numeric_gesture(
    view: &mut EditorView,
    target: GestureTarget,
    phase: NumericEventPhase,
    value: ParameterValue,
) -> Result<(), &'static str> {
    // Delayed events from a different control must never address whichever
    // gesture happens to be active now. Incarnation checks remain with bindings.
    if phase != NumericEventPhase::Begin
        && !view.device_gesture.snapshot().is_some_and(|snapshot| {
            snapshot.target == target && snapshot.key.generation == view.project_generation
        })
    {
        return Ok(());
    }
    match phase {
        NumericEventPhase::Begin => view.begin_device_gesture(target, value),
        NumericEventPhase::Change => view.change_device_gesture(value),
        NumericEventPhase::Commit => view.finish_device_gesture(value),
        NumericEventPhase::Cancel => view.cancel_device_gesture(),
    }
}

pub fn commit_owned_numeric_edit(
    view: &mut EditorView,
    target: GestureTarget,
    finalized: &NumericControlFinalValue,
) -> Result<(), &'static str> {
    let previous = ParameterValue::Float(finalized.previous as f32);
    let value = ParameterValue::Float(finalized.value as f32);
    let active = view.device_gesture.snapshot();
    if active.is_some_and(|snapshot| snapshot.target != target)
        || view.device_gesture.has_pending_terminal()
    {
        let command = match target {
            GestureTarget::TrackGain { track } => Request::Edit(Command::Gain {
                track,
                gain: match value {
                    ParameterValue::Float(gain) => gain,
                    _ => return Err("track gain requires a float value"),
                },
            }),
            GestureTarget::DeviceParameter { device, parameter } => Request::SetDeviceParameter {
                device,
                parameter,
                value,
            },
            GestureTarget::DeviceParameterPair { .. } => {
                return Err("paired parameter edits use the graph gesture");
            }
            GestureTarget::DeviceParameterSnapshot { .. } => {
                return Err("snapshot parameter edits use the harmonic gesture");
            }
            GestureTarget::ReturnSend {
                track,
                return_track,
            } => Request::Edit(Command::SetReturnSend {
                track,
                return_track,
                gain: match value {
                    ParameterValue::Float(gain) => gain,
                    _ => return Err("return send requires a float value"),
                },
            }),
        };
        if view.enqueue_device_barrier(command) {
            return Ok(());
        }
        return Err("device edit queue is full");
    }
    if active.is_some() {
        view.change_device_gesture(value)?;
        view.finish_device_gesture(value)?;
        return Ok(());
    }
    view.begin_device_gesture(target, previous)?;
    if finalized.value != finalized.previous {
        view.change_device_gesture(value)?;
    }
    view.finish_device_gesture(value)
}

pub(in crate::editor) fn dispatch_bound_numeric_gesture(
    view: &mut EditorView,
    target: GestureTarget,
    lease: &NumericGestureLease,
    phase: NumericEventPhase,
    value: ParameterValue,
) -> Result<(), &'static str> {
    if phase == NumericEventPhase::Begin {
        view.begin_device_gesture(target, value)?;
        lease.set(view.device_gesture.snapshot().map(|snapshot| snapshot.key));
        return Ok(());
    }
    if !owns_numeric_gesture(view, target, lease) {
        return Ok(());
    }
    dispatch_numeric_gesture(view, target, phase, value)?;
    if matches!(phase, NumericEventPhase::Commit | NumericEventPhase::Cancel) {
        lease.set(None);
    }
    Ok(())
}

pub(in crate::editor) fn owns_numeric_gesture(
    view: &EditorView,
    target: GestureTarget,
    lease: &NumericGestureLease,
) -> bool {
    !view.device_gesture.has_pending_terminal()
        && view.device_gesture.snapshot().is_some_and(|snapshot| {
            Some(snapshot.key) == lease.get()
                && snapshot.target == target
                && snapshot.key.generation == view.project_generation
        })
}

pub(super) fn commit_bound_numeric_edit(
    view: &mut EditorView,
    target: GestureTarget,
    lease: &NumericGestureLease,
    finalized: &NumericControlFinalValue,
) -> Result<(), &'static str> {
    let active = view.device_gesture.snapshot();
    if active.is_none() {
        view.begin_device_gesture(target, ParameterValue::Float(finalized.previous as f32))?;
        lease.set(view.device_gesture.snapshot().map(|snapshot| snapshot.key));
    } else if !owns_numeric_gesture(view, target, lease) {
        // Another control may own the same parameter. Submit after its terminal
        // through the existing barrier rather than adopting that gesture.
        let GestureTarget::DeviceParameter { device, parameter } = target else {
            return Err("bound numeric edit requires a device parameter");
        };
        return if view.enqueue_device_barrier(Request::SetDeviceParameter {
            device,
            parameter,
            value: ParameterValue::Float(finalized.value as f32),
        }) {
            Ok(())
        } else {
            Err("device edit queue is full")
        };
    }
    if finalized.value != finalized.previous {
        view.change_device_gesture(ParameterValue::Float(finalized.value as f32))?;
    }
    view.finish_device_gesture(ParameterValue::Float(finalized.value as f32))?;
    lease.set(None);
    Ok(())
}
