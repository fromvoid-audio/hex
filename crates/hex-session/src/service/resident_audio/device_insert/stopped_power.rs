//! Narrow command identity; the sink separately proves the installed stopped interval.
use hex_project::{Command, Device, Project};

pub(super) fn eligible(project: &Project, command: &Command) -> bool {
    let [device] = project.main_devices() else {
        return false;
    };
    if device.state().type_id() != hex_lattice_contract::DeviceTypeId::from_static("hex.vst3") {
        return false;
    }
    match command {
        Command::ToggleDeviceBypass { device: target } => *target == device.id(),
        Command::SetDeviceBypass {
            device: target,
            bypassed,
        } => *target == device.id() && *bypassed != device.bypassed(),
        _ => false,
    }
}

pub(super) fn return_power_on_target<'a>(
    project: &'a Project,
    command: &Command,
) -> Option<(hex_engine::ProcessorChainOwner, &'a Device)> {
    let target = match command {
        Command::ToggleDeviceBypass { device }
        | Command::SetDeviceBypass {
            device,
            bypassed: false,
        } => *device,
        _ => return None,
    };
    let (owner, device) = crate::service::audio_sink::sole_hosted_return(project)?;
    (device.id() == target && device.bypassed()).then_some((owner, device))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Created, Editor};

    #[test]
    fn only_one_existing_hosted_main_power_change_qualifies() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_lattice_contract::NativeDeviceState::new(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.vst3"),
                    1,
                    Vec::new(),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("main fixture")
        };
        let off = Command::SetDeviceBypass {
            device,
            bypassed: true,
        };
        assert!(eligible(editor.project(), &off));
        assert!(eligible(
            editor.project(),
            &Command::ToggleDeviceBypass { device }
        ));
        assert!(!eligible(
            editor.project(),
            &Command::SetDeviceBypass {
                device,
                bypassed: false
            }
        ));
        editor.execute(off.clone()).unwrap();
        assert!(!eligible(editor.project(), &off));
        editor.execute(Command::RemoveDevice(device)).unwrap();
        assert!(!eligible(editor.project(), &off));
        let Created::Device(native) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("native fixture")
        };
        assert!(!eligible(
            editor.project(),
            &Command::ToggleDeviceBypass { device: native }
        ));
    }

    #[test]
    fn return_fresh_on_requires_the_sole_authored_off_target() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack { name: "fx".into() })
            .unwrap()
        else {
            panic!("return fixture");
        };
        let Created::Device(device) = editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: hex_lattice_contract::NativeDeviceState::new(
                    hex_lattice_contract::VST3_DEVICE_TYPE_ID,
                    1,
                    vec![1],
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("hosted return fixture");
        };
        let on = Command::SetDeviceBypass {
            device,
            bypassed: false,
        };
        let off = Command::SetDeviceBypass {
            device,
            bypassed: true,
        };
        assert!(return_power_on_target(editor.project(), &on).is_none());
        editor.execute(off.clone()).unwrap();
        let (owner, target) = return_power_on_target(editor.project(), &on).unwrap();
        assert_eq!(owner, hex_engine::ProcessorChainOwner::Return(bus.value()));
        assert_eq!(target.id(), device);
        assert!(return_power_on_target(editor.project(), &off).is_none());
        editor
            .execute(Command::AddReturnTrack {
                name: "other".into(),
            })
            .unwrap();
        assert!(return_power_on_target(editor.project(), &on).is_none());
    }
}
