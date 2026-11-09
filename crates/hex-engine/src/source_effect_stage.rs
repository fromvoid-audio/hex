//! Receiver-only, zero-latency effects on isolated shared generator audio.
use crate::{Frame, Invalid, Project, effect_clock::ArrangementEffectClock};
use hex_lattice::{NativeInstrumentClock, NativeRuntimeChain};
use std::ops::Range;

/// Borrowed control-side recipe. Placement is validated against the compiled
/// receiver; the renderer takes custody only of its prepared runtime.
pub(crate) struct PreparedSourceEffects<'a> {
    pub(crate) source: hex_project::SourceId,
    pub(crate) receiver: usize,
    pub(crate) instances: &'a [hex_lattice::PreparedRuntimeInstance],
}

pub(crate) fn native_zero_latency(instance: &hex_lattice::PreparedRuntimeInstance) -> bool {
    instance.algorithmic_latency_frames() == Some(0)
        && !matches!(
            instance.definition.as_ref(),
            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
        )
}

fn validate_recipes(
    project: &Project,
    recipes: &[PreparedSourceEffects<'_>],
) -> Result<(), Invalid> {
    if recipes.len() > project.tracks.len() {
        return Err(Invalid("source effect recipe count exceeds receiver count"));
    }
    for (ordinal, recipe) in recipes.iter().enumerate() {
        let index = recipe.receiver;
        let instances = recipe.instances;
        let track = project
            .tracks
            .get(index)
            .ok_or(Invalid("source effect receiver is missing"))?;
        if track.source_id() != Some(recipe.source)
            || !track.receives_source_audio()
            || track.source_receiver_index() != Some(index)
        {
            return Err(Invalid("source effects require the explicit receiver"));
        }
        if recipes[..ordinal]
            .iter()
            .any(|prior| prior.source == recipe.source)
        {
            return Err(Invalid("source effects already have a receiver stage"));
        }
        if instances.is_empty() || instances.len() > hex_project::MAX_DEVICES_PER_TRACK {
            return Err(Invalid("source effect count is outside the bounded chain"));
        }
        let owner = hex_lattice::ProcessorChainOwner::Source(recipe.source.value());
        if instances.iter().any(|instance| instance.owner != owner) {
            return Err(Invalid("source effect processor belongs to another owner"));
        }
        if track.instrument_input_gain != 1.0 || track.instrument_output_gain != 1.0 {
            return Err(Invalid(
                "source effect recipes require neutral instrument trims",
            ));
        }
        if instances.iter().any(|instance| {
            instance.sample_rate != project.sample_rate() || !native_zero_latency(instance)
        }) {
            return Err(Invalid(
                "source effects require native zero-latency processors at the project rate",
            ));
        }
    }
    Ok(())
}

/// Source-owned pairs share the existing aggregate effect limits. Complete this
/// control-side check before constructing any runtime, including earlier valid
/// recipes in a batch with a later invalid or over-budget source.
fn validate_runtime_budget(
    project: &Project,
    recipes: &[PreparedSourceEffects<'_>],
) -> Result<(), Invalid> {
    let effects = project
        .tracks
        .iter()
        .flat_map(|track| track.device_chain.iter())
        .chain(
            project
                .return_buses
                .iter()
                .flat_map(|bus| bus.device_chain.iter()),
        )
        .chain(project.main_device_chain.iter())
        .chain(recipes.iter().flat_map(|recipe| recipe.instances));
    const OVER_BUDGET: Invalid =
        Invalid("source effects exceed the aggregate graph runtime budget");
    let mut pair_bytes = 0usize;
    for (index, factory) in effects
        .filter_map(|device| device.definition.as_ref())
        .enumerate()
    {
        pair_bytes = pair_bytes
            .checked_add(factory.declared_pair_bytes().ok_or(OVER_BUDGET)?)
            .ok_or(OVER_BUDGET)?;
        if index >= hex_lattice::MAX_STATEFUL_PROCESSORS
            || pair_bytes > hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2
        {
            return Err(OVER_BUDGET);
        }
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct SourceEffectStages {
    chains: Vec<Option<NativeRuntimeChain>>,
}

impl SourceEffectStages {
    pub(crate) fn prepare(
        project: &Project,
        recipes: &[PreparedSourceEffects<'_>],
    ) -> Result<Self, Invalid> {
        if recipes.is_empty() {
            return Ok(Self::default());
        }
        validate_recipes(project, recipes)?;
        validate_runtime_budget(project, recipes)?;
        let mut chains = Vec::new();
        chains
            .try_reserve_exact(project.tracks.len())
            .map_err(|_| Invalid("source effect runtime allocation failed"))?;
        chains.resize_with(project.tracks.len(), || None);
        for recipe in recipes {
            let instances = recipe.instances;
            let mut prepared = Vec::new();
            prepared
                .try_reserve_exact(instances.len())
                .map_err(|_| Invalid("source effect recipe allocation failed"))?;
            prepared.extend(instances.iter().cloned());
            let chain = NativeRuntimeChain::new_prepared(prepared)
                .map_err(|_| Invalid("source effect runtime preparation failed"))?;
            chains[recipe.receiver] = Some(chain);
        }
        Ok(Self { chains })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chains.is_empty()
    }

    pub(crate) fn has_monitor_tail(&self) -> bool {
        self.chains
            .iter()
            .flatten()
            .any(|chain| !chain.committed_range_is_quiescent(0..chain.len()))
    }

    pub(crate) fn has_tail(&self, index: usize) -> bool {
        self.chains
            .get(index)
            .and_then(Option::as_ref)
            .is_some_and(|chain| !chain.committed_range_is_quiescent(0..chain.len()))
    }

    pub(crate) fn has_chain(&self, index: usize) -> bool {
        self.chains.get(index).is_some_and(Option::is_some)
    }

    pub(crate) fn chains_mut(&mut self) -> impl Iterator<Item = &mut NativeRuntimeChain> {
        self.chains.iter_mut().flatten()
    }

    pub(crate) fn has_stateful_processors(&self) -> bool {
        self.chains
            .iter()
            .flatten()
            .any(NativeRuntimeChain::has_stateful_processors)
    }

    pub(crate) fn process_isolated(
        &mut self,
        project: &Project,
        index: usize,
        start: u64,
        frames: &mut [Frame],
    ) -> Result<(), Invalid> {
        let clock = ArrangementEffectClock::canonical(project.tempo_map.as_ref(), start);
        self.process_with_clock(index, frames, &clock)
    }

    pub(crate) fn process_with_clock(
        &mut self,
        index: usize,
        frames: &mut [Frame],
        clock: &dyn NativeInstrumentClock,
    ) -> Result<(), Invalid> {
        if let Some(chain) = self.chains.get_mut(index).and_then(Option::as_mut) {
            chain
                .process_range_with_clock(0..chain.len(), frames, Some(clock), Some(clock))
                .map_err(|_| Invalid("prepared source effect segment is invalid"))?;
        }
        Ok(())
    }

    pub(crate) fn add_generator(
        &mut self,
        project: &Project,
        index: usize,
        range: Range<u64>,
        output: &mut [Frame],
        timing: Option<&dyn crate::prepared_tempo::MidiTempoBreaks>,
    ) -> Result<(), Invalid> {
        if !self.has_chain(index) {
            // Preserve the existing destination summation order exactly.
            return crate::render_source_generators(
                project,
                index,
                range.start,
                range.end,
                output,
                timing,
            );
        }
        let mut scratch = [[0.0; 2]; 256];
        let source = scratch
            .get_mut(..output.len())
            .ok_or(Invalid("source effect segment exceeds render quantum"))?;
        crate::render_source_generators(project, index, range.start, range.end, source, timing)?;
        self.process_isolated(project, index, range.start, source)?;
        for (target, source) in output.iter_mut().zip(source) {
            target[0] += source[0];
            target[1] += source[1];
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
