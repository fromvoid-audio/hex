//! Host numeric gesture adaptation shared by device parameters and Return sends.
use super::EditorView;
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
