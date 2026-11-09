//! Closed Utility/reverb/Saturator/Auto Pan/foreign chain metadata. No registry/provider owner
//! is adopted and no owning Arc escapes this private preparation path.
use crate::{
    AudioStorageError,
    audio_only_inputs::handle,
    device_modulation::EffectModulation,
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
    modulation_bytes: Option<usize>,
    authored: bool,
    device_id: u64,
    owner: ProcessorChainOwner,
    device_type: hex_lattice_contract::DeviceTypeId,
    foreign: Option<PreparedForeignProcessor>,
    state: SharedDeclaration,
    processor: Option<SharedDeclaration>,
    meter: SharedDeclaration,
    payload: (usize, usize),
    handles: [u64; 4],
}
#[derive(Default)]
pub(in crate::audio_preparation) struct ChainCustody {
    entries: Vec<Entry>,
    construction_overlap: usize,
}
impl ChainCustody {
    /// Quotes only this closed constructor's DATA and transient Vec overlap.
    /// No foreign provider is invoked and no source/worker claim is produced.
    pub(in crate::audio_preparation) fn quote_with_input(
        &self,
        devices: &[hex_project::Device],
        rate: u32,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
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
        let mut modulation_overlap = 0;
        for device in devices {
            let (modulation, staging) =
                EffectModulation::quote(device).ok_or(AudioStorageError::Overflow)?;
            retained = retained
                .checked_add(modulation)
                .ok_or(AudioStorageError::Overflow)?;
            modulation_overlap = modulation_overlap.max(staging);
            retained = retained
                .checked_add(crate::device_automation::quote_with_input(
                    device,
                    native_input,
                )?)
                .ok_or(AudioStorageError::Overflow)?;
            let state = device.state();
            state
                .require_asset_free_processing()
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let (payload, processor) = if state.type_id() == hex_devices::utility::UTILITY_TYPE_ID {
                let migrated = hex_devices::utility::UTILITY_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                let processor = if !device.modulation().routes().iter().any(|route| {
                    matches!(
                        route.destination, hex_project::ModulationDestination::Device { target, .. }
                            if target.provider_parameter().is_some()
                    )
                }) && !device
                    .automation()
                    .iter()
                    .any(|lane| lane.target.provider_parameter().is_some())
                    && !native_input.is_some_and(|input| {
                        input.device() == device.id()
                            && input.target().provider_parameter().is_some()
                    }) {
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
            } else if state.type_id() == hex_devices::overdrive::OVERDRIVE_TYPE_ID {
                let migrated = hex_devices::overdrive::OVERDRIVE_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                hex_devices::overdrive::OverdriveDefinition::prepare_owned(&migrated, rate, &[])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                largest_copy = largest_copy.max(migrated.payload().len());
                (
                    migrated.payload().len(),
                    Some(parts(OwnedShared::<
                        hex_devices::overdrive::PreparedOverdrive,
                    >::declared_parts())?),
                )
            } else if state.type_id() == hex_devices::eq8::EQ8_TYPE_ID {
                let migrated = hex_devices::eq8::EQ8_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                hex_devices::eq8::Eq8Definition::prepare_owned(&migrated, rate, &[])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                largest_copy = largest_copy.max(migrated.payload().len());
                (
                    migrated.payload().len(),
                    Some(parts(
                        OwnedShared::<hex_devices::eq8::PreparedEq8>::declared_parts(),
                    )?),
                )
            } else if state.type_id() == hex_devices::rattle::RATTLE_TYPE_ID {
                let migrated = hex_devices::rattle::RATTLE_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                hex_devices::rattle::RattleDefinition::prepare_owned(&migrated, rate, &[])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                largest_copy = largest_copy.max(migrated.payload().len());
                (
                    migrated.payload().len(),
                    Some(parts(
                        OwnedShared::<hex_devices::rattle::PreparedRattle>::declared_parts(),
                    )?),
                )
            } else if state.type_id() == hex_devices::compressor::COMPRESSOR_TYPE_ID {
                let migrated = hex_devices::compressor::COMPRESSOR_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                hex_devices::compressor::CompressorDefinition::prepare_owned(&migrated, rate, &[])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                largest_copy = largest_copy.max(migrated.payload().len());
                (
                    migrated.payload().len(),
                    Some(parts(OwnedShared::<
                        hex_devices::compressor::PreparedCompressor,
                    >::declared_parts())?),
                )
            } else if state.type_id() == hex_devices::gate::GATE_TYPE_ID {
                let migrated = hex_devices::gate::GATE_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                hex_devices::gate::GateDefinition::prepare_owned(&migrated, rate, &[])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                largest_copy = largest_copy.max(migrated.payload().len());
                (
                    migrated.payload().len(),
                    Some(parts(
                        OwnedShared::<hex_devices::gate::PreparedGate>::declared_parts(),
                    )?),
                )
            } else if state.type_id() == hex_devices::auto_pan::AUTO_PAN_TYPE_ID {
                let migrated = hex_devices::auto_pan::AUTO_PAN_DEFINITION
                    .migrate_state(state)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                hex_devices::auto_pan::AutoPanDefinition::prepare_owned(&migrated, rate, &[])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                largest_copy = largest_copy.max(migrated.payload().len());
                (
                    migrated.payload().len(),
                    Some(parts(
                        OwnedShared::<hex_devices::auto_pan::PreparedAutoPan>::declared_parts(),
                    )?),
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
            if let Some(bytes) = processor {
                retained = retained
                    .checked_add(bytes)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        let automation_overlap = devices
            .iter()
            .try_fold(0usize, |sum, device| {
                sum.checked_add(
                    crate::device_automation::quote_with_input(device, native_input).ok()?,
                )
            })
            .ok_or(AudioStorageError::Overflow)?;
        let automation_staging = devices
            .iter()
            .map(|device| {
                let provider = device
                    .automation().iter()
                    .filter(|lane| !lane.target.is_host())
                    .count();
                hex_lattice::PreparedParameterAutomation::quoted_construction_overlap_bytes_with_host(
                    provider,
                    device.automation().len().checked_sub(provider)?,
                    device
                        .reading_automation()
                        .try_fold(0usize, |sum, lane| sum.checked_add(lane.point_count()))?,
                ).and_then(|bytes| bytes.checked_add(crate::device_automation::input_construction_growth_overlap(device, native_input).ok()?))
            })
            .try_fold(0usize, |largest, value| Some(largest.max(value?)))
            .ok_or(AudioStorageError::Overflow)?;
        let overlap = devices
            .len()
            .checked_mul(std::mem::size_of::<PreparedRuntimeInstance>())
            .and_then(|n| n.checked_add(largest_copy))
            .and_then(|n| n.checked_add(automation_overlap))
            .and_then(|n| n.checked_add(automation_staging))
            .and_then(|n| n.checked_add(modulation_overlap))
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
    #[cfg(test)]
    pub(in crate::audio_preparation) fn prepare_with_effects(
        &mut self,
        devices: &[hex_project::Device],
        owner: ProcessorChainOwner,
        rate: u32,
        effects: Option<&NativeDeviceRegistry<'_>>,
    ) -> Result<OwnedShared<[PreparedRuntimeInstance]>, AudioStorageError> {
        self.prepare_with_effects_and_input(devices, owner, rate, effects, None, None)
    }
    pub(in crate::audio_preparation) fn prepare_with_effects_and_input(
        &mut self,
        devices: &[hex_project::Device],
        owner: ProcessorChainOwner,
        rate: u32,
        effects: Option<&NativeDeviceRegistry<'_>>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
        project: Option<&hex_project::Project>,
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
                let provider = device
                    .automation().iter()
                    .filter(|lane| !lane.target.is_host())
                    .count();
                hex_lattice::PreparedParameterAutomation::quoted_construction_overlap_bytes_with_host(
                    provider,
                    device.automation().len().checked_sub(provider)?,
                    device
                        .reading_automation()
                        .try_fold(0usize, |sum, lane| sum.checked_add(lane.point_count()))?,
                ).and_then(|bytes| bytes.checked_add(crate::device_automation::input_construction_growth_overlap(device, native_input).ok()?))
            })
            .try_fold(0usize, |largest, value| Some(largest.max(value?)))
            .ok_or(AudioStorageError::Overflow)?;
        self.construction_overlap = prepared
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedRuntimeInstance>())
            .and_then(|n| {
                n.checked_add(
                    devices
                        .iter()
                        .filter(|d| d.state().type_id() != hex_devices::utility::UTILITY_TYPE_ID)
                        .map(|d| {
                            if d.state().type_id() == hex_devices::reverb::REVERB_TYPE_ID {
                                hex_devices::reverb::PARAMETER_COUNT * 4
                            } else {
                                d.state().payload().len()
                            }
                        })
                        .max()
                        .unwrap_or(0),
                )
            })
            .ok_or(AudioStorageError::Overflow)?;
        let modulation_overlap = devices.iter().try_fold(0usize, |largest, device| {
            let (_, staging) =
                EffectModulation::quote(device).ok_or(AudioStorageError::Overflow)?;
            Ok(largest.max(staging))
        })?;
        self.construction_overlap = self
            .construction_overlap
            .checked_add(modulation_overlap)
            .and_then(|bytes| bytes.checked_add(automation_staging))
            .ok_or(AudioStorageError::Overflow)?;
        if prepared.capacity() != devices.len() {
            return Err(AudioStorageError::UnknownStorage);
        }
        let mut admission = StereoMatrix::identity();
        for entry in crate::serial_effect_power::EffectPower::new(devices)
            .map_err(|_| AudioStorageError::UnsupportedGraph)?
        {
            let (device, bypassed) = entry.map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let mut automation = if let Some(effects) = effects {
                crate::device_automation::prepare_closed_effect(device, effects, native_input)
            } else if native_input.is_some() {
                let automation_registry = hex_devices::native_registry()
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                crate::device_automation::prepare_with_registry_and_input(
                    device,
                    &automation_registry,
                    native_input,
                )
            } else {
                crate::device_automation::prepare(device)
            }
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let modulation =
                EffectModulation::plan(device, project, rate, automation.parameter_ids())
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            device
                .state()
                .require_asset_free_processing()
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let utility = device.state().type_id() == hex_devices::utility::UTILITY_TYPE_ID;
            let reverb = device.state().type_id() == hex_devices::reverb::REVERB_TYPE_ID;
            let saturator = device.state().type_id() == hex_devices::saturator::SATURATOR_TYPE_ID;
            let auto_pan = device.state().type_id() == hex_devices::auto_pan::AUTO_PAN_TYPE_ID;
            let overdrive = device.state().type_id() == hex_devices::overdrive::OVERDRIVE_TYPE_ID;
            let gate = device.state().type_id() == hex_devices::gate::GATE_TYPE_ID;
            let compressor =
                device.state().type_id() == hex_devices::compressor::COMPRESSOR_TYPE_ID;
            let rattle = device.state().type_id() == hex_devices::rattle::RATTLE_TYPE_ID;
            let eq8 = device.state().type_id() == hex_devices::eq8::EQ8_TYPE_ID;
            let native = crate::native_effect_storage::is_closed_type(device.state().type_id());
            let definition = &hex_devices::utility::UTILITY_DEFINITION;
            // Construct closed native factories here. Foreign factories
            // return scalar facts matched to the original source.
            let foreign_definition = if native {
                None
            } else {
                Some(
                    effects
                        .and_then(|r| r.definition(device.state().type_id()))
                        .ok_or(AudioStorageError::UnsupportedGraph)?,
                )
            };
            let migrated = if eq8 {
                hex_devices::eq8::EQ8_DEFINITION
                    .migrate_state(device.state())
                    .and_then(|state| state.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else if rattle {
                hex_devices::rattle::RATTLE_DEFINITION
                    .migrate_state(device.state())
                    .and_then(|state| state.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else if compressor {
                hex_devices::compressor::COMPRESSOR_DEFINITION
                    .migrate_state(device.state())
                    .and_then(|state| state.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else if gate {
                hex_devices::gate::GATE_DEFINITION
                    .migrate_state(device.state())
                    .and_then(|state| state.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else if overdrive {
                hex_devices::overdrive::OVERDRIVE_DEFINITION
                    .migrate_state(device.state())
                    .and_then(|state| state.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else if auto_pan {
                hex_devices::auto_pan::AUTO_PAN_DEFINITION
                    .migrate_state(device.state())
                    .and_then(|state| state.with_host_metadata_from(device.state()))
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?
            } else if saturator {
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
            let foreign = if native {
                None
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
                    Some(PreparedProcessorFactory::Foreign(facts))
                        if facts.binding().is_some_and(|b| b.is_valid()) =>
                    {
                        Some(facts)
                    }
                    _ => return Err(AudioStorageError::UnsupportedGraph),
                }
            };
            if let (Some(project), Some(effects), Some(foreign)) = (project, effects, foreign) {
                let identity = ProcessorInstanceIdentity::new(owner, device.id().value())
                    .map_err(|_| AudioStorageError::SourceMismatch)?;
                if let Some(program) = crate::foreign_automation::candidate::prepare_device(
                    device,
                    identity,
                    effects,
                    foreign,
                    project.tempo_map(),
                    project.loop_region(),
                )? {
                    if !automation.is_empty() {
                        return Err(AudioStorageError::UnsupportedGraph);
                    }
                    automation = hex_lattice::PreparedParameterAutomation::for_foreign(program);
                }
            }
            let authored = automation.foreign().is_some();
            let automation_bytes = automation
                .storage_bytes()
                .ok_or(AudioStorageError::Overflow)?;
            let parameters = modulation.as_ref().map_or_else(
                || automation.parameter_ids(),
                EffectModulation::parameter_ids,
            );
            let matrix = if bypassed
                || !native
                || !parameters.is_empty()
                || saturator
                || auto_pan
                || overdrive
                || gate
                || compressor
                || rattle
                || eq8
            {
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
            if !bypassed {
                let gain = migrated.input_gain_linear() * migrated.output_gain_linear();
                let trim = StereoMatrix::new([[gain, 0.0], [0.0, gain]])
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
                admission = admission
                    .then(trim)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            }
            let processor = if eq8 {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::eq8::Eq8Definition::prepare_owned(&migrated, rate, parameters)
                            .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else if rattle {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::rattle::RattleDefinition::prepare_owned(
                            &migrated, rate, parameters,
                        )
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else if compressor {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::compressor::CompressorDefinition::prepare_owned(
                            &migrated, rate, parameters,
                        )
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else if gate {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::gate::GateDefinition::prepare_owned(
                            &migrated, rate, parameters,
                        )
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else if overdrive {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::overdrive::OverdriveDefinition::prepare_owned(
                            &migrated, rate, parameters,
                        )
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else if auto_pan {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::auto_pan::AutoPanDefinition::prepare_owned(
                            &migrated, rate, parameters,
                        )
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else if saturator {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::saturator::SaturatorDefinition::prepare_owned(
                            &migrated, rate, parameters,
                        )
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    )
                    .into_processor_parts_on_control(),
                )
            } else if utility {
                (if parameters.is_empty() {
                    hex_devices::utility::UtilityDefinition::prepare_owned_filter(&migrated, rate)
                } else {
                    hex_devices::utility::UtilityDefinition::prepare_owned_automated_filter_for_parameters(
                        &migrated, rate, parameters,
                    )
                    .map(Some)
                })
                .map_err(|_| AudioStorageError::UnsupportedGraph)?
                .map(|value| OwnedShared::new_owned(value).into_processor_parts_on_control())
            } else if reverb {
                Some(
                    OwnedShared::new_owned(
                        hex_devices::reverb::ReverbDefinition::prepare_owned_room_for_parameters(
                            &migrated, rate, parameters,
                        )
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
            let factory = foreign
                .map(PreparedProcessorFactory::Foreign)
                .or_else(|| processor.map(PreparedProcessorFactory::Legacy));
            let modulation = modulation
                .map(|plan| {
                    let metadata: &dyn NativeDeviceMetadata = if utility {
                        definition
                    } else if saturator {
                        &hex_devices::saturator::SATURATOR_DEFINITION
                    } else if auto_pan {
                        &hex_devices::auto_pan::AUTO_PAN_DEFINITION
                    } else if overdrive {
                        &hex_devices::overdrive::OVERDRIVE_DEFINITION
                    } else if compressor {
                        &hex_devices::compressor::COMPRESSOR_DEFINITION
                    } else if eq8 {
                        &hex_devices::eq8::EQ8_DEFINITION
                    } else if rattle {
                        &hex_devices::rattle::RATTLE_DEFINITION
                    } else if gate {
                        &hex_devices::gate::GATE_DEFINITION
                    } else if reverb {
                        &hex_devices::reverb::REVERB_DEFINITION
                    } else {
                        return Err(AudioStorageError::UnsupportedGraph);
                    };
                    plan.prepare(&configuration, metadata, factory.as_ref())
                        .map_err(|_| AudioStorageError::UnsupportedGraph)
                })
                .transpose()?;
            let modulation_bytes = modulation
                .as_ref()
                .map(|value| {
                    value
                        .heap_storage_bytes()
                        .ok_or(AudioStorageError::Overflow)
                })
                .transpose()?;
            let meter = OwnedShared::new_owned(hex_lattice::DeviceMeter::default());
            meter.parts().ok_or(AudioStorageError::UnknownStorage)?;
            let (meter, meter_declaration) = meter.into_paired_parts_on_control();
            let handles = [handle()?, handle()?, handle()?, handle()?];
            self.entries.push(Entry {
                automation_bytes,
                modulation_bytes,
                authored,
                device_id: device.id().value(),
                owner,
                device_type: migrated_type,
                foreign,
                state,
                processor: processor_declaration,
                meter: meter_declaration,
                payload,
                handles,
            });
            prepared.push(PreparedRuntimeInstance {
                modulation,
                automation,
                meter,
                owner,
                device_id: device.id().value(),
                device_type: migrated_type,
                matrix,
                bypassed,
                sample_rate: rate,
                configuration: Some(configuration),
                definition: factory,
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
            let modulation_bytes = value
                .modulation
                .as_ref()
                .map(|modulation| {
                    modulation
                        .heap_storage_bytes()
                        .ok_or(AudioStorageError::Overflow)
                })
                .transpose()?;
            if modulation_bytes != pair.modulation_bytes
                || value.automation.storage_bytes() != Some(pair.automation_bytes)
                || value.device_id != pair.device_id
                || value.owner != pair.owner
                || value.device_type != pair.device_type
                || match (&value.definition, pair.foreign, pair.processor.is_some()) {
                    (Some(PreparedProcessorFactory::Foreign(actual)), Some(expected), false) => {
                        *actual != expected
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
                .and_then(|bytes| bytes.checked_add(pair.modulation_bytes.unwrap_or(0)))
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
            .map(|e| 3 + usize::from(e.processor.is_some()) + usize::from(e.authored))
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
            l.vec(value.automation.touch_storage())?;
            if let Some(modulation) = &value.modulation {
                l.bytes(
                    Owner::exclusive(value),
                    Location("prepared-modulation-backing", 0),
                    modulation
                        .heap_storage_bytes()
                        .ok_or(AudioStorageError::Overflow)?,
                )?;
            }
            if let Some(program) = value.automation.foreign() {
                crate::foreign_automation::storage::declare(program, l)?;
            }
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
    fn serial_parent_power_reaches_prepared_flags_and_matrices_without_mutating_authored_state() {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "chain".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let definition = &hex_devices::utility::UTILITY_DEFINITION;
        let state = definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                hex_devices::utility::parameter::GAIN,
                ParameterValue::Float(2.0),
            )
            .unwrap();
        for _ in 0..3 {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
        }
        let mut snapshot = editor.project().snapshot();
        snapshot.tracks[0].devices[1].parent = Some(snapshot.tracks[0].devices[0].id);
        snapshot.tracks[0].devices[0].bypassed = true;
        let project = Project::from_snapshot(snapshot).unwrap();
        let authored = project.snapshot();
        let devices = project.track(track).unwrap().devices();
        let mut custody = super::ChainCustody::default();
        let prepared = custody.prepare(devices, track, 48_000).unwrap();
        assert_eq!(
            prepared.iter().map(|d| d.bypassed).collect::<Vec<_>>(),
            [true, true, false]
        );
        assert_eq!(prepared[0].matrix, hex_lattice::StereoMatrix::identity());
        assert_eq!(prepared[1].matrix, hex_lattice::StereoMatrix::identity());
        assert_ne!(prepared[2].matrix, hex_lattice::StereoMatrix::identity());
        assert_eq!(
            prepared.iter().map(|d| d.device_id).collect::<Vec<_>>(),
            devices.iter().map(|d| d.id().value()).collect::<Vec<_>>()
        );
        assert!(!devices[1].bypassed());
        assert_eq!(project.snapshot(), authored);
        custody.bytes(&prepared).unwrap();
    }
    #[test]
    fn reverb_feedback_live_input_uses_scoped_closed_factory_and_tail_quote() {
        use hex_devices::reverb::{REVERB_DEFINITION as D, parameter as p};
        use hex_lattice_contract::DeviceAutomationTarget;
        let (project, _) = crate::audio_preparation::tests::fixture(2);
        let mut snapshot = project.snapshot();
        let id = snapshot.next_id;
        snapshot.next_id += 1;
        snapshot.tracks[0]
            .devices
            .push(hex_project::DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                id,
                bypassed: false,
                comparison: None,
                automation: vec![],
                state: D.default_state().unwrap(),
            });
        let project = hex_project::Project::from_snapshot(snapshot).unwrap();
        let track = &project.tracks()[0];
        let device = &track.devices()[0];
        let registry = hex_devices::native_registry().unwrap();
        for target in [None, Some(p::DECAY), Some(p::FREEZE), Some(p::SMOOTH)] {
            let input = target.map(|parameter| {
                crate::device_automation::native_parameter_input_target(
                    &project,
                    device.id(),
                    DeviceAutomationTarget::Parameter(parameter),
                )
                .unwrap()
            });
            let mut custody = super::ChainCustody::default();
            let (quoted, overlap) = custody
                .quote_with_input(track.devices(), project.sample_rate(), input)
                .unwrap();
            let chain = custody
                .prepare_with_effects_and_input(
                    track.devices(),
                    hex_lattice::ProcessorChainOwner::Track(track.id().value()),
                    project.sample_rate(),
                    Some(&registry),
                    input,
                    None,
                )
                .unwrap();
            assert!(custody.bytes(&chain).unwrap() <= quoted);
            assert!(custody.construction_overlap() <= overlap);
            let factory = chain[0].definition.as_ref().unwrap();
            assert_eq!(
                factory.supports_parameter_automation(p::DECAY),
                target == Some(p::DECAY)
            );
            assert_eq!(
                factory.supports_parameter_automation(p::FREEZE),
                target == Some(p::FREEZE)
            );
            assert_eq!(
                factory.supports_parameter_automation(p::SMOOTH),
                target == Some(p::SMOOTH)
            );
            assert_eq!(
                factory.output_tail(),
                if target == Some(p::FREEZE) {
                    hex_lattice::OutputTail::Unbounded
                } else {
                    hex_lattice::OutputTail::Finite {
                        max_tail_frames: u64::from(project.sample_rate())
                            * (if target == Some(p::DECAY) { 181 } else { 8 })
                            + u64::from(project.sample_rate()).div_ceil(10),
                    }
                }
            );
        }
    }
    #[test]
    fn armed_utility_provider_and_host_use_quoted_existing_chain_custody_without_points() {
        use hex_lattice_contract::DeviceAutomationTarget;
        let (project, _) = crate::audio_preparation::tests::fixture(2);
        let mut snapshot = project.snapshot();
        let id = snapshot.next_id;
        snapshot.next_id += 1;
        snapshot.tracks[0]
            .devices
            .push(hex_project::DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                id,
                bypassed: false,
                comparison: None,
                automation: Vec::new(),
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            });
        let project = hex_project::Project::from_snapshot(snapshot).unwrap();
        let track = &project.tracks()[0];
        let device = &track.devices()[0];
        let registry = hex_devices::native_registry().unwrap();
        for target in [
            DeviceAutomationTarget::Parameter(hex_devices::utility::parameter::GAIN),
            DeviceAutomationTarget::HostInputGain,
        ] {
            let input = crate::device_automation::native_parameter_input_target(
                &project,
                device.id(),
                target,
            )
            .unwrap();
            let mut custody = super::ChainCustody::default();
            let (quoted, overlap) = custody
                .quote_with_input(track.devices(), project.sample_rate(), Some(input))
                .unwrap();
            let chain = custody
                .prepare_with_effects_and_input(
                    track.devices(),
                    hex_lattice::ProcessorChainOwner::Track(track.id().value()),
                    project.sample_rate(),
                    Some(&registry),
                    Some(input),
                    None,
                )
                .unwrap();
            assert!(custody.bytes(&chain).unwrap() <= quoted);
            assert!(custody.construction_overlap() <= overlap);
            assert_eq!(chain[0].automation.input_target(), Some(target));
            assert_eq!(
                chain[0].automation.parameter_ids(),
                target.provider_parameter().as_slice()
            );
            if !target.is_host() {
                assert!(chain[0].definition.is_some());
            }
            assert!(device.automation().is_empty());
        }
    }
    #[test]
    fn sealed_native_graphs_match_canonical_pcm_report_nested_payload_and_recover_original_bank() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        for family in 0..3 {
            let saturator = family == 1;
            let auto_pan = family == 2;
            let mut static_bytes = None;
            for reading in [
                None,
                Some(hex_project::AutomationReadSelection::Authored),
                Some(hex_project::AutomationReadSelection::Base),
            ] {
                let automated = reading.is_some();
                let selection = reading.unwrap_or(hex_project::AutomationReadSelection::Authored);
                let (project, mut bank) = crate::audio_preparation::tests::fixture(2);
                bank.reservation.handle_obligations = 32;
                let original = bank.sources.as_ptr();
                let original_capacity = bank.sources.capacity();
                let mut snapshot = project.snapshot();
                snapshot.tracks[0].gain = 0.5;
                snapshot.tracks[0].balance = -0.25;
                let state = if auto_pan {
                    hex_devices::auto_pan::AUTO_PAN_DEFINITION
                        .default_state()
                        .unwrap()
                } else if saturator {
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
                        modulation: Default::default(),
                        parent: None,
                        automation: if automated {
                            vec![hex_project::DeviceAutomation {
                                touch: Vec::new(),
                                vst3_class: None,
                                read_selection: selection,
                                target: hex_lattice_contract::DeviceAutomationTarget::Parameter(
                                    if auto_pan {
                                        hex_devices::auto_pan::parameter::AMOUNT
                                    } else if saturator {
                                        hex_devices::saturator::parameter::DRIVE
                                    } else {
                                        hex_devices::utility::parameter::GAIN
                                    },
                                ),
                                points: vec![
                                    hex_project::AutomationPoint {
                                        beat: hex_project::BeatTime::ZERO,
                                        value: hex_project::AutomationValue::new(
                                            if saturator || auto_pan { 0.5 } else { 0.005 },
                                        )
                                        .unwrap(),
                                        outgoing: hex_project::AutomationInterpolation::Linear,
                                    },
                                    hex_project::AutomationPoint {
                                        beat: hex_project::BeatTime::from_raw(1 << 26),
                                        value: hex_project::AutomationValue::new(
                                            if saturator || auto_pan { 0.75 } else { 0.015 },
                                        )
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
                if automated {
                    let device = snapshot.tracks[0].devices.last_mut().unwrap();
                    for target in [
                        hex_lattice_contract::DeviceAutomationTarget::HostInputGain,
                        hex_lattice_contract::DeviceAutomationTarget::HostOutputGain,
                    ] {
                        device.automation.push(hex_project::DeviceAutomation {
                            touch: Vec::new(),
                            vst3_class: None,
                            read_selection: selection,
                            target,
                            points: vec![
                                hex_project::AutomationPoint {
                                    beat: hex_project::BeatTime::ZERO,
                                    value: hex_project::AutomationValue::new(0.65).unwrap(),
                                    outgoing: hex_project::AutomationInterpolation::Linear,
                                },
                                hex_project::AutomationPoint {
                                    beat: hex_project::BeatTime::from_raw(1 << 26),
                                    value: hex_project::AutomationValue::new(0.75).unwrap(),
                                    outgoing: hex_project::AutomationInterpolation::Step,
                                },
                            ],
                        });
                    }
                }
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
                let input = if saturator || auto_pan {
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
                    // The oversampled Saturator delays its output and drains
                    // within its declared finite tail after the 16-frame input.
                    use hex_lattice::PreparedNativeProcessor as _;
                    let prepared = hex_devices::saturator::SaturatorDefinition::prepare_owned(
                        project.tracks()[0].devices()[0].state(),
                        project.sample_rate(),
                        &[],
                    )
                    .unwrap();
                    let hex_lattice::OutputTail::Finite { max_tail_frames } =
                        prepared.output_tail()
                    else {
                        panic!("finite saturator tail")
                    };
                    let end = 16 + usize::try_from(max_tail_frames).unwrap();
                    assert!(actual[..16].iter().any(|frame| frame[0] != 0.0));
                    assert!(
                        actual[16..end].iter().any(|frame| frame[0] != 0.0),
                        "the delayed output continues past the input"
                    );
                    assert!(
                        actual[end..].iter().all(|frame| *frame == [0.0; 2]),
                        "Saturator output ends within its declared tail"
                    );
                } else if auto_pan {
                    assert!(actual[..16].iter().any(|frame| frame[0] != 0.0));
                    assert!(
                        actual[16..].iter().all(|frame| *frame == [0.0; 2]),
                        "memoryless Auto Pan has no output tail"
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
