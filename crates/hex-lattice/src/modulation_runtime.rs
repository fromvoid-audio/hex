//! Transactional, borrowed delivery of a prepared source bank and route program.
//!
//! Construction, admission, and destruction belong to control. The enclosing
//! graph owns transport policy, source-index resolution, and block transactions.
//! A failed consumer abandons the entire source transaction; later host failures
//! must call `rollback` before beginning the next block.
use crate::{
    MAX_MODULATION_BLOCK_FRAMES, MAX_MODULATION_SOURCES, ModulationBlock, ModulationSourceBank,
    ModulationSourceId, NativeParameterAutomation, PreparedModulation, PreparedModulationSource,
    modulation_source_routes::PreparedSourceRoute,
};
use hex_lattice_contract::DeviceContractError as Error;

/// A device-local bank and program. Route indices refer to the bank's sorted
/// source IDs; the preparing host must resolve and validate those identities.
pub struct ModulationRuntime {
    bank: ModulationSourceBank,
    program: PreparedModulation,
    source_routes: Vec<PreparedSourceRoute>,
}

impl ModulationRuntime {
    /// Refuse incompatible inventories before either prepared object is used.
    pub fn new(bank: ModulationSourceBank, program: PreparedModulation) -> Result<Self, Error> {
        Self::with_prepared_source_routes(bank, program, Vec::new())
    }

    pub(crate) fn with_prepared_source_routes(
        bank: ModulationSourceBank,
        program: PreparedModulation,
        source_routes: Vec<PreparedSourceRoute>,
    ) -> Result<Self, Error> {
        if bank.source_count() != program.source_count()
            || bank.source_count() > MAX_MODULATION_SOURCES
            || bank.chunk_frames() > MAX_MODULATION_BLOCK_FRAMES
        {
            return Err(Error::InvalidDescriptor);
        }
        Ok(Self {
            bank,
            program,
            source_routes,
        })
    }

    pub(crate) fn matches_prepared(
        &self,
        sources: &[PreparedModulationSource],
        program: &PreparedModulation,
        source_routes: &[PreparedSourceRoute],
        chunk_frames: usize,
    ) -> bool {
        self.program == *program
            && self.source_routes == source_routes
            && self.bank.matches_prepared(sources, chunk_frames)
    }

    /// Physical inline and reserved owned storage, excluding allocator metadata.
    pub fn storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>().checked_add(self.heap_storage_bytes()?)
    }

    pub fn heap_storage_bytes(&self) -> Option<usize> {
        self.bank
            .storage_bytes()
            .ok()?
            .checked_add(self.program.storage_bytes()?)?
            .checked_add(
                self.source_routes
                    .capacity()
                    .checked_mul(std::mem::size_of::<PreparedSourceRoute>())?,
            )
    }

    /// Borrow the prepared program for host admission against the real provider.
    pub fn program(&self) -> &PreparedModulation {
        &self.program
    }

    /// Inspect the bank's sorted source identity at an admitted route index.
    pub fn source_id(&self, index: usize) -> Option<ModulationSourceId> {
        self.bank.source_id(index)
    }

    /// Maximum frames accepted by one synchronous render call.
    pub fn chunk_frames(&self) -> usize {
        self.bank.chunk_frames()
    }

    pub fn begin(&mut self) {
        self.bank.begin();
    }

    /// Render one bounded chunk and lend its values to the synchronous consumer.
    /// On consumer or composition refusal, the whole transaction is abandoned.
    pub fn render_chunk(
        &mut self,
        frames: usize,
        upstream: Option<&dyn NativeParameterAutomation>,
        consume: impl FnOnce(&dyn NativeParameterAutomation) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.render_chunk_with_clock(frames, upstream, None, consume)
    }

    /// Render with the host's sample-local tempo; unavailable tempo uses each
    /// source's prepared fallback. The clock is borrowed only during this call.
    /// Refused tempo leaves the bank unchanged so the host can retry or roll back.
    pub fn render_chunk_with_clock(
        &mut self,
        frames: usize,
        upstream: Option<&dyn NativeParameterAutomation>,
        clock: Option<&dyn crate::NativeInstrumentClock>,
        consume: impl FnOnce(&dyn NativeParameterAutomation) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.render_sources(frames, upstream, clock, None)?;
        self.consume_values(frames, upstream, |values| consume(values))
    }

    /// Render against the input immediately before the owning device consumes
    /// it. The same mutable borrow is handed to the provider after all source
    /// reads complete. No additional PCM copy or ownership is needed.
    pub(crate) fn render_audio_chunk(
        &mut self,
        input: &mut [crate::StereoFrame],
        upstream: Option<&dyn NativeParameterAutomation>,
        clock: Option<&dyn crate::NativeInstrumentClock>,
        consume: impl FnOnce(&mut [crate::StereoFrame], &ModulationBlock<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let frames = input.len();
        self.render_sources(frames, upstream, clock, Some(input))?;
        self.consume_values(frames, upstream, |values| consume(input, values))
    }

    fn render_sources(
        &mut self,
        frames: usize,
        upstream: Option<&dyn NativeParameterAutomation>,
        clock: Option<&dyn crate::NativeInstrumentClock>,
        input: Option<&[crate::StereoFrame]>,
    ) -> Result<(), Error> {
        if frames > self.bank.chunk_frames() || frames > MAX_MODULATION_BLOCK_FRAMES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        if let Some(upstream) = upstream {
            let ids = upstream.parameter_ids();
            if ids.windows(2).any(|pair| pair[0] >= pair[1])
                || ids
                    .iter()
                    .any(|id| self.program.parameter_ids().binary_search(id).is_err())
            {
                return Err(Error::UnknownParameter);
            }
        }
        self.bank.render_chunk_with_input(frames, clock, input)?;
        let result = self.bank.apply_source_routes(&self.source_routes);
        if result.is_err() {
            self.bank.rollback();
        }
        result
    }

    fn consume_values(
        &mut self,
        frames: usize,
        upstream: Option<&dyn NativeParameterAutomation>,
        consume: impl FnOnce(&ModulationBlock<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let result = (|| {
            let mut sources: [&[f32]; MAX_MODULATION_SOURCES] = [&[]; MAX_MODULATION_SOURCES];
            for (index, slot) in sources
                .iter_mut()
                .take(self.bank.source_count())
                .enumerate()
            {
                let id = self.bank.source_id(index).ok_or(Error::InvalidId)?;
                *slot = self.bank.samples(id).ok_or(Error::InvalidParameterValue)?;
            }
            let source_block = sources
                .get(..self.bank.source_count())
                .ok_or(Error::RuntimeBudgetExceeded)?;
            let block = self.program.block(upstream, source_block, frames)?;
            consume(&block)
        })();
        if result.is_err() {
            self.bank.rollback();
        }
        result
    }

    pub fn commit(&mut self) -> Result<(), Error> {
        self.bank.commit()
    }

    pub fn rollback(&mut self) {
        self.bank.rollback();
    }

    pub(crate) fn reset_all(&mut self) {
        self.bank.reset_all();
    }

    /// Retarget an active source without resetting its speculative history.
    pub fn retarget(&mut self, source: PreparedModulationSource) -> Result<(), Error> {
        self.bank.retarget(source)
    }

    /// Explicitly reset an oscillator phase outside a transaction.
    pub fn reset(&mut self, id: ModulationSourceId, phase: f64) -> Result<(), Error> {
        self.bank.reset(id, phase)
    }

    pub(crate) fn inherit_graph_committed(&mut self, previous: &Self) {
        self.bank.inherit_graph_committed(&previous.bank);
    }

    /// The host establishes matching device ownership before calling this.
    pub fn inherit_committed(&mut self, previous: &Self) -> Result<(), Error> {
        self.bank.inherit_committed(&previous.bank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModulationRoute, ModulationSourceId, ModulationTarget, PreparedModulationSource};
    use hex_lattice_contract::{
        ParameterAccess, ParameterAutomation, ParameterChangeImpact, ParameterDescriptor,
        ParameterId, ParameterKind, ParameterModulation, ParameterNormalization,
        ParameterPresentation, ParameterSemantics, ParameterSmoothing, ParameterStepping,
        ParameterValue,
    };

    fn prepared(second_rate: f64) -> crate::PreparedModulationState {
        let id = ParameterId::from_static(1);
        let target = ModulationTarget {
            descriptor: ParameterDescriptor {
                id,
                label: "test",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 1.0,
                    unit: None,
                },
                default: ParameterValue::Float(0.5),
                presentation: ParameterPresentation::Linear,
            },
            semantics: ParameterSemantics {
                access: ParameterAccess::ReadWrite,
                normalization: ParameterNormalization::Linear,
                stepping: ParameterStepping::Continuous,
                automation: ParameterAutomation::Unsupported,
                modulation: ParameterModulation::Unsupported,
                smoothing: ParameterSmoothing::None,
                change_impact: ParameterChangeImpact::ValueOnly,
            },
            base: ParameterValue::Float(0.5),
        };
        let routes = [
            ModulationRoute {
                source: 0,
                target: id,
                amount: 0.1,
                range: [-1.0, 1.0],
            },
            ModulationRoute {
                source: 1,
                target: id,
                amount: 0.1,
                range: [-1.0, 1.0],
            },
        ];
        let program = PreparedModulation::new(&[target], &routes, 2).unwrap();
        let sources = [
            PreparedModulationSource::free(
                ModulationSourceId::new(1).unwrap(),
                48_000.0,
                12_000.0,
                0.0,
            )
            .unwrap(),
            PreparedModulationSource::free(
                ModulationSourceId::new(2).unwrap(),
                48_000.0,
                second_rate,
                0.0,
            )
            .unwrap(),
        ];
        crate::PreparedModulationState::new(sources.to_vec(), program, 8).unwrap()
    }

    fn runtime(second_rate: f64) -> ModulationRuntime {
        prepared(second_rate).prepare().unwrap()
    }

    fn read(runtime: &mut ModulationRuntime, output: &mut [f32]) -> Result<(), Error> {
        runtime.render_chunk(output.len(), None, |block| {
            for (index, value) in output.iter_mut().enumerate() {
                let Some(ParameterValue::Float(v)) =
                    block.value_at(ParameterId::from_static(1), index)
                else {
                    return Err(Error::InvalidParameterValue);
                };
                *value = v;
            }
            Ok(())
        })
    }

    #[test]
    fn refusal_and_retry_preserve_both_sources_and_handoff_rates() {
        let mut runtime = runtime(6_000.0);
        runtime.begin();
        let mut first = [0.0; 3];
        read(&mut runtime, &mut first).unwrap();
        assert!((first[0] - 0.5).abs() < 1e-6);
        assert!((first[1] - (0.6 + 0.1 * std::f32::consts::FRAC_1_SQRT_2)).abs() < 1e-6);
        assert!((first[2] - 0.6).abs() < 1e-6);
        assert!(
            runtime
                .render_chunk(1, None, |_| Err(Error::InvalidParameterValue))
                .is_err()
        );
        assert!(runtime.commit().is_err());
        runtime.begin();
        let mut retry = [0.0; 3];
        read(&mut runtime, &mut retry).unwrap();
        assert_eq!(retry, first);
        runtime.commit().unwrap();
        let mut replacement = self::runtime(12_000.0);
        replacement.inherit_committed(&runtime).unwrap();
        replacement.begin();
        let mut after = [0.0; 2];
        read(&mut replacement, &mut after).unwrap();
        assert!((after[0] - (0.4 + 0.1 * std::f32::consts::FRAC_1_SQRT_2)).abs() < 1e-6);
        assert!((after[1] - (0.5 - 0.1 * std::f32::consts::FRAC_1_SQRT_2)).abs() < 1e-6);
    }

    #[test]
    fn oversized_chunks_and_mismatched_inventory_are_refused() {
        let mut current = runtime(6_000.0);
        assert_eq!(current.chunk_frames(), 8);
        current.begin();
        assert!(current.render_chunk(9, None, |_| Ok(())).is_err());
        let mut output = [0.0; 1];
        read(&mut current, &mut output).unwrap();
        assert_eq!(output[0], 0.5);
        let second = ModulationSourceId::new(2).unwrap();
        let faster = PreparedModulationSource::free(second, 48_000.0, 12_000.0, 0.0).unwrap();
        current.retarget(faster).unwrap();
        read(&mut current, &mut output).unwrap();
        assert!((output[0] - (0.6 + 0.1 * std::f32::consts::FRAC_1_SQRT_2)).abs() < 1e-6);
        current.rollback();
        current
            .reset(ModulationSourceId::new(1).unwrap(), 0.25)
            .unwrap();
        current.begin();
        read(&mut current, &mut output).unwrap();
        assert!((output[0] - 0.6).abs() < 1e-6);
        let empty = ModulationSourceBank::new(&[], 8).unwrap();
        let two_sources = PreparedModulation::new(&[], &[], 2).unwrap();
        assert!(ModulationRuntime::new(empty, two_sources).is_err());
    }

    #[test]
    fn exposed_chunk_limit_allows_a_long_segment_without_phase_gaps() {
        let mut current = runtime(6_000.0);
        let mut output = [0.0; 19];
        let limit = current.chunk_frames();
        current.begin();
        for chunk in output.chunks_mut(limit) {
            read(&mut current, chunk).unwrap();
        }
        current.commit().unwrap();
        for (frame, actual) in output.into_iter().enumerate() {
            let phase = frame as f32 * std::f32::consts::TAU;
            let expected = 0.5 + 0.1 * ((phase / 4.0).sin() + (phase / 8.0).sin());
            assert!((actual - expected).abs() < 1e-6);
        }
    }
    #[test]
    fn one_prepared_declaration_builds_independent_source_transactions() {
        let prepared = prepared(6_000.0);
        let mut first = prepared.prepare().unwrap();
        let mut second = prepared.prepare().unwrap();
        assert!(prepared.storage_bytes().unwrap() > std::mem::size_of_val(&prepared));
        assert_eq!(
            prepared.program().parameter_ids(),
            &[ParameterId::from_static(1)]
        );
        first.begin();
        let mut advanced = [0.0; 3];
        read(&mut first, &mut advanced).unwrap();
        first.commit().unwrap();
        second.begin();
        let mut fresh = [0.0; 3];
        read(&mut second, &mut fresh).unwrap();
        assert_eq!(advanced, fresh);
        assert!(
            crate::PreparedModulationState::new(
                Vec::new(),
                PreparedModulation::new(&[], &[], 1).unwrap(),
                8
            )
            .is_err()
        );
        let retained_capacity = crate::prepared_modulation::MAX_PREPARED_MODULATION_BYTES
            / std::mem::size_of::<PreparedModulationSource>()
            + 1;
        let empty_with_capacity = Vec::with_capacity(retained_capacity);
        assert!(
            crate::PreparedModulationState::new(
                empty_with_capacity,
                PreparedModulation::new(&[], &[], 0).unwrap(),
                8
            )
            .is_err()
        );
    }
}
