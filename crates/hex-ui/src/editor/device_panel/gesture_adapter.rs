//! Host numeric gesture adaptation shared by device parameters and Return sends.
use super::EditorView;
use hex_session::device_gesture::GestureKey;
use std::{cell::Cell, rc::Rc};

/// A widget owns only the gesture token it successfully began.
pub(in crate::editor) type NumericGestureLease = Rc<Cell<Option<NumericGestureKey>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum NumericGestureKey {
    Canonical(GestureKey),
    NativeAutomation(GestureKey),
}

pub(in crate::editor) fn owns_native_automation(
    view: &EditorView,
    target: GestureTarget,
    lease: &NumericGestureLease,
) -> bool {
    match lease.get() {
        Some(NumericGestureKey::NativeAutomation(key)) => {
            view.native_automation_recorder.owns(target, Some(key))
        }
        _ => false,
    }
}
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
    if matches!(target, GestureTarget::PluginParameter { .. }) {
        return Err("plugin parameters require lossless sdk conversion");
    }
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
    if matches!(target, GestureTarget::PluginParameter { .. }) {
        return Err("plugin parameters require lossless sdk conversion");
    }
    if !finalized.explicit_edit
        && finalized.value == finalized.previous
        && view.device_gesture.snapshot().is_none()
    {
        return Ok(());
    }
    let previous = ParameterValue::Float(finalized.previous as f32);
    let value = ParameterValue::Float(finalized.value as f32);
    let active = view.device_gesture.snapshot();
    if active.is_some_and(|snapshot| snapshot.target != target)
        || view.device_gesture.has_pending_terminal()
        || (matches!(
            target,
            GestureTarget::MainGain
                | GestureTarget::ReturnGain { .. }
                | GestureTarget::ReturnSend { .. }
        ) && view.document_audio_mode != hex_session::DocumentAudioMode::Resident)
    {
        let command = match target {
            GestureTarget::MainGain => Request::Edit(Command::MasterGain {
                gain: match value {
                    ParameterValue::Float(gain) => gain,
                    _ => return Err("main gain requires a float value"),
                },
            }),
            GestureTarget::TrackBalance { track } => Request::Edit(Command::Balance {
                track,
                balance: match value {
                    ParameterValue::Float(balance) => balance,
                    _ => return Err("track balance requires a float value"),
                },
            }),
            GestureTarget::ReturnGain { return_track } => Request::Edit(Command::ReturnGain {
                return_track,
                gain: match value {
                    ParameterValue::Float(gain) => gain,
                    _ => return Err("return gain requires a float value"),
                },
            }),
            GestureTarget::TrackGain { track } => Request::Edit(Command::Gain {
                track,
                gain: match value {
                    ParameterValue::Float(gain) => gain,
                    _ => return Err("track gain requires a float value"),
                },
            }),
            GestureTarget::DeviceParameter { device, parameter } => {
                view.manual_device_parameter_request(device, parameter, value)?
            }
            GestureTarget::TrackGains { .. } => {
                return Err("selected gains require their captured gesture");
            }
            GestureTarget::DeviceParameterPair { .. } => {
                return Err("paired parameter edits use the graph gesture");
            }
            GestureTarget::DeviceParameterSnapshot { .. }
            | GestureTarget::EqNodeCreation { .. }
            | GestureTarget::PluginParameter { .. } => {
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
        if finalized.explicit_edit || finalized.value != finalized.previous {
            view.change_device_gesture(value)?;
        }
        view.finish_device_gesture(value)?;
        return Ok(());
    }
    view.begin_device_gesture(target, previous)?;
    if finalized.explicit_edit || finalized.value != finalized.previous {
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
        if let Some(key) = view.begin_native_automation_touch(target, value)? {
            lease.set(Some(NumericGestureKey::NativeAutomation(key)));
            return Ok(());
        }
        view.begin_device_gesture(target, value)?;
        lease.set(
            view.device_gesture
                .snapshot()
                .map(|snapshot| NumericGestureKey::Canonical(snapshot.key)),
        );
        return Ok(());
    }
    if owns_native_automation(view, target, lease) {
        view.update_native_automation_touch(phase, value)?;
    } else {
        if !owns_numeric_gesture(view, target, lease) {
            return Ok(());
        }
        dispatch_numeric_gesture(view, target, phase, value)?;
    }
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
    owns_native_automation(view, target, lease)
        || (!view.device_gesture.has_pending_terminal()
            && view.device_gesture.snapshot().is_some_and(|snapshot| {
                Some(NumericGestureKey::Canonical(snapshot.key)) == lease.get()
                    && snapshot.target == target
                    && snapshot.key.generation == view.project_generation
            }))
}

pub(super) fn commit_bound_numeric_edit(
    view: &mut EditorView,
    target: GestureTarget,
    lease: &NumericGestureLease,
    finalized: &NumericControlFinalValue,
) -> Result<(), &'static str> {
    if owns_native_automation(view, target, lease) {
        view.update_native_automation_touch(
            NumericEventPhase::Commit,
            ParameterValue::Float(finalized.value as f32),
        )?;
        lease.set(None);
        return Ok(());
    }
    let active = view.device_gesture.snapshot();
    if active.is_none() && !finalized.explicit_edit && finalized.value == finalized.previous {
        return Ok(());
    }
    if active.is_none() {
        view.begin_device_gesture(target, ParameterValue::Float(finalized.previous as f32))?;
        lease.set(
            view.device_gesture
                .snapshot()
                .map(|snapshot| NumericGestureKey::Canonical(snapshot.key)),
        );
    } else if !owns_numeric_gesture(view, target, lease) {
        // Another control may own the same parameter. Submit after its terminal
        // through the existing barrier rather than adopting that gesture.
        let GestureTarget::DeviceParameter { device, parameter } = target else {
            return Err("bound numeric edit requires a device parameter");
        };
        return if view.enqueue_device_barrier(view.manual_device_parameter_request(
            device,
            parameter,
            ParameterValue::Float(finalized.value as f32),
        )?) {
            Ok(())
        } else {
            Err("device edit queue is full")
        };
    }
    if finalized.explicit_edit || finalized.value != finalized.previous {
        view.change_device_gesture(ParameterValue::Float(finalized.value as f32))?;
    }
    view.finish_device_gesture(ParameterValue::Float(finalized.value as f32))?;
    lease.set(None);
    Ok(())
}
