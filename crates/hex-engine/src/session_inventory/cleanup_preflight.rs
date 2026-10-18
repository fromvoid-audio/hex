//! Check actual retained provider surfaces before moving aggregate owners.
use super::*;

impl PreparedSessionInventory {
    pub(super) fn refuse_opaque_control_drop(&self) -> Result<(), crate::AudioStorageError> {
        self.shared_contexts.refuse_opaque_control_drop()?;
        for row in self.midi_clips.iter().chain(&self.routed_midi_clips) {
            row.instrument.refuse_opaque_control_drop()?;
        }
        for row in &self.mutable_instruments {
            if matches!(row.factory, PreparedTrackRuntimeFactory::Opaque(_))
                || row.history_retarget_target.is_some()
            {
                return Err(crate::AudioStorageError::UnknownStorage);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_drop_preflight_refuses_actual_opaque_factory_without_moving_it() {
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "retained factory".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let state = hex_devices::operator_state::OPERATOR_METADATA
            .patch_parameter(
                &crate::operator_legacy_test::legacy_operator_state(),
                hex_devices::operator_state::parameter::feedback(1),
                ParameterValue::Float(0.25),
            )
            .unwrap();
        editor
            .execute(hex_project::Command::ReplaceDeviceState { device, state })
            .unwrap();
        let hex_project::Created::Scene(scene) = editor
            .execute(hex_project::Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let hex_project::Created::SessionClip(clip) = editor
            .execute(hex_project::Command::CreateSessionMidiClip {
                track,
                scene,
                name: "cell".into(),
                source_length: hex_project::BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("cell")
        };
        editor
            .execute(hex_project::Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let mut inventory = PreparedSessionInventory::prepare(
            editor.project(),
            crate::SharedSineTable::new_on_control(),
            None,
        )
        .unwrap();
        // Isolate the actual factory branch from the aggregate table check.
        inventory.shared_contexts = crate::shared_analytic::SharedAnalyticContextTable::empty();
        let row = inventory
            .mutable_instruments
            .iter()
            .find(|row| row.track == track)
            .unwrap();
        let PreparedTrackRuntimeFactory::Opaque(factory) = &row.factory else {
            panic!("feedback Operator uses the existing opaque provider route")
        };
        let actual = factory.as_ref() as *const dyn PreparedMutableInstrumentRuntime;
        for _ in 0..128 {
            assert_eq!(
                inventory.refuse_opaque_control_drop(),
                Err(crate::AudioStorageError::UnknownStorage)
            );
        }
        let PreparedTrackRuntimeFactory::Opaque(retained) =
            &inventory.mutable_instruments[0].factory
        else {
            panic!("original factory retained")
        };
        assert!(std::ptr::eq(actual, retained.as_ref()));
        assert_eq!(inventory.mutable_instruments.len(), 1);

        // A current table does not describe older tables held by templates.
        editor
            .execute(hex_project::Command::ReplaceDeviceState {
                device,
                state: crate::operator_legacy_test::legacy_operator_state(),
            })
            .unwrap();
        let mut analytic = PreparedSessionInventory::prepare(
            editor.project(),
            crate::SharedSineTable::new_on_control(),
            None,
        )
        .unwrap();
        assert_eq!(analytic.midi_clips.len(), 1);
        assert!(analytic.mutable_instruments.is_empty());
        analytic.shared_contexts = crate::shared_analytic::SharedAnalyticContextTable::empty();
        assert_eq!(
            analytic.refuse_opaque_control_drop(),
            Err(crate::AudioStorageError::UnknownStorage)
        );
        assert_eq!(analytic.midi_clips.len(), 1);
        let empty = PreparedSessionInventory::prepare(
            &Project::new(48_000).unwrap(),
            crate::SharedSineTable::new_on_control(),
            None,
        )
        .unwrap();
        empty.refuse_opaque_control_drop().unwrap();
    }
}
