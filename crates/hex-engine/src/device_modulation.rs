//! Translate a saved effect bank into a bounded device-local render declaration.
//! Control preparation resolves stable IDs and validates real factory bindings.
//! Prepared declarations borrow no project data and never enter project history.
use hex_lattice::{
    ModulationRoute, ModulationTarget, PreparedModulation, PreparedModulationSource,
    PreparedModulationState, PreparedProcessorFactory,
};
use hex_lattice_contract::{
    NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterModulation,
};
use hex_project::{Device, Invalid, ModulationDestination, ModulationOutputId, Project};

/// Small chunks keep device-local scratch independent of the host callback size.
const CHUNK_FRAMES: usize = 128;

/// State-aware eligibility for native device-panel mapping. Ownership is
/// resolved from the current project. Preparation independently validates the
/// actual factory, runtime binding and source program before publication.
pub fn project_device_modulation_target_supported(
    project: &Project,
    device: hex_project::DeviceId,
    parameter: ParameterId,
) -> bool {
    let Some(device) = crate::device_automation::effect_devices(project)
        .find(|candidate| candidate.id() == device)
    else {
        return false;
    };
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

pub(super) fn validate_source_owners(project: &Project) -> Result<(), Invalid> {
    if project.native_sources().iter().any(|source| {
        source
            .devices()
            .any(|device| !device.modulation().is_empty())
    }) {
        return Err(Invalid("source-owned device modulation is unsupported"));
    }
    Ok(())
}

pub(super) struct EffectModulation {
    sources: Vec<PreparedModulationSource>,
    routes: Vec<ModulationRoute>,
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
            use hex_devices::modulation::{envelope_follower, lfo};
            let prepared = match source.state.type_id() {
                lfo::TYPE_ID => lfo::Settings::decode(&source.state).and_then(|settings| {
                    settings.prepare(source.id, sample_rate, fallback_bpm, maximum_bpm)
                }),
                envelope_follower::TYPE_ID => envelope_follower::Settings::decode(&source.state)
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

        let mut routes = reserved(bank.routes().len())?;
        let mut parameters = reserved(automation.len() + bank.routes().len())?;
        parameters.extend_from_slice(automation);
        for route in bank.routes().iter().filter(|route| route.enabled) {
            let Ok(source) = ids.binary_search(&route.source) else {
                continue;
            };
            let ModulationDestination::Device {
                device_type,
                target,
                vst3_class: None,
            } = route.destination
            else {
                return Err(Invalid("modulation destination is unsupported"));
            };
            let parameter = target
                .provider_parameter()
                .filter(|_| {
                    device_type == device.state().type_id()
                        && route.output == ModulationOutputId::MAIN
                })
                .ok_or(Invalid(
                    "modulation destination or source output is unsupported",
                ))?;
            parameters.push(parameter);
            routes.push(ModulationRoute {
                source,
                target: parameter,
                amount: route.amount.get(),
                range: [route.range.start.get(), route.range.end.get()],
            });
        }
        parameters.sort_unstable();
        parameters.dedup();
        Ok(Some(Self {
            sources,
            routes,
            parameters,
        }))
    }

    /// Retained declaration and temporary preparation backing. Include every
    /// possible provider automation slot, including live and external inputs,
    /// so arming a parameter never needs a larger modulation reservation.
    pub(super) fn quote(device: &Device) -> Option<(usize, usize)> {
        let bank = device.modulation();
        Self::quote_counts(bank.sources().len(), bank.routes().len(), 1)
    }

    /// The same estimate applies to summed source and route counts when
    /// reserving a persistent host for the supported project cardinalities.
    pub(super) fn quote_counts(
        sources: usize,
        routes: usize,
        banks: usize,
    ) -> Option<(usize, usize)> {
        use hex_lattice_contract::{MAX_PARAMETERS_PER_DEVICE, ModulationSourceId};
        use hex_project::Modulator;
        use std::mem::size_of;
        if sources == 0 {
            return Some((0, 0));
        }
        let targets = banks.checked_mul(MAX_PARAMETERS_PER_DEVICE)?;
        let retained = PreparedModulationState::quoted_heap_bytes(sources, targets, routes)?;
        let overlap = sources
            .checked_mul(size_of::<&Modulator>() + size_of::<ModulationSourceId>())?
            .checked_add(
                routes.checked_mul(size_of::<ModulationRoute>() + size_of::<ParameterId>())?,
            )?
            .checked_add(
                targets.checked_mul(size_of::<ParameterId>() + size_of::<ModulationTarget>())?,
            )?;
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
        let mut targets = reserved(self.parameters.len())?;
        for parameter in self.parameters {
            let unsupported =
                || Invalid("processor does not support sample-rate modulation for this parameter");
            let factory = factory.ok_or_else(unsupported)?;
            let descriptor = factory
                .parameter_automation_descriptor(parameter)
                .filter(|_| factory.supports_parameter_automation(parameter))
                .ok_or_else(unsupported)?;
            let semantics = if self.routes.iter().any(|route| route.target == parameter) {
                factory
                    .parameter_modulation_semantics(parameter)
                    .filter(|semantics| semantics.modulation == ParameterModulation::SampleRate)
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
        let program = PreparedModulation::new(&targets, &self.routes, self.sources.len())
            .map_err(|_| Invalid("modulation program is invalid or exceeds its capacity"))?;
        PreparedModulationState::new(self.sources, program, CHUNK_FRAMES)
            .map_err(|_| Invalid("modulation declaration exceeds its capacity"))
    }
}

fn reserved<T>(count: usize) -> Result<Vec<T>, Invalid> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve modulation preparation storage"))?;
    Ok(values)
}
