//! Control-side provider validation for both authored comparison slots.
//! No DSP instance, media admission, runtime owner or callback work is created.
use hex_engine::{Invalid, NativeDeviceClass};
use hex_lattice_contract::NativeDeviceState;
use hex_project::{DeviceComparison, Project};

pub(crate) fn validate_states(
    class: NativeDeviceClass,
    state: &NativeDeviceState,
    comparison: Option<&DeviceComparison>,
) -> Result<(), Invalid> {
    for authored in std::iter::once(state).chain(comparison.map(DeviceComparison::inactive)) {
        hex_engine::native_device_view_for_class(class, authored)?;
        if authored.type_id() != state.type_id() {
            return Err(Invalid(
                "device comparison provider does not match active state",
            ));
        }
    }
    Ok(())
}

/// Existing Engine compilation validates active states. A comparison adds one
/// inactive provider input which must also refuse malformed payloads before the
/// candidate can enter canonical state/history or a newly opened Session.
pub(crate) fn validate_project(project: &Project) -> Result<(), Invalid> {
    for track in project.tracks() {
        if let Some(device) = track
            .instrument()
            .filter(|device| device.comparison().is_some())
        {
            validate_states(
                NativeDeviceClass::Instrument,
                device.state(),
                device.comparison(),
            )?;
        }
        for device in track
            .devices()
            .iter()
            .filter(|device| device.comparison().is_some())
        {
            validate_states(
                NativeDeviceClass::Effect,
                device.state(),
                device.comparison(),
            )?;
        }
    }
    for device in project
        .return_tracks()
        .iter()
        .flat_map(|track| track.devices())
        .chain(project.main_devices())
        .filter(|device| device.comparison().is_some())
    {
        validate_states(
            NativeDeviceClass::Effect,
            device.state(),
            device.comparison(),
        )?;
    }
    Ok(())
}

/// True only for a single comparison operation (or its exact history inverse).
/// Strip no audible field: active provider bytes, assets, host metadata, bypass,
/// routing, MIDI, tuning, IDs and every other Project field must stay identical.
pub(crate) fn metadata_pair(
    source: &Project,
    target: &Project,
) -> Result<bool, crate::SessionError> {
    validate_project(source)?;
    validate_project(target)?;
    let mut left = source.snapshot();
    let mut right = target.snapshot();
    let devices = |snapshot: &mut hex_project::Snapshot| {
        snapshot
            .tracks
            .iter_mut()
            .flat_map(|track| track.instrument.iter_mut().chain(track.devices.iter_mut()))
            .chain(
                snapshot
                    .return_tracks
                    .iter_mut()
                    .flat_map(|track| track.devices.iter_mut()),
            )
            .chain(snapshot.main_devices.iter_mut())
            .chain(
                snapshot
                    .native_sources
                    .iter_mut()
                    .flat_map(|source| source.devices.iter_mut()),
            )
            .for_each(|device| device.comparison = None);
    };
    devices(&mut left);
    devices(&mut right);
    if left != right || source == target {
        return Ok(false);
    }
    // Prove a real typed Switch/Copy edge, not arbitrary metadata equivalence.
    for (from, to) in [(source, target), (target, source)] {
        for device in from
            .tracks()
            .iter()
            .flat_map(|track| track.devices().iter())
            .chain(
                from.return_tracks()
                    .iter()
                    .flat_map(|track| track.devices().iter()),
            )
            .chain(from.main_devices())
            .chain(
                from.native_sources()
                    .iter()
                    .flat_map(|source| source.devices()),
            )
        {
            if to
                .device(device.id())
                .is_none_or(|other| other.comparison() == device.comparison())
            {
                continue;
            }
            for action in [
                hex_project::DeviceComparisonAction::Switch,
                hex_project::DeviceComparisonAction::CopyToInactive,
            ] {
                let mut trial = hex_project::Editor::new(from.clone(), 1)?;
                trial.execute(hex_project::Command::CompareDevice {
                    device: device.id(),
                    expected: device.active_variant(),
                    action,
                })?;
                if trial.project() == to {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    use hex_project::{Command, DeviceComparisonAction as Action, DeviceVariant as Slot, Editor};
    #[test]
    fn main_comparison_typed_metadata_edges_include_main_without_stripping_audio() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state_for_class(
                    NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap();
        let source = editor.project().clone();
        let device = source.main_devices()[0].id();
        editor
            .execute(Command::CompareDevice {
                device,
                expected: Slot::A,
                action: Action::Switch,
            })
            .unwrap();
        let switched = editor.project().clone();
        assert!(
            metadata_pair(&source, &switched).unwrap(),
            "same-audio typed Main switch is metadata"
        );
        assert!(
            metadata_pair(&switched, &source).unwrap(),
            "exact undo edge"
        );
        editor
            .execute(Command::CompareDevice {
                device,
                expected: Slot::B,
                action: Action::CopyToInactive,
            })
            .unwrap();
        // When banks already match, Copy is an honest no-op rather than a metadata edit.
        assert_eq!(editor.project(), &switched);
        let mut copy = Editor::new(source.clone(), 16).unwrap();
        copy.execute(Command::CompareDevice {
            device,
            expected: Slot::A,
            action: Action::CopyToInactive,
        })
        .unwrap();
        assert_eq!(
            copy.project(),
            &source,
            "absent B already represents an identical bank"
        );
        assert!(
            !metadata_pair(&source, copy.project()).unwrap(),
            "no-op is not a metadata edit"
        );
        let active = switched.device(device).unwrap().state().clone();
        let inactive = hex_engine::patch_native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            &active,
            hex_lattice_contract::ParameterId::from_static(1),
            hex_lattice_contract::ParameterValue::Float(0.25),
        )
        .unwrap()
        .with_host_metadata_from(&active)
        .unwrap();
        assert_ne!(active, inactive);
        copy = Editor::new(switched.clone(), 16).unwrap();
        copy.execute(Command::ReplaceDeviceComparison {
            device,
            state: active,
            comparison: Some(DeviceComparison::new(Slot::B, inactive)),
        })
        .unwrap();
        let before_copy = copy.project().clone();
        copy.execute(Command::CompareDevice {
            device,
            expected: Slot::B,
            action: Action::CopyToInactive,
        })
        .unwrap();
        assert_eq!(
            copy.project(),
            &switched,
            "directional copy retains active bank and audible state"
        );
        assert!(metadata_pair(&before_copy, copy.project()).unwrap());
        assert!(
            metadata_pair(copy.project(), &before_copy).unwrap(),
            "exact copy Undo edge"
        );
        copy.execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
        assert!(
            !metadata_pair(&switched, copy.project()).unwrap(),
            "bypass must not be stripped"
        );
    }

    #[test]
    fn comparison_metadata_classification_refuses_audio_changes_arbitrary_bank_edits_and_malformed_payloads()
     {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        editor
            .execute(Command::AddMidiTrack { name: "one".into() })
            .unwrap();
        let track = editor.project().tracks()[0].id();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_engine::default_native_device_state_for_class(
                    NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap();
        let source = editor.project().clone();
        let device = source.tracks()[0].devices()[0].id();
        editor
            .execute(Command::CompareDevice {
                device,
                expected: Slot::A,
                action: Action::Switch,
            })
            .unwrap();
        let compared = editor.project().clone();
        assert!(metadata_pair(&source, &compared).unwrap());
        assert!(
            metadata_pair(&compared, &source).unwrap(),
            "exact history inverse"
        );
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        assert!(!metadata_pair(&compared, editor.project()).unwrap());
        let mut trim = Editor::new(compared.clone(), 16).unwrap();
        let state = compared
            .device(device)
            .unwrap()
            .state()
            .clone()
            .with_input_gain_db(-3.0)
            .unwrap();
        trim.execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        assert!(
            !metadata_pair(&compared, trim.project()).unwrap(),
            "global host trim is audible"
        );
        let mut arbitrary = Editor::new(source.clone(), 16).unwrap();
        let active = source.device(device).unwrap().state().clone();
        let inactive = active.clone().with_output_gain_db(-3.0).unwrap();
        // A real provider payload variation, while host trims stay global.
        let inactive = hex_engine::patch_native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            &inactive,
            hex_lattice_contract::ParameterId::from_static(1),
            hex_lattice_contract::ParameterValue::Float(0.25),
        )
        .unwrap()
        .with_host_metadata_from(&active)
        .unwrap();
        arbitrary
            .execute(Command::ReplaceDeviceComparison {
                device,
                state: active.clone(),
                comparison: Some(DeviceComparison::new(Slot::A, inactive)),
            })
            .unwrap();
        assert!(
            !metadata_pair(&source, arbitrary.project()).unwrap(),
            "an arbitrary inactive-state edit is not a typed comparison edge"
        );
        let malformed =
            NativeDeviceState::new(active.type_id(), active.state_version(), vec![255]).unwrap();
        let mut broken = Editor::new(source.clone(), 16).unwrap();
        broken
            .execute(Command::ReplaceDeviceComparison {
                device,
                state: active,
                comparison: Some(DeviceComparison::new(Slot::A, malformed)),
            })
            .unwrap();
        assert!(metadata_pair(&source, broken.project()).is_err());
    }
}
