//! Closed Utility/reverb/foreign chain metadata. No registry/provider owner
//! is adopted and no owning Arc escapes this private preparation path.
use crate::{
    AudioStorageError,
    audio_only_inputs::handle,
    owned_shared::{OwnedShared, SharedDeclaration},
};
use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, PreparedForeignProcessor,
    PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner,
    ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::NativeDeviceMetadata;
use std::sync::Arc;

struct Entry {
    automation_bytes: usize,
    device_id: u64,
    owner: ProcessorChainOwner,
    device_type: hex_lattice_contract::DeviceTypeId,
    foreign: Option<PreparedForeignProcessor>,
    state: SharedDeclaration,
    processor: Option<SharedDeclaration>,
    approval: Option<SharedDeclaration>,
    meter: SharedDeclaration,
    payload: (usize, usize),
    handles: [u64; 6],
}
#[derive(Default)]
pub(in crate::audio_preparation) struct ChainCustody {
    entries: Vec<Entry>,
    construction_overlap: usize,
}
impl ChainCustody {
    /// Quotes only this closed constructor's DATA and transient Vec overlap.
    /// No foreign provider is invoked and no source/worker claim is produced.
    pub(in crate::audio_preparation) fn quote(
        &self,
        devices: &[hex_project::Device],
        rate: u32,
    ) -> Result<(usize, usize), AudioStorageError> {
        let parts = |parts: Option<(usize, usize)>| -> Result<usize, AudioStorageError> {
            let (p, o) = parts.ok_or(AudioStorageError::UnsupportedConstructor)?;
            p.checked_add(o).ok_or(AudioStorageError::Overflow)
        };
        let mut retained = self
            .entries
            .capacity()
            .max(devices.len())
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(AudioStorageError::Overflow)?;
        retained = retained
            .checked_add(parts(
                OwnedShared::<[PreparedRuntimeInstance]>::declared_slice_parts(devices.len()),
            )?)
            .ok_or(AudioStorageError::Overflow)?;
        let mut largest_copy = 0;
        for device in devices {
            retained = retained
                .checked_add(crate::device_automation::quote(device)?)
                .ok_or(AudioStorageError::Overflow)?;
            let state = device.state();
            state
                .require_asset_free_processing()
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let (payload, processor) = if state.type_id() == hex_devices::utility::UTILITY_TYPE_ID {
                let migrated = hex_devices::utility::UTILITY_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                let processor = if device.automation().is_empty() {
                    hex_devices::utility::UtilityDefinition::prepare_owned_filter(&migrated, rate)
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?
                        .is_some()
                } else {
                    hex_devices::utility::UtilityDefinition::prepare_owned_automated_filter(
                        &migrated, rate,
                    )
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                    true
                };
                let bytes = processor
                    .then(|| {
                        parts(OwnedShared::<
                            hex_devices::utility::PreparedUtilityFilterRuntime,
                        >::declared_parts())
                    })
                    .transpose()?;
                (migrated.payload().len(), bytes)
            } else if state.type_id() == hex_devices::saturator::SATURATOR_TYPE_ID {
                let migrated = hex_devices::saturator::SATURATOR_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                hex_devices::saturator::SaturatorDefinition::prepare_owned(&migrated, rate, &[])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                largest_copy = largest_copy.max(migrated.payload().len());
                (
                    migrated.payload().len(),
                    Some(parts(OwnedShared::<
                        hex_devices::saturator::PreparedSaturator,
                    >::declared_parts())?),
                )
            } else if state.type_id() == hex_devices::reverb::REVERB_TYPE_ID {
                let migrated = hex_devices::reverb::REVERB_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                let _ = hex_devices::reverb::ReverbDefinition::prepare_owned_room(&migrated, rate)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                largest_copy = largest_copy.max(migrated.payload().len());
                (
                    migrated.payload().len(),
                    Some(parts(
                        OwnedShared::<hex_devices::reverb::PreparedReverb>::declared_parts(),
                    )?),
                )
            } else {
                largest_copy = largest_copy.max(state.payload().len());
                (state.payload().len(), None)
            };
            for bytes in [
                parts(OwnedShared::<hex_lattice_contract::NativeDeviceState>::declared_parts())?,
                parts(OwnedShared::<[u8]>::declared_slice_parts(payload))?,
                parts(OwnedShared::<hex_lattice::DeviceMeter>::declared_parts())?,
            ] {
                retained = retained
                    .checked_add(bytes)
                    .ok_or(AudioStorageError::Overflow)?;
            }
            if state.type_id() != hex_devices::utility::UTILITY_TYPE_ID
                && state.type_id() != hex_devices::saturator::SATURATOR_TYPE_ID
                && state.type_id() != hex_devices::reverb::REVERB_TYPE_ID
            {
                let approval_bytes =
                    parts(OwnedShared::<hex_lattice::ForeignStateApproval>::declared_parts())?;
                retained = retained
                    .checked_add(approval_bytes)
                    .ok_or(AudioStorageError::Overflow)?;
                // Provider's temporary immutable Arc overlaps the fresh graph copy.
                largest_copy = largest_copy.max(approval_bytes);
            }
            if let Some(bytes) = processor {
                retained = retained
                    .checked_add(bytes)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        let automation_overlap = devices
            .iter()
            .try_fold(0usize, |sum, device| {
                sum.checked_add(crate::device_automation::quote(device).ok()?)
            })
            .ok_or(AudioStorageError::Overflow)?;
        let automation_staging = devices
            .iter()
            .map(|device| {
                hex_lattice::PreparedParameterAutomation::quoted_construction_overlap_bytes(
                    device.automation().len(),
                )
            })
            .try_fold(0usize, |largest, value| Some(largest.max(value?)))
            .ok_or(AudioStorageError::Overflow)?;
        let overlap = devices
            .len()
            .checked_mul(std::mem::size_of::<PreparedRuntimeInstance>())
            .and_then(|n| n.checked_add(largest_copy))
            .and_then(|n| n.checked_add(automation_overlap))
            .and_then(|n| n.checked_add(automation_staging))
            .ok_or(AudioStorageError::Overflow)?;
        Ok((retained, overlap))
    }

    #[cfg(test)]
    pub(in crate::audio_preparation) fn prepare(
        &mut self,
        devices: &[hex_project::Device],
        track: hex_project::TrackId,
        rate: u32,
    ) -> Result<OwnedShared<[PreparedRuntimeInstance]>, AudioStorageError> {
        self.prepare_with_effects(
            devices,
            ProcessorChainOwner::Track(track.value()),
            rate,
            None,
        )
    }
    pub(in crate::audio_preparation) fn prepare_with_effects(
        &mut self,
        devices: &[hex_project::Device],
        owner: ProcessorChainOwner,
        rate: u32,
        effects: Option<&NativeDeviceRegistry<'_>>,
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
        let automation_staging = devices
            .iter()
            .map(|device| {
                hex_lattice::PreparedParameterAutomation::quoted_construction_overlap_bytes(
                    device.automation().len(),
                )
            })
            .try_fold(0usize, |largest, value| Some(largest.max(value?)))
            .ok_or(AudioStorageError::Overflow)?;
        let mut copy_overlap = 0usize;
        for device in devices {
            let ty = device.state().type_id();
            if ty == hex_devices::reverb::REVERB_TYPE_ID {
                copy_overlap = copy_overlap.max(hex_devices::reverb::PARAMETER_COUNT * 4);
            } else if ty != hex_devices::utility::UTILITY_TYPE_ID {
                copy_overlap = copy_overlap.max(device.state().payload().len());
                if ty != hex_devices::saturator::SATURATOR_TYPE_ID {
                    let (payload, overhead) =
                        OwnedShared::<hex_lattice::ForeignStateApproval>::declared_parts()
                            .ok_or(AudioStorageError::UnknownStorage)?;
                    copy_overlap = copy_overlap.max(
                        payload
                            .checked_add(overhead)
                            .ok_or(AudioStorageError::Overflow)?,
                    );
                }
            }
        }
        self.construction_overlap = prepared
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedRuntimeInstance>())
            .and_then(|bytes| bytes.checked_add(copy_overlap))
            .and_then(|bytes| bytes.checked_add(automation_staging))
            .ok_or(AudioStorageError::Overflow)?;
        if prepared.capacity() != devices.len() {
            return Err(AudioStorageError::UnknownStorage);
        }
        let mut admission = StereoMatrix::identity();
        for device in devices {
            let automation = crate::device_automation::prepare(device)
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let automation_bytes = automation
                .storage_bytes()
                .ok_or(AudioStorageError::Overflow)?;
            device
                .state()
                .require_asset_free_processing()
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let utility = device.state().type_id() == hex_devices::utility::UTILITY_TYPE_ID;
            let reverb = device.state().type_id() == hex_devices::reverb::REVERB_TYPE_ID;
            let saturator = device.state().type_id() == hex_devices::saturator::SATURATOR_TYPE_ID;
            let native = utility || reverb || saturator;
            let definition = &hex_devices::utility::UTILITY_DEFINITION;
            // Never adopt a provider's state/processor owner. The Utility,
            // Reverb and Saturator constructors are closed; foreign factories return scalar facts
            // only, matched later to the audio owner's original source.
            let foreign_definition = if native {
                None
            } else {
                Some(
                    effects
                        .and_then(|r| r.definition(device.state().type_id()))
                        .ok_or(AudioStorageError::UnsupportedGraph)?,
                )
            };
            let migrated = if saturator {
                hex_devices::saturator::SATURATOR_DEFINITION
                    .migrate_state(device.state())
                    .and_then(|state| state.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else if utility {
                definition
                    .migrate_state(device.state())
                    .and_then(|s| s.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else if reverb {
                hex_devices::reverb::REVERB_DEFINITION
                    .migrate_state(device.state())
                    .and_then(|s| s.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else {
                copy_state(device.state())?
            };
            let (foreign, foreign_approval) = if native {
                (None, None)
            } else {
                let definition = foreign_definition.ok_or(AudioStorageError::UnsupportedGraph)?;
                let tuning = hex_lattice::TuningTable::default();
                let context = hex_lattice::DevicePreparationContext {
                    sample_rate: rate,
                    tuning: &tuning,
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                };
                let identity = ProcessorInstanceIdentity::new(owner, device.id().value())
                    .map_err(|_| AudioStorageError::SourceMismatch)?;
                match definition
                    .prepare_instance_runtime_factory(identity, &migrated, &context)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
                {
                    Some(factory)
                        if factory
                            .foreign_processor()
                            .is_some_and(|facts| facts.binding().is_some_and(|b| b.is_valid())) =>
                    {
                        let facts = factory
                            .foreign_processor()
                            .ok_or(AudioStorageError::UnsupportedGraph)?;
                        let approval = factory
                            .foreign_state_approval()
                            .map(|approval| {
                                if facts.binding() != Some(approval.binding())
                                    || !approval.matches_state(&migrated)
                                {
                                    return Err(AudioStorageError::SourceMismatch);
                                }
                                // Fresh declared immutable allocation. Never adopt a provider Arc.
                                let copied = OwnedShared::new_owned(approval.as_ref().clone());
                                copied.parts().ok_or(AudioStorageError::UnknownStorage)?;
                                Ok(copied.into_paired_parts_on_control())
                            })
                            .transpose()?;
                        (Some(facts), approval)
                    }
                    _ => return Err(AudioStorageError::UnsupportedGraph),
                }
            };
            let matrix = if device.bypassed() || !native || !automation.is_empty() || saturator {
                StereoMatrix::identity()
            } else if reverb {
                hex_devices::reverb::REVERB_DEFINITION
                    .prepare_effect_matrix(&migrated)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
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
            let processor = if saturator {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::saturator::SaturatorDefinition::prepare_owned(
                            &migrated,
                            rate,
                            automation.parameter_ids(),
                        )
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else if utility {
                (if automation.is_empty() {
                    hex_devices::utility::UtilityDefinition::prepare_owned_filter(&migrated, rate)
                } else {
                    hex_devices::utility::UtilityDefinition::prepare_owned_automated_filter_for_parameters(
                        &migrated, rate, automation.parameter_ids(),
                    )
                    .map(Some)
                })
                .map_err(|_| AudioStorageError::UnsupportedGraph)?
                .map(|value| OwnedShared::new_owned(value).into_processor_parts_on_control())
            } else if reverb {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::reverb::ReverbDefinition::prepare_owned_room(&migrated, rate)
                            .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else {
                None
            };
            let (payload_len, strong, weak) = migrated.payload_storage_observation();
            if strong != 1 || weak != 0 {
                return Err(AudioStorageError::UnknownStorage);
            }
            let payload = OwnedShared::<[u8]>::declared_slice_parts(payload_len)
                .ok_or(AudioStorageError::UnknownStorage)?;
            let migrated_type = migrated.type_id();
            let state = OwnedShared::new_owned(migrated);
            state.parts().ok_or(AudioStorageError::UnknownStorage)?;
            let (configuration, state) = state.into_paired_parts_on_control();
            let (processor, processor_declaration) = if let Some((owner, declaration)) = processor {
                declaration
                    .parts(&owner)
                    .ok_or(AudioStorageError::UnknownStorage)?;
                (Some(owner), Some(declaration))
            } else {
                (None, None)
            };
            let meter = OwnedShared::new_owned(hex_lattice::DeviceMeter::default());
            meter.parts().ok_or(AudioStorageError::UnknownStorage)?;
            let (meter, meter_declaration) = meter.into_paired_parts_on_control();
            let handles = [
                handle()?,
                handle()?,
                handle()?,
                handle()?,
                handle()?,
                handle()?,
            ];
            let (approval, approval_declaration) = foreign_approval
                .map_or((None, None), |(approval, declaration)| {
                    (Some(approval), Some(declaration))
                });
            self.entries.push(Entry {
                automation_bytes,
                device_id: device.id().value(),
                owner,
                device_type: migrated_type,
                foreign,
                state,
                processor: processor_declaration,
                approval: approval_declaration,
                meter: meter_declaration,
                payload,
                handles,
            });
            prepared.push(PreparedRuntimeInstance {
                automation,
                meter,
                owner,
                device_id: device.id().value(),
                device_type: migrated_type,
                matrix,
                bypassed: device.bypassed(),
                sample_rate: rate,
                configuration: Some(configuration),
                definition: foreign
                    .map(|processor| match approval {
                        Some(approval) => PreparedProcessorFactory::ForeignApproved {
                            processor,
                            approval,
                        },
                        None => PreparedProcessorFactory::Foreign(processor),
                    })
                    .or_else(|| processor.map(PreparedProcessorFactory::Legacy)),
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
            if value.automation.storage_bytes() != Some(pair.automation_bytes)
                || value.device_id != pair.device_id
                || value.owner != pair.owner
                || value.device_type != pair.device_type
                || match (&value.definition, pair.foreign, pair.processor.is_some()) {
                    (Some(factory), Some(expected), false) => {
                        factory.foreign_processor() != Some(expected)
                            || factory.foreign_state_approval().is_some() != pair.approval.is_some()
                            || factory.foreign_state_approval().is_some_and(|approval| {
                                expected.binding() != Some(approval.binding())
                                    || value
                                        .configuration
                                        .as_deref()
                                        .is_none_or(|state| !approval.matches_state(state))
                            })
                    }
                    (Some(PreparedProcessorFactory::Legacy(_)), None, true)
                    | (None, None, false) => false,
                    _ => true,
                }
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
    pub(in crate::audio_preparation) fn bytes(
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
            bytes = bytes
                .checked_add(pair.automation_bytes)
                .ok_or(AudioStorageError::Overflow)?;
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
            if let Some(approval) = value
                .definition
                .as_ref()
                .and_then(PreparedProcessorFactory::foreign_state_approval)
            {
                let (payload, overhead) = pair
                    .approval
                    .as_ref()
                    .and_then(|declaration| declaration.parts(approval))
                    .ok_or(AudioStorageError::UnknownStorage)?;
                bytes = bytes
                    .checked_add(payload)
                    .and_then(|n| n.checked_add(overhead))
                    .ok_or(AudioStorageError::Overflow)?;
            }
            if let Some(PreparedProcessorFactory::Legacy(processor)) = &value.definition {
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
    pub(in crate::audio_preparation) fn handles(&self) -> usize {
        self.entries
            .iter()
            .map(|e| 3 + usize::from(e.processor.is_some()) + 2 * usize::from(e.approval.is_some()))
            .sum()
    }
    pub(in crate::audio_preparation) fn visit_metadata<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        l.vec(&self.entries)
    }
    pub(in crate::audio_preparation) fn construction_overlap(&self) -> usize {
        self.construction_overlap
    }
    pub(in crate::audio_preparation) fn metadata_bytes(&self) -> Result<usize, AudioStorageError> {
        self.entries
            .capacity()
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(AudioStorageError::Overflow)
    }
    pub(in crate::audio_preparation) fn clear(&mut self) {
        self.entries.clear();
        self.construction_overlap = 0;
    }
    pub(in crate::audio_preparation) fn visit<'a>(
        &'a self,
        chain: &'a Arc<[PreparedRuntimeInstance]>,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        self.validate(chain)?;
        for (value, pair) in chain.iter().zip(&self.entries) {
            let (ids, lanes, points) = value.automation.storage_vectors();
            l.vec(ids)?;
            l.vec(lanes)?;
            l.vec(points)?;
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
            if let Some(approval) = value
                .definition
                .as_ref()
                .and_then(PreparedProcessorFactory::foreign_state_approval)
            {
                l.shared(
                    approval,
                    pair.approval
                        .as_ref()
                        .and_then(|declaration| declaration.parts(approval)),
                    Owner::exclusive(approval),
                    pair.handles[4],
                    None,
                )?;
                // One weak alias in the immutable approval, not source DATA or custody.
                l.foreign_source_weak(approval.as_ref(), pair.handles[5])?;
            }
            if let Some(PreparedProcessorFactory::Legacy(processor)) = &value.definition {
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

/// Fresh exact constructor payload; no caller/provider Arc enters the ledger.
fn copy_state(
    state: &hex_lattice_contract::NativeDeviceState,
) -> Result<hex_lattice_contract::NativeDeviceState, AudioStorageError> {
    state
        .require_asset_free_processing()
        .map_err(|_| AudioStorageError::UnsupportedGraph)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(state.payload().len())
        .map_err(|_| AudioStorageError::Allocation)?;
    if payload.capacity() != state.payload().len() {
        return Err(AudioStorageError::UnknownStorage);
    }
    payload.extend_from_slice(state.payload());
    hex_lattice_contract::NativeDeviceState::new(state.type_id(), state.state_version(), payload)
        .and_then(|s| s.with_host_metadata_from(state))
        .map_err(|_| AudioStorageError::UnsupportedGraph)
}

#[cfg(test)]
mod tests {
    use crate::audio_preparation::*;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    #[test]
    fn sealed_native_graphs_match_canonical_pcm_report_nested_payload_and_recover_original_bank() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        for saturator in [false, true] {
            let mut static_bytes = None;
            for automated in [false, true] {
                let (project, mut bank) = crate::audio_preparation::tests::fixture(2);
                bank.reservation.handle_obligations = 32;
                let original = bank.sources.as_ptr();
                let original_capacity = bank.sources.capacity();
                let mut snapshot = project.snapshot();
                snapshot.tracks[0].gain = 0.5;
                snapshot.tracks[0].balance = -0.25;
                let state = if saturator {
                    let definition = &hex_devices::saturator::SATURATOR_DEFINITION;
                    definition
                        .patch_parameter(
                            &definition.default_state().unwrap(),
                            hex_devices::saturator::parameter::DRIVE,
                            ParameterValue::Float(12.0),
                        )
                        .unwrap()
                } else {
                    let definition = &hex_devices::utility::UTILITY_DEFINITION;
                    definition
                        .patch_parameter(
                            &definition.default_state().unwrap(),
                            hex_devices::utility::parameter::DC_REMOVE,
                            ParameterValue::Bool(true),
                        )
                        .unwrap()
                }
                .with_input_gain_millidb(-6000)
                .unwrap()
                .with_output_gain_millidb(3000)
                .unwrap();
                snapshot.tracks[0]
                    .devices
                    .push(hex_project::DeviceSnapshot {
                        automation: if automated {
                            vec![hex_project::DeviceAutomation {
                                parameter: if saturator {
                                    hex_devices::saturator::parameter::DRIVE
                                } else {
                                    hex_devices::utility::parameter::GAIN
                                },
                                points: vec![
                                    hex_project::AutomationPoint {
                                        beat: hex_project::BeatTime::ZERO,
                                        value: hex_project::AutomationValue::new(if saturator {
                                            0.5
                                        } else {
                                            0.005
                                        })
                                        .unwrap(),
                                        outgoing: hex_project::AutomationInterpolation::Linear,
                                    },
                                    hex_project::AutomationPoint {
                                        beat: hex_project::BeatTime::from_raw(1 << 26),
                                        value: hex_project::AutomationValue::new(if saturator {
                                            0.75
                                        } else {
                                            0.015
                                        })
                                        .unwrap(),
                                        outgoing: hex_project::AutomationInterpolation::Step,
                                    },
                                ],
                            }]
                        } else {
                            Vec::new()
                        },
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
                let mut reference =
                    crate::SessionPlayback::new(std::sync::Arc::new(reference)).unwrap();
                let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
                let registry = hex_devices::native_registry().unwrap();
                let input =
                    AudioArrangementInput::new(&project, 11, bank).with_audio_prepared_revision();
                let input = if saturator {
                    input.with_effect_registry(&registry)
                } else {
                    input
                };
                let token = custody
                    .begin_input_on_control(input)
                    .unwrap_or_else(|refusal| {
                        panic!("native root preflight: {:?}", refusal.reason)
                    });
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
                if automated {
                    let program_bytes =
                        crate::device_automation::quote(&project.tracks()[0].devices()[0]).unwrap();
                    assert!(
                        summary.data_bytes >= static_bytes.unwrap() + program_bytes,
                        "closed program backing is charged in physical inventory"
                    );
                } else {
                    static_bytes = Some(summary.data_bytes);
                }
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
                if saturator {
                    assert!(actual[..16].iter().any(|frame| frame[0] != 0.0));
                    assert!(
                        actual[16..].iter().all(|frame| *frame == [0.0; 2]),
                        "memoryless Saturator has no output tail"
                    );
                } else {
                    assert!(actual[16..].iter().any(|f| f[0] != 0.0));
                }
                owner.dispose_on_control().unwrap();
                let inputs = owner.recover_inputs_on_control().unwrap();
                assert_eq!(inputs.sources.as_ptr(), original);
                assert_eq!(inputs.sources.capacity(), original_capacity);
            }
        }
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
                .execute(Command::InsertTrackWithDevice {
                    name: "audio".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state()
                        .expect("fixture instrument state"),
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
            let mut custody = ChainCustody::default();
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

#[cfg(test)]
#[path = "chains/foreign_tests.rs"]
mod foreign_tests;
