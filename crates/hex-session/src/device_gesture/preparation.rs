//! Turn one already ordered, checked gesture snapshot into the ordinary canonical command.
//! No compiler, source bank, device runtime or publication owner belongs to this boundary.
use super::*;
use hex_project::Command;

pub(crate) fn command_for_snapshot(
    session: &Session,
    snapshot: GestureSnapshot,
) -> Result<Command, SessionError> {
    if let Some(parameters) = snapshot.parameters {
        let GestureTarget::DeviceParameterSnapshot { device } = snapshot.target else {
            return Err(SessionError::Invalid("snapshot target changed"));
        };
        let current = session
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_snapshot_for_class(
            session.native_device_class(device)?,
            current.state(),
            &parameters,
        )?;
        return Ok(Command::ReplaceDeviceState { device, state });
    }
    match snapshot.target {
        GestureTarget::TrackGain { track } => Ok(Command::Gain {
            track,
            gain: track_gain_value(snapshot.value)?,
        }),
        GestureTarget::TrackBalance { track } => Ok(Command::Balance {
            track,
            balance: mix_value(snapshot.value, -1.0, 1.0)?,
        }),
        GestureTarget::ReturnGain { return_track } => Ok(Command::ReturnGain {
            return_track,
            gain: mix_value(snapshot.value, 0.0, 2.0)?,
        }),
        GestureTarget::ReturnSend {
            track,
            return_track,
        } => Ok(Command::SetReturnSend {
            track,
            return_track,
            gain: send_gain(snapshot.value)?,
        }),
        GestureTarget::DeviceParameter { device, parameter } => {
            let current = session
                .project()
                .device(device)
                .ok_or(SessionError::Invalid("device no longer exists"))?;
            let state = hex_engine::patch_native_device_parameter_for_class(
                session.native_device_class(device)?,
                current.state(),
                parameter,
                snapshot.value,
            )?;
            Ok(Command::ReplaceDeviceState { device, state })
        }
        GestureTarget::DeviceParameterPair {
            device,
            first,
            second,
        } => {
            if first == second {
                return Err(SessionError::Invalid(
                    "paired device gesture needs distinct parameters",
                ));
            }
            let second_value = snapshot
                .secondary_value
                .ok_or(SessionError::Invalid("paired gesture value is missing"))?;
            let current = session
                .project()
                .device(device)
                .ok_or(SessionError::Invalid("device no longer exists"))?;
            let class = session.native_device_class(device)?;
            let state = hex_engine::patch_native_device_parameter_for_class(
                class,
                current.state(),
                first,
                snapshot.value,
            )?;
            let state = hex_engine::patch_native_device_parameter_for_class(
                class,
                &state,
                second,
                second_value,
            )?;
            Ok(Command::ReplaceDeviceState { device, state })
        }
        GestureTarget::DeviceParameterSnapshot { .. } => {
            Err(SessionError::Invalid("snapshot parameters are missing"))
        }
    }
}
