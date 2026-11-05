//! Turn one already ordered, checked gesture snapshot into the ordinary canonical command.
//! No compiler, source bank, device runtime or publication owner belongs to this boundary.
use super::*;
use hex_project::Command;

pub(crate) fn command_for_snapshot(
    session: &Session,
    snapshot: GestureSnapshot,
) -> Result<Command, SessionError> {
    if let Some(group) = snapshot.track_gains {
        if snapshot.target
            != (GestureTarget::TrackGains {
                track: group.anchor(),
            })
            || snapshot.creation.is_some()
            || snapshot.parameters.is_some()
            || snapshot.secondary_value.is_some()
        {
            return Err(SessionError::Invalid("selected gain cohort target changed"));
        }
        return group
            .command(session.project(), track_gain_value(snapshot.value)?)
            .map_err(SessionError::from);
    }
    if let Some(creation) = snapshot.creation {
        let GestureTarget::EqNodeCreation { device } = snapshot.target else {
            return Err(SessionError::Invalid("eq creation target changed"));
        };
        return creation.command(session, device, snapshot.sequence);
    }
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
        return Ok(manual_device_edit(
            session,
            device,
            state,
            snapshot
                .touched_parameters
                .ids(&parameters)
                .map_err(SessionError::Invalid)?,
            snapshot.manual_edit,
        ));
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
        GestureTarget::DeviceParameter { device, parameter } => device_parameter_command(
            session.project(),
            device,
            parameter,
            snapshot.value,
            snapshot.manual_edit,
        ),
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
            Ok(manual_device_edit(
                session,
                device,
                state,
                [first, second],
                snapshot.manual_edit,
            ))
        }
        GestureTarget::TrackGains { .. }
        | GestureTarget::DeviceParameterSnapshot { .. }
        | GestureTarget::EqNodeCreation { .. } => {
            Err(SessionError::Invalid("snapshot parameters are missing"))
        }
    }
}

/// Reconstruct the same single-parameter intent against an authoritative
/// snapshot. This owns validated command construction, never source facts.
pub(crate) fn device_parameter_command(
    project: &Project,
    device: DeviceId,
    parameter: ParameterId,
    value: ParameterValue,
    manual_edit: bool,
) -> Result<Command, SessionError> {
    let current = project
        .device(device)
        .ok_or(SessionError::Invalid("device no longer exists"))?;
    let state = hex_engine::patch_native_device_parameter_for_class(
        Session::native_device_class_for_project(project, device)?,
        current.state(),
        parameter,
        value,
    )?;
    Ok(manual_device_edit_for_project(
        project,
        device,
        state,
        [parameter],
        manual_edit,
    ))
}

/// One neutral manual-selection seam for keyed gestures and trusted direct edits.
/// Unsupported/source-owned targets retain their ordinary base-state edit behavior.
pub(crate) fn manual_device_edit(
    session: &Session,
    device: DeviceId,
    state: hex_lattice_contract::NativeDeviceState,
    parameters: impl IntoIterator<Item = ParameterId>,
    manual_edit: bool,
) -> Command {
    manual_device_edit_for_project(session.project(), device, state, parameters, manual_edit)
}
fn manual_device_edit_for_project(
    project: &Project,
    device: DeviceId,
    state: hex_lattice_contract::NativeDeviceState,
    parameters: impl IntoIterator<Item = ParameterId>,
    manual_edit: bool,
) -> Command {
    if !manual_edit {
        return Command::ReplaceDeviceState { device, state };
    }
    let mut targets: Vec<_> = parameters
        .into_iter()
        .map(automation_target)
        .filter(|target| {
            project
                .device(device)
                .and_then(|device| device.automation_target(*target))
                .is_some()
                && hex_engine::project_device_automation_target_supported(project, device, *target)
        })
        .collect();
    targets.sort_unstable();
    targets.dedup();
    if targets.is_empty() {
        Command::ReplaceDeviceState { device, state }
    } else {
        Command::ManualDeviceEdit {
            device,
            state,
            targets,
        }
    }
}
pub(crate) fn automation_target(
    parameter: ParameterId,
) -> hex_lattice_contract::DeviceAutomationTarget {
    use hex_lattice_contract::DeviceAutomationTarget as Target;
    if parameter == hex_engine::DEVICE_INPUT_GAIN.id {
        Target::HostInputGain
    } else if parameter == hex_engine::DEVICE_OUTPUT_GAIN.id {
        Target::HostOutputGain
    } else {
        Target::Parameter(parameter)
    }
}
