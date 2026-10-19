//! Closed, constructor-paired Utility preparation. No registry/provider owner
//! is adopted and no owning Arc escapes this private preparation path.
use crate::{
    AudioStorageError,
    audio_only_inputs::handle,
    owned_shared::{OwnedShared, SharedDeclaration},
};
use hex_lattice::{
    NativeDeviceDefinition, PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::NativeDeviceMetadata;
use std::sync::Arc;

struct Entry {
    device_id: u64,
    state: SharedDeclaration,
    processor: Option<SharedDeclaration>,
    meter: SharedDeclaration,
    payload: (usize, usize),
    handles: [u64; 4],
}
#[derive(Default)]
pub(super) struct UtilityCustody {
    entries: Vec<Entry>,
    construction_overlap: usize,
}
impl UtilityCustody {
    pub(super) fn prepare(
        &mut self,
        devices: &[hex_project::Device],
        track: hex_project::TrackId,
        rate: u32,
    ) -> Result<OwnedShared<[PreparedRuntimeInstance]>, AudioStorageError> {
        if !self.entries.is_empty() {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.entries
            .try_reserve_exact(devices.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(devices.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        self.construction_overlap = prepared
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedRuntimeInstance>())
            .ok_or(AudioStorageError::Overflow)?;
        let mut admission = StereoMatrix::identity();
        for device in devices {
            device
                .state()
                .require_asset_free_processing()
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            if device.state().type_id() != hex_devices::utility::UTILITY_TYPE_ID {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            let definition = &hex_devices::utility::UTILITY_DEFINITION;
            // This exact closed constructor creates a fresh inner payload Arc;
            // caller state handles are never adopted. Preserve both host gains.
            let migrated = definition
                .migrate_state(device.state())
                .and_then(|s| s.with_input_gain_millidb(device.state().input_gain_millidb()))
                .and_then(|s| s.with_output_gain_millidb(device.state().output_gain_millidb()))
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let matrix = if device.bypassed() {
                StereoMatrix::identity()
            } else {
                definition
                    .prepare_effect_matrix(&migrated)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            };
            admission = admission
                .then(matrix)
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            if !device.bypassed() {
                let gain = migrated.input_gain_linear() * migrated.output_gain_linear();
                let trim = StereoMatrix::new([[gain, 0.0], [0.0, gain]])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                admission = admission
                    .then(trim)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            }
            let processor =
                hex_devices::utility::UtilityDefinition::prepare_owned_filter(&migrated, rate)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let (payload_len, strong, weak) = migrated.payload_storage_observation();
            if strong != 1 || weak != 0 {
                return Err(AudioStorageError::UnknownStorage);
            }
            let payload = OwnedShared::<[u8]>::declared_slice_parts(payload_len)
                .ok_or(AudioStorageError::UnknownStorage)?;
            let state = OwnedShared::new_owned(migrated);
            state.parts().ok_or(AudioStorageError::UnknownStorage)?;
            let (configuration, state) = state.into_paired_parts_on_control();
            let (processor, processor_declaration) = if let Some(value) = processor {
                let value = OwnedShared::new_owned(value);
                value.parts().ok_or(AudioStorageError::UnknownStorage)?;
                let (owner, declaration) = value.into_processor_parts_on_control();
                (Some(owner), Some(declaration))
            } else {
                (None, None)
            };
            let meter = OwnedShared::new_owned(hex_lattice::DeviceMeter::default());
            meter.parts().ok_or(AudioStorageError::UnknownStorage)?;
            let (meter, meter_declaration) = meter.into_paired_parts_on_control();
            let handles = [handle()?, handle()?, handle()?, handle()?];
            self.entries.push(Entry {
                device_id: device.id().value(),
                state,
                processor: processor_declaration,
                meter: meter_declaration,
                payload,
                handles,
            });
            prepared.push(PreparedRuntimeInstance {
                meter,
                owner: ProcessorChainOwner::Track(track.value()),
                device_id: device.id().value(),
                device_type: hex_devices::utility::UTILITY_TYPE_ID,
                matrix,
                bypassed: device.bypassed(),
                sample_rate: rate,
                configuration: Some(configuration),
                definition: processor.map(hex_lattice::PreparedProcessorFactory::Legacy),
            });
        }
        Ok(if prepared.is_empty() {
            OwnedShared::new_owned_empty_slice()
        } else {
            OwnedShared::from_owned_vec(prepared)
        })
    }
    fn validate(&self, chain: &[PreparedRuntimeInstance]) -> Result<(), AudioStorageError> {
        if chain.len() != self.entries.len() {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (value, pair) in chain.iter().zip(&self.entries) {
            if value.device_id != pair.device_id
                || value.device_type != hex_devices::utility::UTILITY_TYPE_ID
                || value.definition.is_some() != pair.processor.is_some()
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            let state = value
                .configuration
                .as_ref()
                .ok_or(AudioStorageError::UnknownStorage)?;
            state
                .require_asset_free_processing()
                .map_err(|_| AudioStorageError::UnknownStorage)?;
            let (len, strong, weak) = state.payload_storage_observation();
            if len != pair.payload.0 || strong != 1 || weak != 0 {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        Ok(())
    }
    pub(super) fn bytes(
        &self,
        chain: &[PreparedRuntimeInstance],
    ) -> Result<usize, AudioStorageError> {
        self.validate(chain)?;
        let mut bytes = self
            .entries
            .capacity()
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(AudioStorageError::Overflow)?;
        for (value, pair) in chain.iter().zip(&self.entries) {
            let state = value
                .configuration
                .as_ref()
                .ok_or(AudioStorageError::UnknownStorage)?;
            let mut parts = [
                Some(pair.payload),
                pair.state.parts(state),
                pair.meter.parts(&value.meter),
            ];
            for part in &mut parts {
                let (payload, overhead) = part.take().ok_or(AudioStorageError::UnknownStorage)?;
                bytes = bytes
                    .checked_add(payload)
                    .and_then(|n| n.checked_add(overhead))
                    .ok_or(AudioStorageError::Overflow)?;
            }
            if let Some(processor) = &value.definition {
                let processor = processor
                    .legacy()
                    .ok_or(AudioStorageError::UnknownStorage)?;
                let (payload, overhead) = pair
                    .processor
                    .as_ref()
                    .and_then(|d| d.parts(processor))
                    .ok_or(AudioStorageError::UnknownStorage)?;
                bytes = bytes
                    .checked_add(payload)
                    .and_then(|n| n.checked_add(overhead))
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        Ok(bytes)
    }
    pub(super) fn handles(&self) -> usize {
        self.entries
            .iter()
            .map(|e| 3 + usize::from(e.processor.is_some()))
            .sum()
    }
    pub(super) fn visit_metadata<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        l.vec(&self.entries)
    }
    pub(super) fn construction_overlap(&self) -> usize {
        self.construction_overlap
    }
    pub(super) fn metadata_bytes(&self) -> Result<usize, AudioStorageError> {
        self.entries
            .capacity()
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(AudioStorageError::Overflow)
    }
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.construction_overlap = 0;
    }
    pub(super) fn visit<'a>(
        &'a self,
        chain: &'a Arc<[PreparedRuntimeInstance]>,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        self.validate(chain)?;
        for (value, pair) in chain.iter().zip(&self.entries) {
            let state = value
                .configuration
                .as_ref()
                .ok_or(AudioStorageError::UnknownStorage)?;
            l.shared(
                state,
                pair.state.parts(state),
                Owner::exclusive(state),
                pair.handles[0],
                None,
            )?;
            l.bytes(
                Owner::device_payload(state),
                Location("native-state-payload", 0),
                pair.payload
                    .0
                    .checked_add(pair.payload.1)
                    .ok_or(AudioStorageError::Overflow)?,
            )?;
            l.handle(
                Owner::device_payload(state),
                Owner::exclusive(state.as_ref()),
                pair.handles[1],
            )?;
            l.shared(
                &value.meter,
                pair.meter.parts(&value.meter),
                Owner::exclusive(&value.meter),
                pair.handles[2],
                None,
            )?;
            if let Some(processor) = &value.definition {
                let processor = processor
                    .legacy()
                    .ok_or(AudioStorageError::UnknownStorage)?;
                l.shared(
                    processor,
                    pair.processor.as_ref().and_then(|d| d.parts(processor)),
                    Owner::exclusive(processor),
                    pair.handles[3],
                    None,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::audio_preparation::*;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    #[test]
    fn sealed_utility_graph_matches_canonical_pcm_reports_nested_payload_and_recovers_original_bank()
     {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let (project, mut bank) = crate::audio_preparation::tests::fixture(2);
        bank.reservation.handle_obligations = 32;
        let original = bank.sources.as_ptr();
        let original_capacity = bank.sources.capacity();
        let mut snapshot = project.snapshot();
        snapshot.tracks[0].gain = 0.5;
        snapshot.tracks[0].balance = -0.25;
        let definition = &hex_devices::utility::UTILITY_DEFINITION;
        let state = definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                hex_devices::utility::parameter::DC_REMOVE,
                ParameterValue::Bool(true),
            )
            .unwrap()
            .with_input_gain_millidb(-6000)
            .unwrap()
            .with_output_gain_millidb(3000)
            .unwrap();
        snapshot.tracks[0]
            .devices
            .push(hex_project::DeviceSnapshot {
                comparison: None,
                id: snapshot.next_id,
                bypassed: false,
                state,
            });
        snapshot.next_id += 1;
        let project = hex_project::Project::from_snapshot(snapshot).unwrap();
        let mut media = std::collections::BTreeMap::new();
        let (asset, source) = &bank.sources[0];
        let SourceKind::Resident(audio) = &source.kind else {
            panic!("resident fixture")
        };
        media.insert(*asset, audio.clone());
        let reference = crate::PreparedRevision::prepare(
            &project,
            &media,
            std::sync::Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap();
        let mut reference = crate::SessionPlayback::new(std::sync::Arc::new(reference)).unwrap();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                AudioArrangementInput::new(&project, 11, bank).with_audio_prepared_revision(),
            )
            .unwrap_or_else(|refusal| panic!("Utility root preflight: {:?}", refusal.reason));
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: 1024,
                handle_rows: 256,
                root_rows: 8,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            },
        )
        .unwrap();
        ledger
            .borrow_custody_on_control(&custody, crate::AudioStorageRole::Current)
            .unwrap();
        let summary = ledger.summary().unwrap();
        assert_eq!(summary.unknown_storage, 0);
        assert!(summary.require_complete().is_err());
        drop(ledger);
        let mut owner = crate::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        reference
            .apply(crate::SessionPlaybackCommand::Play)
            .unwrap();
        let mut actual = [[0.0; 2]; 256];
        let mut expected = actual;
        owner.test_render(&mut actual).unwrap();
        reference.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        assert!(actual[16..].iter().any(|f| f[0] != 0.0));
        owner.dispose_on_control().unwrap();
        let inputs = owner.recover_inputs_on_control().unwrap();
        assert_eq!(inputs.sources.as_ptr(), original);
        assert_eq!(inputs.sources.capacity(), original_capacity);
    }
}

#[cfg(test)]
mod asset_refusal_tests {
    use super::*;
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn closed_utility_paths_refuse_asset_metadata_before_constructor_or_migration() {
        for bypassed in [false, true] {
            let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: "audio".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor.execute(Command::RemoveInstrument { track }).unwrap();
            let Created::Asset(asset) = editor
                .execute(Command::AddAsset {
                    path: "Original.WAV".into(),
                    sample_rate: 48000,
                    frames: 8,
                })
                .unwrap()
            else {
                panic!("asset")
            };
            let state = hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap()
                .with_asset_references(vec![DeviceAssetReference {
                    slot: DeviceAssetSlot::new(1).unwrap(),
                    asset: DeviceAssetKey::new(asset.value()).unwrap(),
                }])
                .unwrap();
            let Created::Device(device) = editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state,
                })
                .unwrap()
            else {
                panic!("device")
            };
            editor
                .execute(Command::SetDeviceBypass { device, bypassed })
                .unwrap();
            assert_eq!(
                crate::audio_preparation::validate_constructor_project(editor.project()),
                Err(AudioStorageError::UnsupportedGraph)
            );
            let mut custody = UtilityCustody::default();
            assert!(matches!(
                custody.prepare(
                    editor.project().track(track).unwrap().devices(),
                    track,
                    48000
                ),
                Err(AudioStorageError::UnsupportedGraph)
            ));
            assert!(custody.entries.is_empty());
        }
    }
}
