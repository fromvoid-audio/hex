//! Schema83 adds an explicit compatibility boundary for retained source chains
//! bound to a track. The exact prior codec still owns all state and memberships.
use crate::{BOUND_SOURCE_EFFECT_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION, Project, StorageError};

pub(crate) fn contains(project: &Project) -> bool {
    project.native_sources().iter().any(|source| {
        !source.post_instrument_effects().is_empty()
            && project.source_contributors(source.id()).next().is_some()
    })
}

pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut value: serde_json::Value = serde_json::from_slice(&crate::encode_base(project)?)?;
    let base = value["version"].clone();
    value["version"] = BOUND_SOURCE_EFFECT_SCHEMA_VERSION.into();
    value["source_effect_base_version"] = base;
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid source effect project"))?;
    let base = object
        .remove("source_effect_base_version")
        .and_then(|value| value.as_u64())
        .filter(|version| {
            (u64::from(CURRENT_SCHEMA_VERSION)..u64::from(BOUND_SOURCE_EFFECT_SCHEMA_VERSION))
                .contains(version)
        })
        .ok_or(StorageError::Invalid("invalid source effect base schema"))?;
    object.insert("version".into(), base.into());
    let project = crate::decode_base(&serde_json::to_vec(&value)?)?;
    if !contains(&project) {
        return Err(StorageError::Invalid(
            "bound source effect schema has no bound unit",
        ));
    }
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    use hex_project::{
        AutomationValue, BeatTime, Command, Created, DeviceAutomationTarget,
        DeviceAutomationTouchSpan, DeviceAutomationTouchStep, Editor, TrackDeviceSlot,
    };
    use serde_json::Value;

    fn written_bound_source_with_main_touch(foreign: bool, counter: bool) -> Project {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "source".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver");
        };
        let track = editor.project().track(receiver).unwrap();
        let source = track.source_id().unwrap();
        let instrument = track.instrument().unwrap().id();
        let native = || {
            NativeDeviceState::new(DeviceTypeId::from_static("test.effect"), 1, vec![8]).unwrap()
        };
        editor
            .execute(Command::BindTrackSource {
                track: receiver,
                source: None,
            })
            .unwrap();
        editor
            .execute(Command::InsertNestedDevice {
                parent: instrument,
                state: native(),
            })
            .unwrap();
        editor
            .execute(Command::BindTrackSource {
                track: receiver,
                source: Some(source),
            })
            .unwrap();
        let (state, target, vst3_class) = if foreign {
            let class = [9; 16];
            let state = hex_plugin_state::encode(hex_plugin_state::State {
                identity: hex_plugin_state::Identity {
                    class: hex_plugin_state::ClassId::new(class).unwrap(),
                    binary: hex_plugin_state::BinaryIdentity {
                        executable_sha256: [6; 32],
                        info_sha256: [7; 32],
                    },
                },
                component: Some(&[42; 12]),
                controller: Some(&[]),
            })
            .unwrap();
            (
                state,
                DeviceAutomationTarget::Vst3 { parameter: 0 },
                Some(class),
            )
        } else {
            (native(), DeviceAutomationTarget::HostOutputGain, None)
        };
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("main effect");
        };
        editor
            .execute(Command::SpliceDeviceAutomationTouch {
                device,
                target,
                vst3_class,
                spans: vec![DeviceAutomationTouchSpan {
                    start: BeatTime::ZERO,
                    end: BeatTime::ONE,
                    steps: vec![DeviceAutomationTouchStep {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.25).unwrap(),
                    }],
                }],
            })
            .unwrap();
        if counter {
            use hex_project::{DeviceModulation, ElementColor, ModulationSourceId, Modulator};
            let counter = Modulator {
                id: ModulationSourceId::new(editor.project().next_identity()).unwrap(),
                color: ElementColor::Cyan,
                enabled: true,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("hex.modulator.note-counter"),
                    1,
                    vec![12, 0, 5, 0, 1],
                )
                .unwrap(),
            };
            editor
                .execute(Command::ReplaceDeviceModulation {
                    device: instrument,
                    modulation: DeviceModulation::new(vec![counter], Vec::new()).unwrap(),
                })
                .unwrap();
        }
        editor.project().clone()
    }

    #[test]
    fn formerly_mislabeled_native_and_vst3_touch_leaves_reopen_without_rewriting() {
        for (foreign, counter, leaf, wrapper, base_field) in [
            (
                false,
                false,
                crate::DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION,
                BOUND_SOURCE_EFFECT_SCHEMA_VERSION,
                "source_effect_base_version",
            ),
            (
                true,
                false,
                crate::VST3_AUTOMATION_TOUCH_SCHEMA_VERSION,
                BOUND_SOURCE_EFFECT_SCHEMA_VERSION,
                "source_effect_base_version",
            ),
            (
                false,
                true,
                crate::DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION,
                crate::NOTE_COUNTER_MODULATOR_SCHEMA_VERSION,
                "modulation_base_version",
            ),
        ] {
            let project = written_bound_source_with_main_touch(foreign, counter);
            let canonical: Value =
                serde_json::from_slice(&crate::encode(&project).unwrap()).unwrap();
            assert_eq!(canonical["version"], wrapper);
            assert_eq!(canonical[base_field], crate::SERIAL_DEVICE_SCHEMA_VERSION);
            assert_eq!(canonical["serial_base_version"], leaf);
            // The historical encoder called schema_version_for on the whole
            // project while writing this nested leaf. Only this field differed.
            let mut historical = canonical.clone();
            historical["serial_base_version"] = BOUND_SOURCE_EFFECT_SCHEMA_VERSION.into();
            let historical_bytes = serde_json::to_vec(&historical).unwrap();
            let path = std::env::temp_dir().join(format!(
                "hex-bound-touch-{}-{wrapper}-{leaf}.hex",
                std::process::id()
            ));
            let saved_as = path.with_extension("new.hex");
            std::fs::write(&path, &historical_bytes).unwrap();
            let opened = crate::load(&path).unwrap();
            assert_eq!(opened, project);
            crate::save_new(&saved_as, &opened).unwrap();
            assert_eq!(crate::load(&saved_as).unwrap(), project);
            let normalized: Value =
                serde_json::from_slice(&std::fs::read(&saved_as).unwrap()).unwrap();
            assert_eq!(normalized, canonical);
            assert_eq!(std::fs::read(&path).unwrap(), historical_bytes);
            std::fs::remove_file(path).unwrap();
            std::fs::remove_file(saved_as).unwrap();

            let mut forged = historical.clone();
            if foreign {
                forged["device_automation_touch"][0]["target"]["vst3"]["class"] =
                    serde_json::to_value([8u8; 16]).unwrap();
            } else {
                forged["device_automation_touch"][0]["target"] =
                    serde_json::json!({ "parameter": 0 });
            }
            assert!(crate::decode(&serde_json::to_vec(&forged).unwrap()).is_err());
            let mut wrong_owner = historical.clone();
            wrong_owner["device_automation_touch"][0]["device"] = u64::MAX.into();
            assert!(crate::decode(&serde_json::to_vec(&wrong_owner).unwrap()).is_err());
            historical["version"] = crate::SERIAL_DEVICE_SCHEMA_VERSION.into();
            let outer = historical.as_object_mut().unwrap();
            outer.remove(base_field);
            outer.remove("device_modulation");
            assert!(crate::decode(&serde_json::to_vec(&historical).unwrap()).is_err());
        }
    }
}
