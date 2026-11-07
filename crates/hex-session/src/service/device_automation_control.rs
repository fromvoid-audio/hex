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
        vst3_class: Option<[u8; 16]>,
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
                && matches!(
                    session.native_device_class(device).ok(),
                    Some(
                        hex_engine::NativeDeviceClass::Effect
                            | hex_engine::NativeDeviceClass::Instrument
                    )
                )
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
            vst3_class,
            selection,
        } => {
            if !accepts_lane_target(session, device, target, vst3_class)
                || session
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_vst3_lane_class_admits_instruments_and_effects_with_raw_zero() {
        use hex_lattice_contract::DeviceAutomationTarget;
        use hex_project::{
            AutomationInterpolation, AutomationPoint, AutomationReadSelection as Read,
            AutomationValue, BeatTime, Created,
        };
        let class = [1; 16];
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new(class).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [2; 32],
                    info_sha256: [3; 32],
                },
            },
            component: Some(b"canonical"),
            controller: None,
        })
        .unwrap();
        for instrument in [false, true] {
            let mut session = Session::new(48_000, 16, 0).unwrap();
            let Created::Track(track) = session
                .editor
                .execute(Command::AddMidiTrack {
                    name: "owner".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            let insertion = if instrument {
                Command::InsertInstrument {
                    track,
                    state: state.clone(),
                }
            } else {
                Command::InsertDevice {
                    track,
                    before: None,
                    state: state.clone(),
                }
            };
            let Created::Device(device) = session.editor.execute(insertion).unwrap() else {
                panic!("device")
            };
            let target = DeviceAutomationTarget::Vst3 { parameter: 0 };
            assert!(accepts_lane_target(&session, device, target, Some(class)));
            assert!(!accepts_lane_target(
                &session,
                device,
                target,
                Some([9; 16])
            ));
            assert!(!accepts_lane_target(&session, device, target, None));
            session
                .editor
                .execute(Command::ReplaceDeviceAutomationTarget {
                    device,
                    target,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.25).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
            let edit = DeviceAutomationControlEdit::ReadSelection {
                device,
                target,
                vst3_class: Some(class),
                selection: Read::Base,
            };
            let prepared = command(&session, &edit).unwrap();
            session.editor.execute(prepared).unwrap();
            assert_eq!(session.project().device(device).unwrap().state(), &state);
            assert_eq!(
                session
                    .project()
                    .device(device)
                    .unwrap()
                    .automation_target(target)
                    .unwrap()
                    .read_selection,
                Read::Base
            );
            let prepared = command(&session, &DeviceAutomationControlEdit::Reenable).unwrap();
            session.editor.execute(prepared).unwrap();
            assert!(
                session
                    .project()
                    .device(device)
                    .unwrap()
                    .automation_target(target)
                    .unwrap()
                    .is_reading()
            );
        }
    }
}
