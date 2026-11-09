//! Receiver-only, zero-latency effects on isolated shared generator audio.
use crate::{Frame, Invalid, Project, effect_clock::ArrangementEffectClock};
use hex_lattice::{NativeInstrumentClock, NativeRuntimeChain};
use std::{ops::Range, sync::Arc};

pub(crate) mod metadata;

/// Control-prepared recipe retained once per source by the compiled project.
/// Renderers borrow this immutable configuration to build their runtime.
#[derive(Debug, Clone)]
pub(crate) struct PreparedSourceEffects {
    pub(crate) source: hex_project::SourceId,
    pub(crate) receiver: usize,
    pub(crate) instances: Arc<[hex_lattice::PreparedRuntimeInstance]>,
    pub(crate) factory_bytes: usize,
}

pub(crate) fn native_zero_latency(instance: &hex_lattice::PreparedRuntimeInstance) -> bool {
    instance.algorithmic_latency_frames() == Some(0)
        && !matches!(
            instance.definition.as_ref(),
            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
        )
}

fn validate_recipes(project: &Project, recipes: &[PreparedSourceEffects]) -> Result<(), Invalid> {
    if recipes.len() > project.tracks.len() {
        return Err(Invalid("source effect recipe count exceeds receiver count"));
    }
    for (ordinal, recipe) in recipes.iter().enumerate() {
        let index = recipe.receiver;
        let instances = recipe.instances.as_ref();
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
            instance.sample_rate != project.sample_rate()
                || !native_zero_latency(instance)
                || instance.automation.foreign().is_some()
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
    recipes: &[PreparedSourceEffects],
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
        .chain(recipes.iter().flat_map(|recipe| recipe.instances.iter()));
    const OVER_BUDGET: Invalid =
        Invalid("source effects exceed the aggregate graph runtime budget");
    let mut pair_bytes = 0usize;
    let mut stateful = 0;
    for device in effects {
        if let Some(factory) = &device.definition {
            stateful += 1;
            pair_bytes = pair_bytes
                .checked_add(factory.declared_pair_bytes().ok_or(OVER_BUDGET)?)
                .ok_or(OVER_BUDGET)?;
        }
        if let Some(program) = &device.modulation {
            pair_bytes = pair_bytes
                .checked_add(program.quoted_runtime_heap_bytes().ok_or(OVER_BUDGET)?)
                .ok_or(OVER_BUDGET)?;
        }
        if stateful > hex_lattice::MAX_STATEFUL_PROCESSORS
            || pair_bytes > hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2
        {
            return Err(OVER_BUDGET);
        }
    }
    Ok(())
}

struct SourceEffectStage {
    source: hex_project::SourceId,
    chain: NativeRuntimeChain,
}

/// Bounded receiver-to-donor indices, prepared before any history is moved.
pub(crate) struct SourceEffectTransfer {
    donors: [Option<usize>; hex_project::MAX_TRACKS],
}

#[derive(Default)]
pub(crate) struct SourceEffectStages {
    chains: Vec<Option<SourceEffectStage>>,
}

impl SourceEffectStages {
    pub(crate) fn validate_preparation(
        project: &Project,
        recipes: &[PreparedSourceEffects],
    ) -> Result<(), Invalid> {
        if !recipes.is_empty() {
            validate_recipes(project, recipes)?;
            validate_runtime_budget(project, recipes)?;
        }
        Ok(())
    }

    pub(crate) fn prepare(
        project: &Project,
        recipes: &[PreparedSourceEffects],
    ) -> Result<Self, Invalid> {
        if recipes.is_empty() {
            return Ok(Self::default());
        }
        Self::validate_preparation(project, recipes)?;
        let mut chains = Vec::new();
        chains
            .try_reserve_exact(project.tracks.len())
            .map_err(|_| Invalid("source effect runtime allocation failed"))?;
        chains.resize_with(project.tracks.len(), || None);
        for recipe in recipes {
            let chain = NativeRuntimeChain::new_from_source_recipe(&recipe.instances)
                .map_err(|_| Invalid("source effect runtime preparation failed"))?;
            chains[recipe.receiver] = Some(SourceEffectStage {
                source: recipe.source,
                chain,
            });
        }
        Ok(Self { chains })
    }

    /// Ordinary construction quote. The compiled recipes are retained by the
    /// revision; this charges only the exclusive runtime backings.
    pub(crate) fn requested_runtime_bytes(
        receiver_slots: usize,
        recipes: &[PreparedSourceEffects],
    ) -> Option<usize> {
        if recipes.is_empty() {
            return Some(0);
        }
        Self::runtime_bytes(
            receiver_slots,
            recipes.iter().map(|recipe| {
                NativeRuntimeChain::declared_storage_preflight_bytes_for_prepared(&recipe.instances)
            }),
        )
    }

    pub(crate) fn accounted_runtime_bytes(&self) -> Option<usize> {
        Self::runtime_bytes(
            self.chains.capacity(),
            self.chains
                .iter()
                .flatten()
                .map(|stage| stage.chain.accounted_storage_bytes()),
        )
    }

    fn runtime_bytes(
        slots: usize,
        mut chains: impl Iterator<Item = Option<usize>>,
    ) -> Option<usize> {
        let stage_table = slots.checked_mul(std::mem::size_of::<Option<SourceEffectStage>>())?;
        chains.try_fold(stage_table, |total, chain| {
            // Chain headers are already inline in the receiver slots.
            total.checked_add(chain?.checked_sub(std::mem::size_of::<NativeRuntimeChain>())?)
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chains.is_empty()
    }

    /// Match by source identity, independently of the current receiver index.
    /// Live publication may change a compatible recipe's control programs
    /// or attach a fresh stage. An unmatched old stage may retire only when
    /// its host is stopped and the processor proves its committed state quiet.
    pub(crate) fn plan_continuation(
        &self,
        previous: &Self,
        live: bool,
        allow_quiet_retirement: bool,
        generator_matches: impl Fn(hex_project::SourceId) -> bool,
    ) -> Option<SourceEffectTransfer> {
        let mut donors = [None; hex_project::MAX_TRACKS];
        for (index, old) in previous.chains.iter().enumerate() {
            let Some(old) = old else { continue };
            let matching = self
                .chains
                .iter()
                .enumerate()
                .find(|(_, next)| next.as_ref().is_some_and(|next| next.source == old.source));
            if let Some((receiver, Some(next))) = matching
                && next
                    .chain
                    .can_transfer_source_recipe_history_from(&old.chain, live)
                && generator_matches(old.source)
            {
                *donors.get_mut(receiver)? = Some(index);
            } else if !allow_quiet_retirement
                || !old.chain.committed_range_is_quiescent(0..old.chain.len())
            {
                return None;
            }
        }
        if !live
            && self
                .chains
                .iter()
                .zip(&donors)
                .any(|(stage, donor)| stage.is_some() && donor.is_none())
        {
            return None;
        }
        Some(SourceEffectTransfer { donors })
    }

    pub(crate) fn has_continued_source(
        &self,
        transfer: &SourceEffectTransfer,
        source: hex_project::SourceId,
    ) -> bool {
        self.chains
            .iter()
            .zip(&transfer.donors)
            .any(|(stage, donor)| {
                donor.is_some() && stage.as_ref().is_some_and(|stage| stage.source == source)
            })
    }

    /// Processors exchange or copy existing backing; the caller still retires
    /// the old graph on control. An unpaired candidate keeps its fresh history.
    pub(crate) fn continue_from(&mut self, previous: &mut Self, transfer: SourceEffectTransfer) {
        for (stage, donor) in self.chains.iter_mut().zip(transfer.donors) {
            if let (Some(stage), Some(index)) = (stage, donor) {
                let old = previous.chains[index]
                    .as_mut()
                    .expect("source effect handoff preflight matched this donor");
                stage.chain.transfer_history_from(&mut old.chain);
            }
        }
    }

    pub(crate) fn has_monitor_tail(&self) -> bool {
        self.chains.iter().flatten().any(|stage| {
            !stage
                .chain
                .committed_range_is_quiescent(0..stage.chain.len())
        })
    }

    pub(crate) fn has_tail(&self, index: usize) -> bool {
        self.chains
            .get(index)
            .and_then(Option::as_ref)
            .is_some_and(|stage| {
                !stage
                    .chain
                    .committed_range_is_quiescent(0..stage.chain.len())
            })
    }

    pub(crate) fn has_chain(&self, index: usize) -> bool {
        self.chains.get(index).is_some_and(Option::is_some)
    }

    pub(crate) fn chain_for(&self, index: usize, source: u64) -> Option<&NativeRuntimeChain> {
        self.chains
            .get(index)?
            .as_ref()
            .filter(|stage| stage.source.value() == source)
            .map(|stage| &stage.chain)
    }

    pub(crate) fn chain_for_mut(
        &mut self,
        index: usize,
        source: u64,
    ) -> Option<&mut NativeRuntimeChain> {
        self.chains
            .get_mut(index)?
            .as_mut()
            .filter(|stage| stage.source.value() == source)
            .map(|stage| &mut stage.chain)
    }

    pub(crate) fn chains_mut(&mut self) -> impl Iterator<Item = &mut NativeRuntimeChain> {
        self.chains
            .iter_mut()
            .flatten()
            .map(|stage| &mut stage.chain)
    }

    pub(crate) fn has_stateful_processors(&self) -> bool {
        self.chains
            .iter()
            .flatten()
            .any(|stage| stage.chain.has_stateful_processors())
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
        if let Some(stage) = self.chains.get_mut(index).and_then(Option::as_mut) {
            let chain = &mut stage.chain;
            chain
                .process_range_with_clock(0..chain.len(), frames, Some(clock), Some(clock))
                .map_err(|_| Invalid("prepared source effect segment is invalid"))?;
        }
        Ok(())
    }

    pub(crate) fn process_with_live_input(
        &mut self,
        index: usize,
        frames: &mut [Frame],
        clock: &dyn NativeInstrumentClock,
        offset: usize,
        input: Option<&mut crate::native_parameter_input::Processing>,
    ) -> Result<(), Invalid> {
        if let Some(stage) = self.chains.get_mut(index).and_then(Option::as_mut) {
            let chain = &mut stage.chain;
            crate::native_parameter_input::process_range_with_external(
                input,
                None,
                chain,
                0..chain.len(),
                frames,
                offset,
                Some(clock),
                Some(clock),
                0,
                0,
                &mut hex_lattice::RefuseForeign,
                &mut None,
            )?;
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
