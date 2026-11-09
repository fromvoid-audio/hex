//! Translate a saved device bank into a bounded device-local render declaration.
//! Control preparation resolves stable IDs and validates real factory bindings.
//! Prepared declarations borrow no project data and never enter project history.
use hex_devices::modulation::{
    adsr, ahd_release, audio_rate, button, buttons, envelope_follower, lfo, macro_four,
    macro_value, math, mix, note_counter, note_pressure, ramp, random_hold, steps,
};
use hex_lattice::{
    HostGainModulationRoute, MAX_MODULATION_SOURCES, ModulationRoute, ModulationTarget,
    PreparedModulation, PreparedModulationSource, PreparedModulationState,
    PreparedMutableInstrumentRuntime, PreparedProcessorFactory, SourceModulationParameter,
    SourceModulationRoute,
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
pub(crate) const INSTRUMENT_MODULATION_CHUNK_FRAMES: usize = 256;

fn mapping_effect(project: &Project, device: DeviceId) -> Option<DeviceLocation<'_>> {
    let location = project.device_location(device)?;
    effect_location_supported(project, location).then_some(location)
}

fn effect_location_supported(project: &Project, location: DeviceLocation<'_>) -> bool {
    if location.class != DeviceUnitClass::Effect {
        return false;
    }
    if let DeviceOwner::Source(source) = location.owner {
        return project.native_source(source).is_some_and(|source| {
            source.receiver().is_some()
                && source
                    .post_instrument_effects()
                    .iter()
                    .any(|effect| effect.id() == location.device.id())
        });
    }
    true
}

fn fm4_modulation_supported(source: &hex_project::NativeSource) -> bool {
    source.receiver().is_some()
        && source.instrument().state().type_id() == hex_devices::fm4::device::FM4_TYPE_ID
}

fn loaded_sampler_modulation_supported(state: &NativeDeviceState) -> bool {
    state.type_id() == hex_devices::sampler::state::TYPE_ID
        && state.asset_references().len() == 1
        && !hex_devices::sampler::state::is_empty_state(state)
}

fn instrument_note_events_supported(source: &hex_project::NativeSource) -> bool {
    fm4_modulation_supported(source)
        || source.receiver().is_some()
            && loaded_sampler_modulation_supported(source.instrument().state())
}

/// Instrument owners whose current prepared runtime supplies ordered note events.
pub fn project_device_has_modulation_note_events(project: &Project, device: DeviceId) -> bool {
    let Some(location) = project.device_location(device) else {
        return false;
    };
    let (DeviceUnitClass::Instrument, DeviceOwner::Source(source)) =
        (location.class, location.owner)
    else {
        return false;
    };
    project
        .native_source(source)
        .is_some_and(instrument_note_events_supported)
}

/// Remaining device-local prepared signal buffers. Occurrence pressure is
/// owned by the instrument and does not use a bank signal buffer.
pub fn device_available_modulation_outputs(device: &Device) -> usize {
    let used = device
        .modulation()
        .sources()
        .iter()
        .filter(|slot| slot.enabled && slot.state.type_id() != note_pressure::TYPE_ID)
        .map(|slot| source_output_count(slot.state.type_id()))
        .sum::<usize>();
    MAX_MODULATION_SOURCES.saturating_sub(used)
}

/// The native note-pressure slot belongs to the FM4 source receiver. A
/// bank with a pressure slot already owns that source identity.
pub fn project_device_can_add_note_pressure(project: &Project, device: DeviceId) -> bool {
    let Some(location) = project.device_location(device) else {
        return false;
    };
    let (DeviceUnitClass::Instrument, DeviceOwner::Source(source)) =
        (location.class, location.owner)
    else {
        return false;
    };
    project
        .native_source(source)
        .is_some_and(fm4_modulation_supported)
        && !location
            .device
            .modulation()
            .sources()
            .iter()
            .any(|source| source.state.type_id() == note_pressure::TYPE_ID)
}

/// FM4's per-occurrence pressure grant is limited to carrier weights.
pub fn fm4_note_pressure_parameter_supported(parameter: ParameterId) -> bool {
    use hex_devices::fm4::device::parameter::carrier;
    (carrier(1)..=carrier(4)).contains(&parameter)
}

/// State-aware eligibility for native device-panel mapping. Ownership is
/// resolved from the current project. Preparation independently validates the
/// actual factory, runtime binding and source program before publication.
pub fn project_device_modulation_target_supported(
    project: &Project,
    device: DeviceId,
    parameter: ParameterId,
) -> bool {
    let Some(location) = project.device_location(device) else {
        return false;
    };
    let device = location.device;
    let eligible = match (location.class, location.owner) {
        (DeviceUnitClass::Instrument, DeviceOwner::Source(source)) => project
            .native_source(source)
            .is_some_and(instrument_note_events_supported),
        (DeviceUnitClass::Effect, DeviceOwner::Source(_)) => {
            effect_location_supported(project, location)
                && source_provider_mapping_supported(device.state(), parameter)
        }
        _ => effect_location_supported(project, location),
    };
    if !eligible {
        return false;
    }
    let state = device.state();
    let definition: Option<&dyn NativeDeviceMetadata> = match location.class {
        DeviceUnitClass::Instrument => hex_devices::native_instrument_registry()
            .ok()
            .and_then(|registry| registry.definition(state.type_id()))
            .map(|definition| definition as &dyn NativeDeviceMetadata),
        DeviceUnitClass::Effect => hex_devices::native_registry()
            .ok()
            .and_then(|registry| registry.definition(state.type_id()))
            .map(|definition| definition as &dyn NativeDeviceMetadata),
    };
    let Some(definition) = definition else {
        return false;
    };
    let migrated;
    let state = if state.state_version() == definition.state_version() {
        state
    } else {
        let Ok(current) = definition
            .migrate_state(state)
            .and_then(|current| current.with_host_metadata_from(state))
        else {
            return false;
        };
        migrated = current;
        &migrated
    };
    hex_lattice_contract::validated_parameter_semantics(definition, state, parameter)
        .ok()
        .flatten()
        .is_some_and(|semantics| semantics.modulation == ParameterModulation::SampleRate)
        && definition
            .parameter_enabled(state, parameter)
            .unwrap_or(false)
}

fn instrument_unity_output_modulation_supported(state: &NativeDeviceState) -> bool {
    state.output_gain_millidb() == 0
        && (state.type_id() == hex_devices::fm4::device::FM4_TYPE_ID
            || loaded_sampler_modulation_supported(state))
}

/// A receiving FM4 or loaded resident Sampler can modulate its unity trim
/// after its voices mix.
pub fn project_device_instrument_output_modulation_supported(
    project: &Project,
    device: DeviceId,
) -> bool {
    let Some(location) = project.device_location(device) else {
        return false;
    };
    let (DeviceUnitClass::Instrument, DeviceOwner::Source(source)) =
        (location.class, location.owner)
    else {
        return false;
    };
    project.native_source(source).is_some_and(|source| {
        source.receiver().is_some()
            && instrument_unity_output_modulation_supported(source.instrument().state())
    })
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
        || id == hex_devices::delay::TYPE_ID
            && matches!(
                parameter,
                hex_devices::delay::FEEDBACK
                    | hex_devices::delay::DRY_WET
                    | hex_devices::delay::LFO_DELAY
                    | hex_devices::delay::LFO_RATE
            )
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
        if !instrument.modulation().is_empty() {
            let state = instrument.state();
            if !loaded_sampler_modulation_supported(state)
                && state.type_id() != hex_devices::fm4::device::FM4_TYPE_ID
            {
                return Err(Invalid(
                    "instrument modulation requires a supported provider",
                ));
            }
        }
        for device in source.post_instrument_effects() {
            EffectModulation::validate(device, project)?;
        }
    }
    Ok(())
}

pub(super) struct VoicePressure {
    pub source: hex_lattice_contract::ModulationSourceId,
    pub route: Option<hex_lattice::PreparedPressureRoute>,
}

/// Resolve the authored source on control. The instrument's prepared provider
/// independently admits the exact descriptor and owns the per-voice reading.
pub(super) fn prepare_voice_pressure(
    device: &Device,
    registry: &hex_lattice::NativeInstrumentRegistry<'_>,
) -> Result<Option<VoicePressure>, Invalid> {
    let bank = device.modulation();
    let mut slots = bank
        .sources()
        .iter()
        .filter(|slot| slot.state.type_id() == note_pressure::TYPE_ID);
    let Some(slot) = slots.next() else {
        return Ok(None);
    };
    if device.state().type_id() != hex_devices::fm4::device::FM4_TYPE_ID || slots.next().is_some() {
        return Err(Invalid(
            "one note pressure source is supported on an fm4 receiver",
        ));
    }
    note_pressure::validate(&slot.state).map_err(|_| Invalid("note pressure state is invalid"))?;
    let mut routes = bank.routes().iter().filter(|route| route.source == slot.id);
    let Some(route) = routes.next() else {
        return Ok(Some(VoicePressure {
            source: slot.id,
            route: None,
        }));
    };
    if routes.next().is_some() || route.output != ModulationOutputId::MAIN {
        return Err(Invalid("note pressure supports one carrier assignment"));
    }
    let ModulationDestination::Device {
        device_type,
        target: DeviceAutomationTarget::Parameter(parameter),
        vst3_class: None,
    } = route.destination
    else {
        return Err(Invalid("note pressure requires a carrier assignment"));
    };
    let invalid_target = || Invalid("note pressure carrier is unsupported");
    if device_type != device.state().type_id() || !fm4_note_pressure_parameter_supported(parameter)
    {
        return Err(invalid_target());
    }
    let definition = registry
        .definition(device_type)
        .ok_or_else(invalid_target)?;
    let descriptor = definition
        .descriptors()
        .iter()
        .find(|descriptor| descriptor.id == parameter)
        .copied()
        .ok_or_else(invalid_target)?;
    let semantics =
        hex_lattice_contract::validated_parameter_semantics(definition, device.state(), parameter)
            .map_err(|_| invalid_target())?
            .ok_or_else(invalid_target)?;
    let prepared = hex_lattice::PreparedPressureRoute::new(
        descriptor,
        semantics,
        route.amount.get(),
        [route.range.start.get(), route.range.end.get()],
    )
    .map_err(|_| invalid_target())?;
    Ok(Some(VoicePressure {
        source: slot.id,
        route: (slot.enabled && route.enabled).then_some(prepared),
    }))
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
        Self::plan_bank(device, project, sample_rate, automation, None, false)
    }

    pub(super) fn plan_instrument(
        device: &Device,
        project: &Project,
        automation: &[ParameterId],
    ) -> Result<Option<Self>, Invalid> {
        let voice_source = device
            .modulation()
            .sources()
            .iter()
            .find(|slot| slot.state.type_id() == note_pressure::TYPE_ID)
            .map(|slot| slot.id);
        Self::plan_bank(
            device,
            Some(project),
            project.sample_rate(),
            automation,
            voice_source,
            true,
        )
    }

    fn plan_bank(
        device: &Device,
        project: Option<&Project>,
        sample_rate: u32,
        automation: &[ParameterId],
        voice_source: Option<hex_lattice_contract::ModulationSourceId>,
        instrument_events: bool,
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
        ordered.extend(
            bank.sources()
                .iter()
                .filter(|slot| Some(slot.id) != voice_source),
        );
        ordered.sort_unstable_by_key(|source| source.id);

        let output_count = ordered
            .iter()
            .filter(|slot| slot.enabled)
            .map(|slot| source_output_count(slot.state.type_id()))
            .sum::<usize>();
        if output_count > MAX_MODULATION_SOURCES {
            return Err(Invalid("device modulator output capacity reached"));
        }
        let mut sources = reserved(output_count)?;
        let mut outputs = reserved(output_count)?;
        for source in ordered {
            let mut retain = |output, prepared| {
                if source.enabled {
                    outputs.push((source.id, output));
                    sources.push(prepared);
                }
            };
            if source.state.type_id() == macro_four::TYPE_ID {
                let settings =
                    macro_four::Settings::decode(&source.state).map_err(invalid_source)?;
                for parameter in macro_four::VALUE_PARAMETERS {
                    let output = macro_four::output_for_parameter(parameter)
                        .ok_or(Invalid("macro-4 output is unsupported"))?;
                    let prepared = settings
                        .prepare(source.id, sample_rate, output)
                        .map_err(invalid_source)?;
                    retain(output, prepared);
                }
                continue;
            }
            if source.state.type_id() == buttons::TYPE_ID {
                let settings = buttons::Settings::decode(&source.state).map_err(invalid_source)?;
                for number in 1..=buttons::OUTPUT_COUNT as u32 {
                    let output = ModulationOutputId::new(number).map_err(invalid_source)?;
                    let prepared = settings
                        .prepare(source.id, sample_rate, output)
                        .map_err(invalid_source)?;
                    retain(output, prepared);
                }
                continue;
            }
            let prepared = match source.state.type_id() {
                lfo::TYPE_ID => lfo::Settings::decode(&source.state).and_then(|settings| {
                    settings.prepare(source.id, sample_rate, fallback_bpm, maximum_bpm)
                }),
                audio_rate::TYPE_ID => audio_rate::prepare(&source.state, source.id, sample_rate),
                button::TYPE_ID => button::prepare(&source.state, source.id, sample_rate),
                envelope_follower::TYPE_ID => envelope_follower::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                macro_value::TYPE_ID => macro_value::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                math::TYPE_ID => math::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                mix::TYPE_ID => mix::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                note_counter::TYPE_ID => note_counter::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                ramp::TYPE_ID => ramp::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                ahd_release::TYPE_ID => ahd_release::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                adsr::TYPE_ID => adsr::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                steps::TYPE_ID => steps::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                random_hold::TYPE_ID => random_hold::Settings::decode(&source.state)
                    .and_then(|settings| settings.prepare(source.id, sample_rate)),
                _ => return Err(Invalid("modulator state or rate is unsupported")),
            }
            .map_err(invalid_source)?;
            if prepared.reads_note_events() && !instrument_events {
                return Err(Invalid("this modulator requires instrument note events"));
            }
            retain(ModulationOutputId::MAIN, prepared);
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
            let Ok(source) = outputs.binary_search(&(route.source, route.output)) else {
                if outputs
                    .binary_search_by_key(&route.source, |(id, _)| *id)
                    .is_ok()
                {
                    return Err(Invalid("modulator source output is unsupported"));
                }
                continue;
            };
            let amount = route.amount.get();
            let range = [route.range.start.get(), route.range.end.get()];
            if range != [-1.0, 1.0]
                && bank.sources().iter().any(|slot| {
                    slot.id == route.source && button::uses_identity_range(slot.state.type_id())
                })
            {
                return Err(Invalid("button assignments use the original on/off values"));
            }
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
                    let output = if module_type == macro_four::TYPE_ID {
                        macro_four::output_for_parameter(parameter)
                            .ok_or(Invalid("macro-4 control is unsupported"))?
                    } else {
                        ModulationOutputId::MAIN
                    };
                    let target = outputs.binary_search(&(target, output)).ok();
                    let target_source = target.and_then(|index| sources.get(index));
                    let parameter = match (module_type, parameter) {
                        (macro_four::TYPE_ID, _)
                        | (macro_value::TYPE_ID, macro_value::VALUE_PARAMETER) => {
                            SourceModulationParameter::MacroValue
                        }
                        (math::TYPE_ID, math::INPUT_A) => SourceModulationParameter::MathA,
                        (math::TYPE_ID, math::INPUT_B) => SourceModulationParameter::MathB,
                        (mix::TYPE_ID, mix::INPUT_A) => SourceModulationParameter::MixA,
                        (mix::TYPE_ID, mix::INPUT_B) => SourceModulationParameter::MixB,
                        (mix::TYPE_ID, mix::BLEND) => SourceModulationParameter::MixBlend,
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
        let outputs = bank
            .sources()
            .iter()
            .map(|slot| source_output_count(slot.state.type_id()))
            .sum::<usize>()
            .min(MAX_MODULATION_SOURCES);
        Self::quote_routes(outputs, bank.routes().len(), 1, source_routes)
    }

    /// The same estimate applies to summed source and route counts when
    /// reserving a persistent host for the supported project cardinalities.
    pub(super) fn quote_counts(
        sources: usize,
        routes: usize,
        banks: usize,
    ) -> Option<(usize, usize)> {
        let outputs = sources
            .checked_mul(macro_four::VALUE_PARAMETERS.len())?
            .min(banks.checked_mul(MAX_MODULATION_SOURCES)?);
        Self::quote_routes(outputs, routes, banks, routes)
    }

    fn quote_routes(
        outputs: usize,
        routes: usize,
        banks: usize,
        source_routes: usize,
    ) -> Option<(usize, usize)> {
        use hex_lattice_contract::{MAX_PARAMETERS_PER_DEVICE, ModulationSourceId};
        use hex_project::Modulator;
        use std::mem::size_of;
        if outputs == 0 {
            return Some((0, 0));
        }
        let targets = banks.checked_mul(MAX_PARAMETERS_PER_DEVICE)?;
        let retained = PreparedModulationState::quoted_heap_bytes(outputs, targets, routes)?
            .checked_add(PreparedModulationState::quoted_source_route_bytes(
                source_routes,
            )?)?;
        let overlap = outputs
            .checked_mul(
                size_of::<&Modulator>() + size_of::<(ModulationSourceId, ModulationOutputId)>(),
            )?
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
        let output = if self.host_output_routes.is_empty() {
            None
        } else {
            if !instrument_unity_output_modulation_supported(state) {
                return Err(Invalid(
                    "instrument output modulation requires a supported provider with unity trim",
                ));
            }
            Some((HostGain::ZERO, self.host_output_routes.as_slice()))
        };
        let program = PreparedModulation::with_host_gains(
            &targets,
            &self.routes,
            self.sources.len(),
            None,
            output,
        )
        .map_err(|_| Invalid("instrument modulation program is invalid"))?;
        PreparedModulationState::with_source_routes(
            self.sources,
            program,
            INSTRUMENT_MODULATION_CHUNK_FRAMES,
            &self.source_routes,
        )
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

fn source_output_count(device_type: hex_lattice_contract::DeviceTypeId) -> usize {
    match device_type {
        macro_four::TYPE_ID => macro_four::VALUE_PARAMETERS.len(),
        buttons::TYPE_ID => buttons::OUTPUT_COUNT,
        _ => 1,
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
