//! Control-side catalog resolution for neutral device asset references.
use crate::{AssetId, Invalid, Project};

impl Project {
    /// Resolve identities only; this performs no file access or PCM admission.
    pub fn validate_device_asset_references(&self) -> Result<(), Invalid> {
        for asset in self
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .flat_map(|c| c.referenced_assets())
            .chain(
                self.session_cells()
                    .iter()
                    .filter_map(|cell| match cell.content() {
                        crate::SessionCellContent::Audio(clip) => Some(clip.asset()),
                        _ => None,
                    }),
            )
        {
            if self
                .asset(asset)
                .is_some_and(|a| a.original_source().is_some())
            {
                return Err(Invalid(
                    "device-original media cannot be used as a clip derivative",
                ));
            }
        }
        for device in self.devices() {
            for reference in device
                .authored_states()
                .flat_map(|state| state.asset_references())
            {
                if self.asset(AssetId(reference.asset.value())).is_none() {
                    return Err(Invalid("device references missing project media"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn device_uses_asset(&self, asset: AssetId) -> bool {
        self.devices().any(|device| {
            device
                .authored_states()
                .flat_map(|state| state.asset_references())
                .any(|reference| reference.asset.value() == asset.value())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor};
    use hex_lattice_contract::{DeviceAssetSlot, DeviceTypeId, NativeDeviceState};
    #[test]
    fn prepared_source_and_empty_instrument_binding_share_one_commit_and_undo() {
        let mut editor = Editor::new(Project::new(16000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "sample".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let before = editor.project().snapshot();
        let command = || Command::ImportDeviceAssetIntoNewInstrument {
            track,
            replacing: None,
            path: "source.wav".into(),
            sample_rate: 8000,
            frames: 4,
            state: NativeDeviceState::new(
                DeviceTypeId::new("test.instrument.resource").unwrap(),
                1,
                vec![],
            )
            .unwrap(),
            source_slot: DeviceAssetSlot::new(1).unwrap(),
        };
        let rejected: Result<_, Invalid> = editor.execute_prepared(command(), |candidate| {
            assert_eq!(candidate.assets().count(), 1);
            let target = candidate.track(track).unwrap().instrument().unwrap();
            assert_eq!(target.state().asset_references().len(), 1);
            Err::<(), _>(Invalid("prepared source refused"))
        });
        assert!(rejected.is_err());
        assert_eq!(editor.project().snapshot(), before);
        let (Created::Asset(asset), ()) = editor
            .execute_prepared(command(), |_| Ok::<_, Invalid>(()))
            .unwrap()
        else {
            panic!("asset")
        };
        assert_eq!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .asset_references()[0]
                .asset
                .value(),
            asset.value()
        );
        let loaded = editor.project().snapshot();
        assert!(editor.execute(command()).is_err());
        assert_eq!(editor.project().snapshot(), loaded);
        assert!(editor.undo());
        assert_eq!(editor.project().assets().count(), 0);
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        assert!(editor.undo());
        assert!(
            editor.project().track(track).is_none(),
            "one source+binding command makes one undo entry"
        );
    }
}

#[cfg(test)]
mod replacement_tests {
    use crate::{Command, Created, Editor, Invalid, Project};
    use hex_lattice_contract::{DeviceAssetSlot, DeviceTypeId, NativeDeviceState};
    #[test]
    fn original_replacement_is_one_prepared_identity_preserving_history_edge() {
        let mut editor = Editor::new(Project::new(16000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "sample".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let slot = DeviceAssetSlot::new(1).unwrap();
        editor
            .execute(Command::ImportDeviceAssetIntoNewInstrument {
                track,
                replacing: None,
                path: "old.wav".into(),
                sample_rate: 8000,
                frames: 4,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("test.resource"),
                    1,
                    vec![1],
                )
                .unwrap(),
                source_slot: slot,
            })
            .unwrap();
        let device = editor.project().track(track).unwrap().instrument().unwrap();
        let id = device.id();
        let state = device
            .state()
            .clone()
            .with_input_gain_millidb(3000)
            .unwrap()
            .with_output_gain_millidb(-1000)
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device: id, state })
            .unwrap();
        let before = editor.project().clone();
        let expected = before.device(id).unwrap().state().clone();
        let state = NativeDeviceState::new(expected.type_id(), 1, vec![9])
            .unwrap()
            .with_host_metadata_from(&expected)
            .unwrap();
        let command = || Command::ImportDeviceAssetReplacingInstrument {
            track,
            device: id,
            expected_state: expected.clone(),
            path: "new.wav".into(),
            sample_rate: 12000,
            frames: 6,
            state: state.clone(),
            source_slot: slot,
        };
        let refused = editor.execute_prepared::<(), Invalid>(command(), |candidate| {
            assert_eq!(candidate.assets().count(), 2);
            Err(Invalid("native refused"))
        });
        assert!(refused.is_err());
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_redo());
        for case in 0..3 {
            let mut bad = command();
            if let Command::ImportDeviceAssetReplacingInstrument {
                expected_state,
                state,
                source_slot,
                ..
            } = &mut bad
            {
                match case {
                    0 => {
                        *expected_state =
                            expected_state.clone().with_output_gain_millidb(0).unwrap()
                    }
                    1 => {
                        *state = NativeDeviceState::new(
                            DeviceTypeId::from_static("other.resource"),
                            1,
                            vec![9],
                        )
                        .unwrap()
                    }
                    _ => *source_slot = DeviceAssetSlot::new(2).unwrap(),
                }
            }
            assert!(editor.execute(bad).is_err());
            assert_eq!(editor.project(), &before);
        }
        let Created::Asset(new) = editor.execute(command()).unwrap() else {
            panic!("asset")
        };
        let after = editor.project().clone();
        let device = after.device(id).unwrap();
        assert_eq!(after.assets().count(), 2);
        assert_eq!(device.state().payload(), &[9]);
        assert_eq!(
            (
                device.state().input_gain_millidb(),
                device.state().output_gain_millidb()
            ),
            (3000, -1000)
        );
        assert_eq!(
            device.state().asset_references()[0].asset.value(),
            new.value()
        );
        assert_eq!(
            after.track(track).unwrap().instrument_position(),
            before.track(track).unwrap().instrument_position()
        );
        assert!(editor.undo());
        let mut normalized = before.snapshot();
        normalized.next_id = after.snapshot().next_id;
        assert_eq!(editor.project().snapshot(), normalized);
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
    }
}

#[cfg(test)]
mod first_binding_tests {
    use crate::{Command, Created, Editor, Invalid, Project};
    use hex_lattice_contract::{
        DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
    };
    #[test]
    fn first_source_binding_preserves_existing_device_and_refuses_stale_or_forged_metadata() {
        let mut editor = Editor::new(Project::new(16000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "target".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let empty = NativeDeviceState::new(DeviceTypeId::from_static("test.resource"), 1, vec![0])
            .unwrap()
            .with_input_gain_millidb(3000)
            .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertInstrument {
                track,
                state: empty.clone(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let before = editor.project().clone();
        let template = NativeDeviceState::new(empty.type_id(), 1, vec![1])
            .unwrap()
            .with_host_metadata_from(&empty)
            .unwrap();
        let slot = DeviceAssetSlot::new(1).unwrap();
        let command = || Command::ImportDeviceAssetReplacingInstrument {
            track,
            device,
            expected_state: empty.clone(),
            path: "original.wav".into(),
            sample_rate: 8000,
            frames: 4,
            state: template.clone(),
            source_slot: slot,
        };
        assert!(
            editor
                .execute_prepared::<(), Invalid>(command(), |_| Err(Invalid("prepare refused")))
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        for case in 0..4 {
            let mut bad = command();
            if let Command::ImportDeviceAssetReplacingInstrument {
                expected_state,
                state,
                source_slot,
                ..
            } = &mut bad
            {
                match case {
                    0 => {
                        *expected_state = expected_state
                            .clone()
                            .with_output_gain_millidb(1000)
                            .unwrap()
                    }
                    1 => *state = state.clone().with_input_gain_millidb(0).unwrap(),
                    2 => {
                        *state = state
                            .clone()
                            .with_asset_references(vec![DeviceAssetReference {
                                slot,
                                asset: DeviceAssetKey::new(99).unwrap(),
                            }])
                            .unwrap()
                    }
                    _ => {
                        let references = vec![DeviceAssetReference {
                            slot: DeviceAssetSlot::new(2).unwrap(),
                            asset: DeviceAssetKey::new(99).unwrap(),
                        }];
                        *expected_state = expected_state
                            .clone()
                            .with_asset_references(references.clone())
                            .unwrap();
                        *state = state.clone().with_asset_references(references).unwrap();
                        *source_slot = slot;
                    }
                }
            }
            assert!(editor.execute(bad).is_err());
            assert_eq!(editor.project(), &before);
        }
        let Created::Asset(asset) = editor.execute(command()).unwrap() else {
            panic!("asset")
        };
        let after = editor.project().clone();
        let bound = after.device(device).unwrap();
        assert_eq!(bound.id(), device);
        assert_eq!(bound.state().input_gain_millidb(), 3000);
        assert_eq!(bound.state().payload(), &[1]);
        assert_eq!(
            bound.state().asset_references(),
            &[DeviceAssetReference {
                slot,
                asset: DeviceAssetKey::new(asset.value()).unwrap()
            }]
        );
        assert!(editor.undo());
        assert_eq!(editor.project().device(device).unwrap().state(), &empty);
        assert!(editor.project().assets().next().is_none());
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
    }
}
