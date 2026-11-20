//! Narrow command identity; the sink separately proves the installed stopped interval.
use hex_project::{Command, Project};

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
}
