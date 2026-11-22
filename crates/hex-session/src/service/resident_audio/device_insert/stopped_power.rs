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

pub(super) fn running_return_power_on_target<'a>(
    project: &'a Project,
    command: &Command,
) -> Option<(hex_engine::ProcessorChainOwner, &'a Device)> {
    let (owner, device) = return_power_on_target(project, command)?;
    let hex_engine::ProcessorChainOwner::Return(bus) = owner else {
        return None;
    };
    if !project.main_devices().is_empty()
        || project.has_gain_automation()
        || !(1..=2).contains(&project.tracks().len())
    {
        return None;
    }
    let mut fed_return = false;
    for track in project.tracks() {
        if track.is_group()
            || track.parent_group().is_some()
            || track.clips().len() != 1
            || !track.midi_clips().is_empty()
            || track.instrument().is_some()
        {
            return None;
        }
        if let [send] = track.sends() {
            if send.return_track().value() != bus
                || send.gain() <= 0.0
                || !track.devices().is_empty()
                || fed_return
            {
                return None;
            }
            fed_return = true;
        } else if !track.sends().is_empty()
            || track.devices().iter().any(|dev| {
                !dev.automation().is_empty()
                    || !dev.modulation().sources().is_empty()
                    || !dev.modulation().routes().is_empty()
            })
        {
            return None;
        }
    }
    fed_return.then_some((owner, device))
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

    #[test]
    fn running_return_on_requires_one_direct_audio_track_sending_to_the_sole_off_return() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "dry".into(),
                path: "memory/dry.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("direct audio track");
        };
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
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let on = Command::SetDeviceBypass {
            device,
            bypassed: false,
        };
        assert!(running_return_power_on_target(editor.project(), &on).is_none());
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        let (owner, target) = running_return_power_on_target(editor.project(), &on).unwrap();
        assert_eq!(owner, hex_engine::ProcessorChainOwner::Return(bus.value()));
        assert_eq!(target.id(), device);
        let Created::Imported { track: bed, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "bed".into(),
                path: "memory/bed.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("bed track");
        };
        editor
            .execute(Command::InsertDevice {
                track: bed,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            })
            .unwrap();
        let (owner, target) = running_return_power_on_target(editor.project(), &on).unwrap();
        assert_eq!(owner, hex_engine::ProcessorChainOwner::Return(bus.value()));
        assert_eq!(target.id(), device);
        editor
            .execute(Command::AddTrack {
                name: "extra".into(),
            })
            .unwrap();
        assert!(running_return_power_on_target(editor.project(), &on).is_none());
    }
}
