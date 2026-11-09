//! Transactional, borrowed delivery of prepared source outputs and routes.
//!
//! Construction, admission, and destruction belong to control. The enclosing
//! graph owns transport policy, source-index resolution, and block transactions.
//! A failed consumer abandons the entire source transaction; later host failures
//! must call `rollback` before beginning the next block.
use crate::{
    MAX_MODULATION_BLOCK_FRAMES, MAX_MODULATION_SOURCES, ModulationBlock, ModulationSourceBank,
    ModulationSourceId, NativeInstrumentEvent, NativeParameterAutomation, PreparedModulation,
    PreparedModulationSource,
    modulation_source_routes::{PreparedSourceRoute, target_mask},
};
use hex_lattice_contract::DeviceContractError as Error;

/// A device-local bank and program. Route indices name sorted source/output
/// pairs that the preparing host resolves and validates.
pub struct ModulationRuntime {
    bank: ModulationSourceBank,
    program: PreparedModulation,
    source_routes: Vec<PreparedSourceRoute>,
    routed_targets: u32,
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
            routed_targets: target_mask(&source_routes),
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

    pub(crate) fn same_declaration(&self, other: &Self) -> bool {
        self.program == other.program
            && self.source_routes == other.source_routes
            && self.bank.same_declaration(&other.bank)
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

    pub(crate) fn follows_input(&self) -> bool {
        self.bank.follows_input()
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
        self.render_sources(frames, upstream, clock, None, None)?;
        self.consume_values(frames, upstream, |values| consume(values))
    }

    /// Render against the ordinary instrument host's current event batch.
    /// Offsets are relative to this chunk. The same ordered events are
    /// passed to the instrument consumer synchronously.
    pub fn render_instrument_chunk(
        &mut self,
        frames: usize,
        upstream: Option<&dyn NativeParameterAutomation>,
        clock: Option<&dyn crate::NativeInstrumentClock>,
        events: &[NativeInstrumentEvent],
        consume: impl FnOnce(&dyn NativeParameterAutomation) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.render_sources(frames, upstream, clock, None, Some(events))?;
        self.consume_values(frames, upstream, |values| consume(values))
    }

    /// Render against the owning device's input tap. A modulated input gain
    /// uses the independently evaluated, unmodulated tap to avoid feedback.
    /// Otherwise the same mutable block goes to the provider after source reads.
    pub(crate) fn render_audio_chunk(
        &mut self,
        input: &mut [crate::StereoFrame],
        tap: Option<&[crate::StereoFrame]>,
        upstream: Option<&dyn NativeParameterAutomation>,
        clock: Option<&dyn crate::NativeInstrumentClock>,
        consume: impl FnOnce(&mut [crate::StereoFrame], &ModulationBlock<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let frames = input.len();
        self.render_sources(frames, upstream, clock, tap.or(Some(input)), None)?;
        self.consume_values(frames, upstream, |values| consume(input, values))
    }

    fn render_sources(
        &mut self,
        frames: usize,
        upstream: Option<&dyn NativeParameterAutomation>,
        clock: Option<&dyn crate::NativeInstrumentClock>,
        input: Option<&[crate::StereoFrame]>,
        note_events: Option<&[NativeInstrumentEvent]>,
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
        self.bank.render_chunk_with_routes(
            frames,
            clock,
            input,
            note_events,
            &self.source_routes,
            self.routed_targets,
        )
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
                *slot = self
                    .bank
                    .samples_at_index(index)
                    .ok_or(Error::InvalidParameterValue)?;
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

    /// Reset every source to its prepared initial history at a transport discontinuity.
    pub fn reset_all(&mut self) {
        self.bank.reset_all();
    }

    /// Retarget an active source without resetting its speculative history.
    pub fn retarget(&mut self, source: PreparedModulationSource) -> Result<(), Error> {
        if self.source_routes.iter().any(|route| {
            self.bank.source_key(route.target) == Some(source.key())
                && !source.supports_modulation_parameter(route.parameter)
        }) {
            return Err(Error::UnsupportedProcessingClass);
        }
        self.bank.retarget(source)
    }

    /// Explicitly reset an oscillator phase outside a transaction.
    pub fn reset(&mut self, id: ModulationSourceId, phase: f64) -> Result<(), Error> {
        self.bank.reset(id, phase)
    }

    /// Carry compatible committed source IDs across a graph edit. The host
    /// proves stable device ownership before borrowing the old bank.
    pub fn inherit_graph_committed(&mut self, previous: &Self) {
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
    fn two_outputs_of_one_source_keep_distinct_routes_and_committed_phase() {
        use hex_lattice_contract::ModulationOutputId;

        let fixture = prepared(6_000.0);
        let first_id = ParameterId::from_static(1);
        let second_id = ParameterId::from_static(2);
        let first = ModulationTarget {
            descriptor: fixture.program().descriptor(first_id).unwrap(),
            semantics: fixture.program().semantics(first_id).unwrap(),
            base: ParameterValue::Float(0.5),
        };
        let second = ModulationTarget {
            descriptor: ParameterDescriptor {
                id: second_id,
                ..first.descriptor
            },
            ..first
        };
        let route = |source, target| ModulationRoute {
            source,
            target,
            amount: 0.25,
            range: [-1.0, 1.0],
        };
        let program = PreparedModulation::new(
            &[first, second],
            &[route(0, first_id), route(1, second_id)],
            2,
        )
        .unwrap();
        let id = ModulationSourceId::new(u64::MAX).unwrap();
        let source = |phase| {
            PreparedModulationSource::free_with_waveform(
                id,
                48_000.0,
                12_000.0,
                phase,
                crate::LfoWaveform::Square,
            )
            .unwrap()
        };
        let right = ModulationOutputId::new(2).unwrap();
        let left_source = source(0.0);
        let right_source = source(0.25).with_output(right);
        assert!(ModulationSourceBank::new(&[left_source, left_source], 8).is_err());
        assert!(ModulationSourceBank::new(&[right_source, left_source], 8).is_err());
        let declaration =
            crate::PreparedModulationState::new(vec![left_source, right_source], program, 8)
                .unwrap();
        let mut runtime = declaration.prepare().unwrap();
        let mut successor = declaration.prepare().unwrap();
        let verify = |runtime: &mut ModulationRuntime, expected: &[[f32; 2]]| {
            runtime
                .render_chunk(expected.len(), None, |block| {
                    for (offset, [left, right]) in expected.iter().copied().enumerate() {
                        assert_eq!(
                            block.value_at(first_id, offset),
                            Some(ParameterValue::Float(left))
                        );
                        assert_eq!(
                            block.value_at(second_id, offset),
                            Some(ParameterValue::Float(right))
                        );
                    }
                    Ok(())
                })
                .unwrap();
        };
        let first = [[0.75, 0.75], [0.75, 0.25], [0.25, 0.25]];
        runtime.begin();
        verify(&mut runtime, &first);
        assert_eq!(runtime.bank.samples(id), Some([1.0, 1.0, -1.0].as_slice()));
        assert_eq!(
            runtime.bank.samples_for_output(id, right),
            Some([1.0, -1.0, -1.0].as_slice())
        );
        runtime.rollback();
        runtime.begin();
        let missing = source(0.0).with_output(ModulationOutputId::new(3).unwrap());
        assert_eq!(runtime.retarget(missing), Err(Error::InvalidId));
        verify(&mut runtime, &first);
        runtime.commit().unwrap();
        successor.inherit_committed(&runtime).unwrap();
        successor.begin();
        verify(&mut successor, &[[0.25, 0.75], [0.75, 0.75]]);
        successor.commit().unwrap();
    }

    #[test]
    fn two_math_inputs_follow_distinct_upstream_samples_and_reset_as_a_bank() {
        use crate::{LfoWaveform, ModulationMathOperation as Op, SourceModulationParameter};

        let fixture = prepared(6_000.0);
        let id = ParameterId::from_static(1);
        let target = ModulationTarget {
            descriptor: fixture.program().descriptor(id).unwrap(),
            semantics: fixture.program().semantics(id).unwrap(),
            base: ParameterValue::Float(0.5),
        };
        let program = PreparedModulation::new(
            &[target],
            &[ModulationRoute {
                source: 1,
                target: id,
                amount: 0.25,
                range: [-1.0, 1.0],
            }],
            3,
        )
        .unwrap();
        let stable_id = |value| ModulationSourceId::new(value).unwrap();
        let square = |value, phase| {
            PreparedModulationSource::free_with_waveform(
                stable_id(value),
                48_000.0,
                12_000.0,
                phase,
                LfoWaveform::Square,
            )
            .unwrap()
        };
        let math = PreparedModulationSource::math(stable_id(2), 48_000, [0.0, 0.25], Op::Subtract)
            .unwrap();
        let incoming = [
            crate::SourceModulationRoute {
                source: 2,
                target: 1,
                parameter: SourceModulationParameter::MathB,
                amount: 0.125,
                range: [-1.0, 1.0],
            },
            crate::SourceModulationRoute {
                source: 0,
                target: 1,
                parameter: SourceModulationParameter::MathA,
                amount: 0.25,
                range: [-1.0, 1.0],
            },
        ];
        let sources = vec![square(1, 0.0), math, square(3, 0.25)];
        let mut invalid = incoming;
        invalid[0].parameter = SourceModulationParameter::MacroValue;
        assert!(
            crate::PreparedModulationState::with_source_routes(
                sources.clone(),
                program.clone(),
                8,
                &invalid,
            )
            .is_err()
        );
        let declaration =
            crate::PreparedModulationState::with_source_routes(sources, program, 8, &incoming)
                .unwrap();
        let mut runtime = declaration.prepare().unwrap();
        let check = |runtime: &mut ModulationRuntime, expected: &[f32]| {
            runtime
                .render_chunk(expected.len(), None, |block| {
                    for (offset, &sample) in expected.iter().enumerate() {
                        assert_eq!(
                            block.value_at(id, offset),
                            Some(ParameterValue::Float(sample))
                        );
                    }
                    Ok(())
                })
                .unwrap();
        };
        runtime.begin();
        check(&mut runtime, &[0.5, 0.625, 0.375]);
        runtime.rollback();
        runtime.begin();
        check(&mut runtime, &[0.5, 0.625, 0.375]);
        runtime.commit().unwrap();
        let mut next = declaration.prepare().unwrap();
        next.inherit_committed(&runtime).unwrap();
        next.begin();
        check(&mut next, &[0.25, 0.5, 0.625]);
        next.commit().unwrap();
        next.reset_all();
        next.begin();
        check(&mut next, &[0.5, 0.625, 0.375, 0.25]);
        next.commit().unwrap();
    }

    #[test]
    fn three_mix_inputs_follow_distinct_samples_across_splits_and_reset() {
        use crate::{LfoWaveform, SourceModulationParameter, SourceModulationRoute};

        let fixture = prepared(6_000.0);
        let id = ParameterId::from_static(1);
        let target = ModulationTarget {
            descriptor: fixture.program().descriptor(id).unwrap(),
            semantics: fixture.program().semantics(id).unwrap(),
            base: ParameterValue::Float(0.5),
        };
        let program = PreparedModulation::new(
            &[target],
            &[ModulationRoute {
                source: 1,
                target: id,
                amount: 0.25,
                range: [-1.0, 1.0],
            }],
            4,
        )
        .unwrap();
        let stable_id = |value| ModulationSourceId::new(value).unwrap();
        let square = |value, phase| {
            PreparedModulationSource::free_with_waveform(
                stable_id(value),
                48_000.0,
                12_000.0,
                phase,
                LfoWaveform::Square,
            )
            .unwrap()
        };
        let mix = PreparedModulationSource::mix(stable_id(2), 48_000, [-0.25, 0.5], 0.5).unwrap();
        let sources = vec![square(1, 0.0), mix, square(3, 0.25), square(4, 0.5)];
        let incoming = [
            (3, SourceModulationParameter::MixBlend, 0.25),
            (0, SourceModulationParameter::MixA, 0.5),
            (2, SourceModulationParameter::MixB, 0.25),
        ]
        .map(|(source, parameter, amount)| SourceModulationRoute {
            source,
            target: 1,
            parameter,
            amount,
            range: [-1.0, 1.0],
        });
        let mut invalid = incoming;
        invalid[1].parameter = SourceModulationParameter::MathA;
        assert!(
            crate::PreparedModulationState::with_source_routes(
                sources.clone(),
                program.clone(),
                8,
                &invalid,
            )
            .is_err()
        );
        let declaration =
            crate::PreparedModulationState::with_source_routes(sources, program, 8, &incoming)
                .unwrap();
        let mut runtime = declaration.prepare().unwrap();
        let expected = [0.703125, 0.640625, 0.4375, 0.625];
        let check = |runtime: &mut ModulationRuntime, expected: &[f32]| {
            runtime.begin();
            runtime
                .render_chunk(expected.len(), None, |block| {
                    for (offset, &sample) in expected.iter().enumerate() {
                        assert_eq!(
                            block.value_at(id, offset),
                            Some(ParameterValue::Float(sample))
                        );
                    }
                    Ok(())
                })
                .unwrap();
            runtime.commit().unwrap();
        };
        check(&mut runtime, &expected[..3]);
        let mut successor = declaration.prepare().unwrap();
        successor.inherit_committed(&runtime).unwrap();
        check(&mut successor, &expected[3..]);
        check(&mut successor, &expected);
        successor.reset_all();
        check(&mut successor, &expected);
    }

    #[test]
    fn note_counter_follows_sample_offsets_and_transaction_custody() {
        use crate::NativeInstrumentExpression;
        use std::num::NonZeroU64;

        let fixture = prepared(6_000.0);
        let id = ParameterId::from_static(1);
        let target = ModulationTarget {
            descriptor: fixture.program().descriptor(id).unwrap(),
            semantics: fixture.program().semantics(id).unwrap(),
            base: ParameterValue::Float(0.5),
        };
        let program = PreparedModulation::new(
            &[target],
            &[ModulationRoute {
                source: 0,
                target: id,
                amount: 0.5,
                range: [-1.0, 1.0],
            }],
            1,
        )
        .unwrap();
        let counter = PreparedModulationSource::note_counter(
            ModulationSourceId::new(1).unwrap(),
            48_000,
            5,
            2,
            true,
        )
        .unwrap();
        let declaration = crate::PreparedModulationState::new(vec![counter], program, 8).unwrap();
        let mut runtime = declaration.prepare().unwrap();
        let onset = |frame_offset, occurrence_id| NativeInstrumentEvent::NoteOn {
            frame_offset,
            occurrence_id,
            pitch: 60,
            velocity: 100,
        };
        let first = [
            NativeInstrumentEvent::Expression {
                frame_offset: 0,
                expression: NativeInstrumentExpression::ModWheel(64),
            },
            onset(0, 1),
            onset(0, 2),
            NativeInstrumentEvent::NotePressure {
                frame_offset: 1,
                occurrence_id: NonZeroU64::new(1).unwrap(),
                value: 32,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 2,
                occurrence_id: 1,
            },
        ];
        let second = [onset(0, 3), onset(2, 4)];
        let check = |runtime: &mut ModulationRuntime, events, expected: &[f32]| {
            runtime
                .render_instrument_chunk(expected.len(), None, None, events, |block| {
                    for (offset, &sample) in expected.iter().enumerate() {
                        assert_eq!(
                            block.value_at(id, offset),
                            Some(ParameterValue::Float(sample))
                        );
                    }
                    Ok(())
                })
                .unwrap();
        };
        runtime.begin();
        assert!(runtime.render_chunk(3, None, |_| Ok(())).is_err());
        for invalid in [
            &[onset(3, 1)][..],
            &[onset(1, 1), onset(0, 2)],
            &[onset(0, 1); 513],
        ] {
            assert!(
                runtime
                    .render_instrument_chunk(3, None, None, invalid, |_| Ok(()))
                    .is_err()
            );
        }
        check(&mut runtime, &first, &[1.0; 3]);
        runtime.rollback();
        runtime.begin();
        check(&mut runtime, &first, &[1.0; 3]);
        runtime.commit().unwrap();
        let mut successor = declaration.prepare().unwrap();
        successor.inherit_committed(&runtime).unwrap();
        successor.begin();
        check(&mut successor, &second, &[0.25, 0.25, 0.75]);
        successor.commit().unwrap();
        successor.reset_all();
        successor.begin();
        check(&mut successor, &[], &[0.0; 2]);
        let combined = [
            first[0],
            first[1],
            first[2],
            first[3],
            first[4],
            onset(3, 3),
            onset(5, 4),
        ];
        check(
            &mut successor,
            &combined,
            &[1.0, 1.0, 1.0, 0.25, 0.25, 0.75],
        );
        successor.commit().unwrap();
    }

    #[test]
    fn note_ramp_loops_at_the_declared_period_and_preserves_transaction_history() {
        use crate::ModulationRampCurve;

        let fixture = prepared(6_000.0);
        let id = ParameterId::from_static(1);
        let target = ModulationTarget {
            descriptor: fixture.program().descriptor(id).unwrap(),
            semantics: fixture.program().semantics(id).unwrap(),
            base: ParameterValue::Float(0.5),
        };
        let program = PreparedModulation::new(
            &[target],
            &[ModulationRoute {
                source: 0,
                target: id,
                amount: 0.5,
                range: [-1.0, 1.0],
            }],
            1,
        )
        .unwrap();
        let source = ModulationSourceId::new(1).unwrap();
        for frames in [0, 60 * 48_000 + 1] {
            assert!(
                PreparedModulationSource::note_ramp(
                    source,
                    48_000,
                    frames,
                    ModulationRampCurve::Linear,
                    false,
                    false,
                )
                .is_err()
            );
        }
        let ramp = PreparedModulationSource::note_ramp(
            source,
            48_000,
            4,
            ModulationRampCurve::EaseIn,
            true,
            true,
        )
        .unwrap();
        let declaration = crate::PreparedModulationState::new(vec![ramp], program, 8).unwrap();
        let mut runtime = declaration.prepare().unwrap();
        let onset = |occurrence_id| NativeInstrumentEvent::NoteOn {
            frame_offset: 1,
            occurrence_id,
            pitch: 60,
            velocity: 100,
        };
        let first = [
            onset(1),
            onset(2),
            NativeInstrumentEvent::NoteOff {
                frame_offset: 2,
                occurrence_id: 1,
            },
        ];
        let check = |runtime: &mut ModulationRuntime, events, expected: &[f32]| {
            runtime
                .render_instrument_chunk(expected.len(), None, None, events, |block| {
                    for (offset, &sample) in expected.iter().enumerate() {
                        assert_eq!(
                            block.value_at(id, offset),
                            Some(ParameterValue::Float(sample))
                        );
                    }
                    Ok(())
                })
                .unwrap();
        };
        runtime.begin();
        check(&mut runtime, &first, &[0.5, 1.0, 0.96875, 0.875]);
        runtime.rollback();
        runtime.begin();
        check(&mut runtime, &first, &[0.5, 1.0, 0.96875, 0.875]);
        runtime.commit().unwrap();
        let mut next = declaration.prepare().unwrap();
        next.inherit_committed(&runtime).unwrap();
        next.begin();
        check(&mut next, &[], &[0.71875, 1.0, 0.96875]);
        next.commit().unwrap();
        next.reset_all();
        next.begin();
        check(&mut next, &[], &[0.5; 2]);
        check(
            &mut next,
            &first,
            &[0.5, 1.0, 0.96875, 0.875, 0.71875, 1.0, 0.96875],
        );
        next.commit().unwrap();
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
