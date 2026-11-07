//! Source-stamped manual parameter and automation read-selection proposals.
//! The worker's ordinary prepared Edit path owns publication and history.
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub enum DeviceAutomationControlEdit {
    ManualParameter {
        device: hex_project::DeviceId,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
    },
    ReadSelection {
        device: hex_project::DeviceId,
        target: hex_lattice_contract::DeviceAutomationTarget,
        selection: hex_project::AutomationReadSelection,
    },
    Reenable,
}

/// Ordinary preparation remains the authority for SDK flags, raw parameter
/// IDs, discrete steps and quantum capacity. This preflight prevents stale
/// class or placement from reaching that prepare-before-commit transaction.
pub(super) fn accepts_lane_target(
    session: &Session,
    device: hex_project::DeviceId,
    target: hex_lattice_contract::DeviceAutomationTarget,
    vst3_class: Option<[u8; 16]>,
) -> bool {
    match target {
        hex_lattice_contract::DeviceAutomationTarget::Vst3 { .. } => {
            vst3_class.is_some()
                && session.native_device_class(device).ok()
                    == Some(hex_engine::NativeDeviceClass::Effect)
                && session
                    .project()
                    .device(device)
                    .and_then(|device| vst3_automation_class(device.state()))
                    == vst3_class
        }
        _ => {
            vst3_class.is_none()
                && hex_engine::project_device_automation_target_supported(
                    session.project(),
                    device,
                    target,
                )
        }
    }
}

pub(super) fn command(
    session: &Session,
    edit: &DeviceAutomationControlEdit,
) -> Result<Command, String> {
    match *edit {
        DeviceAutomationControlEdit::ManualParameter {
            device,
            parameter,
            value,
        } => {
            let class = session
                .native_device_class(device)
                .map_err(|e| e.to_string())?;
            let current = session
                .project()
                .device(device)
                .ok_or("device no longer exists")?;
            let state = hex_engine::patch_native_device_parameter_for_class(
                class,
                current.state(),
                parameter,
                value,
            )
            .map_err(|e| e.to_string())?;
            Ok(crate::device_gesture::manual_device_edit(
                session,
                device,
                state,
                [parameter],
                true,
            ))
        }
        DeviceAutomationControlEdit::ReadSelection {
            device,
            target,
            selection,
        } => {
            if !hex_engine::project_device_automation_target_supported(
                session.project(),
                device,
                target,
            ) || session
                .project()
                .device(device)
                .and_then(|device| device.automation_target(target))
                .is_none()
            {
                return Err("automation edit refused · target no longer available".into());
            }
            Ok(Command::SetDeviceAutomationReadSelection {
                device,
                target,
                selection,
            })
        }
        DeviceAutomationControlEdit::Reenable => Ok(Command::ReenableDeviceAutomation),
    }
}
