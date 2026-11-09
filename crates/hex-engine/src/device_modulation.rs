//! Translate a saved device bank into a bounded device-local render declaration.
//! Control preparation resolves stable IDs and validates real factory bindings.
//! Prepared declarations borrow no project data and never enter project history.
use hex_devices::modulation::{audio_rate, envelope_follower, lfo, macro_value, random_hold};
use hex_lattice::{
    HostGainModulationRoute, ModulationRoute, ModulationTarget, PreparedModulation,
    PreparedModulationSource, PreparedModulationState, PreparedMutableInstrumentRuntime,
    PreparedProcessorFactory, SourceModulationParameter, SourceModulationRoute,
};
use hex_lattice_contract::{
    DeviceAutomationTarget, HostGain, NativeDeviceMetadata, NativeDeviceState, ParameterDescriptor,
    ParameterId, ParameterModulation, ParameterSemantics,
};
use hex_project::{
    Device, DeviceId, DeviceLocation, DeviceOwner, DeviceUnitClass, Invalid, ModulationDestination,
    ModulationOutputId, Project,
};

/// Small chunks keep device-local scratch independent of the host callback size.
const CHUNK_FRAMES: usize = 128;

fn mapping_effect(project: &Project, device: DeviceId) -> Option<DeviceLocation<'_>> {
    let location = project.device_location(device)?;
    if location.class != DeviceUnitClass::Effect {
        return None;
    }
    if let DeviceOwner::Source(source) = location.owner {
        let source = project.native_source(source)?;
        source.receiver()?;
        if !source
            .post_instrument_effects()
            .iter()
            .any(|effect| effect.id() == device)
        {
            return None;
        }
    }
    Some(location)
}

/// State-aware eligibility for native device-panel mapping. Ownership is
/// resolved from the current project. Preparation independently validates the
/// actual factory, runtime binding and source program before publication.
pub fn project_device_modulation_target_supported(
    project: &Project,
    device: DeviceId,
    parameter: ParameterId,
) -> bool {
    let Some(location) = mapping_effect(project, device) else {
        return false;
    };
    let device = location.device;
    if matches!(location.owner, DeviceOwner::Source(_))
        && !source_provider_mapping_supported(device.state(), parameter)
    {
        return false;
    }
    let Ok(registry) = hex_devices::native_registry() else {
        return false;
    };
    let Some(definition) = registry.definition(device.state().type_id()) else {
        return false;
    };
    let state = device.state();
    hex_lattice_contract::validated_parameter_semantics(definition, state, parameter)
        .ok()
        .flatten()
        .is_some_and(|semantics| semantics.modulation == ParameterModulation::SampleRate)
        && definition
            .parameter_enabled(state, parameter)
            .unwrap_or(false)
}

/// Control-side mapping eligibility for both native host gain stages.
/// The closed provider's actual factory supplies the latency. No render
/// runtime or audio resources are constructed by this query.
pub fn project_device_host_gain_modulation_supported(project: &Project, device: DeviceId) -> bool {
    let Some(location) = mapping_effect(project, device) else {
        return false;
    };
    let device = location.device;
    let state = device.state();
    if !crate::native_effect_storage::is_closed_type(state.type_id()) {
        return false;
    }
    let Ok(registry) = hex_devices::native_registry() else {
        return false;
    };
    if matches!(location.owner, DeviceOwner::Source(_))
        && crate::source_effect_stage::metadata::validate_authored(state, &registry).is_err()
    {
        return false;
    }
    let context = hex_lattice::DevicePreparationContext {
        sample_rate: project.sample_rate(),
        tuning: &hex_lattice::TuningTable::default(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    registry
        .prepare_processor(state, &context)
        .is_ok_and(|factory| {
            factory.is_none_or(|factory| factory.algorithmic_latency_frames() == Some(0))
        })
}

/// These source providers keep a zero-latency processor and identity host
/// matrix when the first provider route is added or the last is removed.
fn source_provider_mapping_supported(state: &NativeDeviceState, parameter: ParameterId) -> bool {
    let id = state.type_id();
    id == hex_devices::auto_pan::AUTO_PAN_TYPE_ID
        || id == hex_devices::gate::GATE_TYPE_ID
        || id == hex_devices::compressor::COMPRESSOR_TYPE_ID
        || id == hex_devices::reverb::REVERB_TYPE_ID
        || id == hex_devices::utility::UTILITY_TYPE_ID
            && hex_devices::utility::UtilityDefinition::supports_source_matrix_modulation(
                state, parameter,
            )
}

pub(super) fn validate_source_owners(project: &Project) -> Result<(), Invalid> {
    for source in project.native_sources() {
        if source
            .devices()
            .take(source.instrument_index())
            .any(|device| !device.modulation().is_empty())
        {
            return Err(Invalid("source prefix modulation is unsupported"));
        }
        let instrument = source.instrument();
        // An unbound instrument stays in the retained source inventory. Keep
        // its bank for a later binding and compile it only for a real receiver.
        if !instrument.modulation().is_empty()
            && instrument.state().type_id() != hex_devices::fm4::device::FM4_TYPE_ID
        {
            return Err(Invalid(
                "instrument modulation requires a supported provider",
            ));
        }
        for device in source.post_instrument_effects() {
            EffectModulation::validate(device, project)?;
        }
    }
    Ok(())
}

pub(super) struct EffectModulation {
    sources: Vec<PreparedModulationSource>,
    routes: Vec<ModulationRoute>,
    host_input_routes: Vec<HostGainModulationRoute>,
    host_output_routes: Vec<HostGainModulationRoute>,
    source_routes: Vec<SourceModulationRoute>,
    parameters: Vec<ParameterId>,
}

impl EffectModulation {
    pub(super) fn validate(device: &Device, project: &Project) -> Result<(), Invalid> {
        if !device.modulation().is_empty() {
            Self::plan(device, Some(project), project.sample_rate(), &[])?;
        }
        Ok(())
    }

    pub(super) fn plan(
        device: &Device,
        project: Option<&Project>,
        sample_rate: u32,
        automation: &[ParameterId],
    ) -> Result<Option<Self>, Invalid> {
        let bank = device.modulation();
        if bank.is_empty() {
            return Ok(None);
        }
        let invalid_source = |_| Invalid("modulator state or rate is unsupported");
        let (fallback_bpm, maximum_bpm) = project.map_or((120.0, 120.0), |project| {
            let tempo_map = project.tempo_map();
            let fallback = tempo_map.tempo_at(hex_project::BeatTime::ZERO).micro_bpm();
            let maximum = tempo_map
                .points()
                .iter()
                .map(|point| point.tempo().micro_bpm())
                .max()
                .unwrap_or(fallback);
            (f64::from(fallback) / 1e6, f64::from(maximum) / 1e6)
        });
        let mut ordered = reserved(bank.sources().len())?;
        ordered.extend(bank.sources());
        ordered.sort_unstable_by_key(|source| source.id);

        let mut sources = reserved(ordered.len())?;
        let mut ids = reserved(ordered.len())?;
        for source in ordered {
            let prepared = match source.state.type_id() {
                lfo::TYPE_ID => lfo::Settings::decode(&source.state).and_then(|settings| {
                    settings.prepare(source.id, sample_rate, fallback_bpm, maximum_bpm)
                }),
                audio_rate::TYPE_ID => audio_rate::prepare(&source.state, source.id, sample_rate),
                envelope_follower::TYPE_ID => envelope_follower::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                macro_value::TYPE_ID => macro_value::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                random_hold::TYPE_ID => random_hold::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                _ => return Err(Invalid("modulator state or rate is unsupported")),
            }
            .map_err(invalid_source)?;
            if source.enabled {
                ids.push(source.id);
                sources.push(prepared);
            }
        }
        if sources.is_empty() {
            return Ok(None);
        }

        let source_route_count = bank
            .routes()
            .iter()
            .filter(|route| matches!(route.destination, ModulationDestination::Modulator { .. }))
            .count();
        let host_route_count = |target| {
            bank.routes()
                .iter()
                .filter(|route| {
                    matches!(route.destination, ModulationDestination::Device { target: candidate, .. }
                        if candidate == target)
                })
                .count()
        };
        let host_input_count = host_route_count(DeviceAutomationTarget::HostInputGain);
        let host_output_count = host_route_count(DeviceAutomationTarget::HostOutputGain);
        let mut routes = reserved(
            bank.routes().len() - source_route_count - host_input_count - host_output_count,
        )?;
        let mut host_input_routes = reserved(host_input_count)?;
        let mut host_output_routes = reserved(host_output_count)?;
        let mut source_routes = reserved(source_route_count)?;
        let mut parameters = reserved(automation.len() + bank.routes().len())?;
        parameters.extend_from_slice(automation);
        for route in bank.routes().iter().filter(|route| route.enabled) {
            let Ok(source) = ids.binary_search(&route.source) else {
                continue;
            };
            if route.output != ModulationOutputId::MAIN {
                return Err(Invalid("modulator source output is unsupported"));
            }
            let amount = route.amount.get();
            let range = [route.range.start.get(), route.range.end.get()];
            match route.destination {
                ModulationDestination::Device {
                    device_type,
                    target,
                    vst3_class: None,
                } if device_type == device.state().type_id() => {
                    let host_routes = match target {
                        DeviceAutomationTarget::HostInputGain => Some(&mut host_input_routes),
                        DeviceAutomationTarget::HostOutputGain => Some(&mut host_output_routes),
                        _ => None,
                    };
                    if let Some(host_routes) = host_routes {
                        host_routes.push(HostGainModulationRoute {
                            source,
                            amount,
                            range,
                        });
                        continue;
                    }
                    let parameter = target
                        .provider_parameter()
                        .ok_or(Invalid("modulation destination is unsupported"))?;
                    parameters.push(parameter);
                    routes.push(ModulationRoute {
                        source,
                        target: parameter,
                        amount,
                        range,
                    });
                }
                ModulationDestination::Modulator {
                    source: target,
                    module_type,
                    parameter,
                } => {
                    let target = ids.binary_search(&target).ok();
                    let target_source = target.and_then(|index| sources.get(index));
                    let parameter = match (module_type, parameter) {
                        (macro_value::TYPE_ID, macro_value::VALUE_PARAMETER) => {
                            SourceModulationParameter::MacroValue
                        }
                        (lfo::TYPE_ID, lfo::RATE_PARAMETER) => {
                            if target_source.is_some_and(|source| {
                                source.supports_modulation_parameter(
                                    SourceModulationParameter::TempoLfoRate,
                                )
                            }) {
                                SourceModulationParameter::TempoLfoRate
                            } else {
                                SourceModulationParameter::FreeLfoRate
                            }
                        }
                        _ => return Err(Invalid("modulation destination is unsupported")),
                    };
                    if let Some(target) = target {
                        if !target_source
                            .is_some_and(|source| source.supports_modulation_parameter(parameter))
                        {
                            return Err(Invalid("modulation destination is unsupported"));
                        }
                        source_routes.push(SourceModulationRoute {
                            source,
                            target,
                            parameter,
                            amount,
                            range,
                        });
                    }
                }
                _ => return Err(Invalid("modulation destination is unsupported")),
            }
        }
        parameters.sort_unstable();
        parameters.dedup();
        Ok(Some(Self {
            sources,
            routes,
            host_input_routes,
            host_output_routes,
            source_routes,
            parameters,
        }))
    }

    /// Retained declaration and temporary preparation backing. Include every
    /// possible provider automation slot, including live and external inputs,
    /// so arming a parameter never needs a larger modulation reservation.
    pub(super) fn quote(device: &Device) -> Option<(usize, usize)> {
        let bank = device.modulation();
        let source_routes = bank
            .routes()
            .iter()
            .filter(|route| matches!(route.destination, ModulationDestination::Modulator { .. }))
            .count();
        Self::quote_routes(bank.sources().len(), bank.routes().len(), 1, source_routes)
    }

    /// The same estimate applies to summed source and route counts when
    /// reserving a persistent host for the supported project cardinalities.
    pub(super) fn quote_counts(
        sources: usize,
        routes: usize,
        banks: usize,
    ) -> Option<(usize, usize)> {
        Self::quote_routes(sources, routes, banks, routes)
    }

    fn quote_routes(
        sources: usize,
        routes: usize,
        banks: usize,
        source_routes: usize,
    ) -> Option<(usize, usize)> {
        use hex_lattice_contract::{MAX_PARAMETERS_PER_DEVICE, ModulationSourceId};
        use hex_project::Modulator;
        use std::mem::size_of;
        if sources == 0 {
            return Some((0, 0));
        }
        let targets = banks.checked_mul(MAX_PARAMETERS_PER_DEVICE)?;
        let retained = PreparedModulationState::quoted_heap_bytes(sources, targets, routes)?
            .checked_add(PreparedModulationState::quoted_source_route_bytes(
                source_routes,
            )?)?;
        let overlap = sources
            .checked_mul(size_of::<&Modulator>() + size_of::<ModulationSourceId>())?
            .checked_add(routes.checked_mul(
                size_of::<ModulationRoute>().max(size_of::<HostGainModulationRoute>())
                    + size_of::<ParameterId>(),
            )?)?
            .checked_add(
                targets.checked_mul(size_of::<ParameterId>() + size_of::<ModulationTarget>())?,
            )?
            .checked_add(PreparedModulationState::quoted_source_route_overlap(
                source_routes,
            )?)?;
        Some((retained, overlap))
    }

    pub(super) fn parameter_ids(&self) -> &[ParameterId] {
        &self.parameters
    }

    pub(super) fn prepare(
        self,
        state: &NativeDeviceState,
        metadata: &dyn NativeDeviceMetadata,
        factory: Option<&PreparedProcessorFactory>,
    ) -> Result<PreparedModulationState, Invalid> {
        if matches!(factory, Some(PreparedProcessorFactory::Foreign(_))) {
            return Err(Invalid("foreign effect modulation is unsupported"));
        }
        let targets = self.prepare_targets(state, metadata, |parameter| {
            let factory = factory?;
            let descriptor = factory
                .parameter_automation_descriptor(parameter)
                .filter(|_| factory.supports_parameter_automation(parameter))?;
            Some((
                descriptor,
                factory.parameter_modulation_semantics(parameter),
            ))
        })?;
        let host_gain = |millidb, routes: &[HostGainModulationRoute]| {
            if routes.is_empty() {
                return Ok(None);
            }
            if factory.map_or(
                Some(0),
                PreparedProcessorFactory::algorithmic_latency_frames,
            ) != Some(0)
            {
                return Err(Invalid(
                    "host gain modulation requires a zero-latency native effect",
                ));
            }
            HostGain::from_millidb(millidb)
                .map(Some)
                .map_err(|_| Invalid("authored host gain is invalid"))
        };
        let input = host_gain(state.input_gain_millidb(), &self.host_input_routes)?;
        let output = host_gain(state.output_gain_millidb(), &self.host_output_routes)?;
        let program = PreparedModulation::with_host_gains(
            &targets,
            &self.routes,
            self.sources.len(),
            input.map(|base| (base, self.host_input_routes.as_slice())),
            output.map(|base| (base, self.host_output_routes.as_slice())),
        )
        .map_err(|_| Invalid("modulation program is invalid or exceeds its capacity"))?;
        PreparedModulationState::with_source_routes(
            self.sources,
            program,
            CHUNK_FRAMES,
            &self.source_routes,
        )
        .map_err(|_| Invalid("modulation declaration exceeds its capacity"))
    }
    pub(super) fn prepare_instrument(
        self,
        state: &NativeDeviceState,
        metadata: &dyn NativeDeviceMetadata,
        factory: &dyn PreparedMutableInstrumentRuntime,
    ) -> Result<PreparedModulationState, Invalid> {
        if !self.host_input_routes.is_empty()
            || !self.host_output_routes.is_empty()
            || self
                .sources
                .iter()
                .any(PreparedModulationSource::reads_audio_input)
        {
            return Err(Invalid(
                "instrument modulation requires sources independent of audio input and provider targets",
            ));
        }
        let targets = self.prepare_targets(state, metadata, |parameter| {
            let descriptor = *factory
                .parameter_automation_descriptors()
                .iter()
                .find(|descriptor| descriptor.id == parameter)?;
            Some((
                descriptor,
                factory.parameter_modulation_semantics(parameter),
            ))
        })?;
        let program = PreparedModulation::new(&targets, &self.routes, self.sources.len())
            .map_err(|_| Invalid("instrument modulation program is invalid"))?;
        // Session source generation splits at the existing 256-frame boundary.
        PreparedModulationState::with_source_routes(self.sources, program, 256, &self.source_routes)
            .map_err(|_| Invalid("instrument modulation exceeds its bounded storage"))
    }

    fn prepare_targets(
        &self,
        state: &NativeDeviceState,
        metadata: &dyn NativeDeviceMetadata,
        grant: impl Fn(ParameterId) -> Option<(ParameterDescriptor, Option<ParameterSemantics>)>,
    ) -> Result<Vec<ModulationTarget>, Invalid> {
        let mut targets = reserved(self.parameters.len())?;
        for &parameter in &self.parameters {
            let unsupported = || Invalid("provider does not grant this sample-rate parameter");
            let (descriptor, semantics) = grant(parameter).ok_or_else(unsupported)?;
            let semantics = if self.routes.iter().any(|route| route.target == parameter) {
                semantics.filter(|value| value.modulation == ParameterModulation::SampleRate)
            } else {
                hex_lattice_contract::validated_parameter_semantics(metadata, state, parameter)
                    .map_err(|_| unsupported())?
            }
            .ok_or_else(unsupported)?;
            let base = metadata
                .read_parameter(state, parameter)
                .map_err(|_| Invalid("authored modulation base value is invalid"))?;
            targets.push(ModulationTarget {
                descriptor,
                semantics,
                base,
            });
        }
        Ok(targets)
    }
}

fn reserved<T>(count: usize) -> Result<Vec<T>, Invalid> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve modulation preparation storage"))?;
    Ok(values)
}

#[cfg(test)]
mod macro_tests;

#[cfg(test)]
mod random_tests;
