//! Sample-local normalized composition for up to 16 continuous foreign (VST3)
//! targets, supporting both audio effects and note-driven VST3 instruments.
//! The audio host admits the actual source and capability, owns the borrowed
//! output scratch and decides when the surrounding callback commits.
use crate::{
    ForeignModulationRoute, ForeignProcessorNode, ForeignProcessors, ForeignTargetModulationRoute,
    ModulationRuntime, NativeInstrumentClock, NativeInstrumentEvent, OffsetClock,
    PreparedModulation, PreparedModulationSource, PreparedModulationState, SourceModulationRoute,
    StereoFrame,
};
use hex_lattice_contract::{DeviceContractError as Error, ForeignProcessorBinding};

pub(crate) const MAX_FOREIGN_MODULATION_CHUNK: usize = 256;

/// Maximum number of continuous raw foreign parameters modulated concurrently
/// on one VST3 effect or instrument.
pub const MAX_FOREIGN_MODULATION_TARGETS: usize = 16;

/// One continuous raw foreign parameter and its exact restored f64 base value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForeignModulationTarget {
    pub raw_id: u32,
    pub normalized_base: f64,
}

impl ForeignModulationTarget {
    const EMPTY: Self = Self {
        raw_id: 0,
        normalized_base: 0.0,
    };

    fn bits_eq(self, other: Self) -> bool {
        self.raw_id == other.raw_id
            && self.normalized_base.to_bits() == other.normalized_base.to_bits()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ForeignEffectTarget {
    pub binding: ForeignProcessorBinding,
    pub targets: [ForeignModulationTarget; MAX_FOREIGN_MODULATION_TARGETS],
    pub target_count: u8,
    pub allows_note_events: bool,
}

impl ForeignEffectTarget {
    pub(crate) fn as_slice(&self) -> &[ForeignModulationTarget] {
        &self.targets[..usize::from(self.target_count)]
    }
}

/// Per-frame normalized SDK values for one exact borrowed effect segment.
#[derive(Clone, Copy)]
pub struct ForeignModulationSamples<'a> {
    pub id: u32,
    pub normalized: &'a [f64],
}

/// Up to `MAX_FOREIGN_MODULATION_TARGETS` foreign raw IDs and their exact
/// restored f64 bases, paired with a validated device-local modulator bank.
/// The provider host still proves that every target exists, is continuous and
/// accepts sample-rate input on the actual prepared source.
#[derive(Clone, Debug)]
pub struct PreparedForeignModulation {
    targets: [ForeignModulationTarget; MAX_FOREIGN_MODULATION_TARGETS],
    target_count: u8,
    allows_note_events: bool,
    state: PreparedModulationState,
}

impl PreparedForeignModulation {
    /// Single-target audio-effect constructor preserving the original API.
    /// Rejects note-driven modulators because audio effects supply no note stream.
    pub fn new(
        raw_id: u32,
        normalized_base: f64,
        sources: Vec<PreparedModulationSource>,
        routes: &[ForeignModulationRoute],
        source_routes: &[SourceModulationRoute],
        chunk_frames: usize,
    ) -> Result<Self, Error> {
        let mut target_routes = Vec::new();
        target_routes
            .try_reserve_exact(routes.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        for route in routes {
            target_routes.push(ForeignTargetModulationRoute {
                target: raw_id,
                source: route.source,
                amount: route.amount,
                range: route.range,
            });
        }
        Self::new_multi(
            &[ForeignModulationTarget {
                raw_id,
                normalized_base,
            }],
            &target_routes,
            sources,
            source_routes,
            chunk_frames,
            false,
        )
    }

    /// Multi-target constructor admitting up to `MAX_FOREIGN_MODULATION_TARGETS`
    /// strictly increasing continuous raw foreign parameter IDs. Set
    /// `allow_note_events = true` for VST3 instruments to permit note-driven
    /// modulators (ADSR, AHD-Release, Ramp, Note Counter, Steps).
    pub fn new_multi(
        targets: &[ForeignModulationTarget],
        routes: &[ForeignTargetModulationRoute],
        sources: Vec<PreparedModulationSource>,
        source_routes: &[SourceModulationRoute],
        chunk_frames: usize,
        allow_note_events: bool,
    ) -> Result<Self, Error> {
        if targets.is_empty() || targets.len() > MAX_FOREIGN_MODULATION_TARGETS {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let mut inline_targets = [ForeignModulationTarget::EMPTY; MAX_FOREIGN_MODULATION_TARGETS];
        let mut raw_ids = [0u32; MAX_FOREIGN_MODULATION_TARGETS];
        for (index, &target) in targets.iter().enumerate() {
            if !(0.0..=1.0).contains(&target.normalized_base) {
                return Err(Error::InvalidParameterValue);
            }
            if index > 0 && raw_ids[index - 1] >= target.raw_id {
                return Err(Error::InvalidDescriptor);
            }
            inline_targets[index] = target;
            raw_ids[index] = target.raw_id;
        }
        if !allow_note_events
            && sources
                .iter()
                .any(PreparedModulationSource::reads_note_events)
        {
            return Err(Error::UnsupportedProcessingClass);
        }
        let program = PreparedModulation::for_foreign_targets(
            &raw_ids[..targets.len()],
            routes,
            sources.len(),
        )?;
        let state = PreparedModulationState::with_source_routes(
            sources,
            program,
            chunk_frames,
            source_routes,
        )?;
        Ok(Self {
            targets: inline_targets,
            target_count: targets.len() as u8,
            allows_note_events: allow_note_events,
            state,
        })
    }

    pub fn targets(&self) -> &[ForeignModulationTarget] {
        &self.targets[..usize::from(self.target_count)]
    }

    pub fn allows_note_events(&self) -> bool {
        self.allows_note_events
    }

    pub fn quoted_runtime_heap_bytes(&self) -> Option<usize> {
        let scratch_bytes = usize::from(self.target_count)
            .checked_mul(self.state.chunk_frames())?
            .checked_mul(std::mem::size_of::<f64>())?;
        self.state
            .quoted_runtime_heap_bytes()?
            .checked_add(scratch_bytes)
    }

    /// Bind the declaration to a specific prepared effect/instrument endpoint.
    /// The provider must still admit the exact source and raw parameter capabilities.
    pub fn into_effect_modulation(
        self,
        binding: ForeignProcessorBinding,
    ) -> Result<PreparedModulationState, Error> {
        if !binding.is_valid() {
            return Err(Error::InvalidId);
        }
        Ok(self.state.bind_foreign_effect(ForeignEffectTarget {
            binding,
            targets: self.targets,
            target_count: self.target_count,
            allows_note_events: self.allows_note_events,
        }))
    }

    pub fn prepare(&self, sample_rate: u32) -> Result<ForeignModulationRuntime, Error> {
        self.state.validate_sample_rate(sample_rate)?;
        ForeignModulationRuntime::new(
            self.targets(),
            self.allows_note_events,
            self.state.prepare()?,
        )
    }
}

/// Separate from native provider automation so raw IDs cannot masquerade as
/// native typed parameters or grant access to a provider's controls.
pub struct ForeignModulationRuntime {
    targets: [ForeignModulationTarget; MAX_FOREIGN_MODULATION_TARGETS],
    target_count: u8,
    allows_note_events: bool,
    runtime: ModulationRuntime,
    committed_values: [f64; MAX_FOREIGN_MODULATION_TARGETS],
    staged_values: [f64; MAX_FOREIGN_MODULATION_TARGETS],
    /// Preallocated control-side scratch (`target_count * chunk_frames`) so
    /// multi-target callback rendering allocates zero heap memory.
    scratch: Vec<f64>,
}

impl ForeignModulationRuntime {
    pub(crate) fn new(
        targets: &[ForeignModulationTarget],
        allows_note_events: bool,
        runtime: ModulationRuntime,
    ) -> Result<Self, Error> {
        if targets.is_empty() || targets.len() > MAX_FOREIGN_MODULATION_TARGETS {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let mut inline_targets = [ForeignModulationTarget::EMPTY; MAX_FOREIGN_MODULATION_TARGETS];
        let mut committed = [0.0; MAX_FOREIGN_MODULATION_TARGETS];
        for (index, &target) in targets.iter().enumerate() {
            inline_targets[index] = target;
            committed[index] = target.normalized_base;
        }
        let scratch_len = targets
            .len()
            .checked_mul(runtime.chunk_frames())
            .ok_or(Error::RuntimeBudgetExceeded)?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(scratch_len)
            .map_err(|_| Error::RuntimeAllocation)?;
        scratch.resize(scratch_len, 0.0);
        Ok(Self {
            targets: inline_targets,
            target_count: targets.len() as u8,
            allows_note_events,
            runtime,
            committed_values: committed,
            staged_values: committed,
            scratch,
        })
    }

    pub fn targets(&self) -> &[ForeignModulationTarget] {
        &self.targets[..usize::from(self.target_count)]
    }

    pub fn allows_note_events(&self) -> bool {
        self.allows_note_events
    }

    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(self.runtime.heap_storage_bytes()?)?
            .checked_add(
                self.scratch
                    .capacity()
                    .checked_mul(std::mem::size_of::<f64>())?,
            )
    }

    pub(crate) fn matches_prepared(&self, prepared: &PreparedModulationState) -> bool {
        prepared.foreign_effect().is_some_and(|target| {
            self.target_count == target.target_count
                && self.allows_note_events == target.allows_note_events
                && self
                    .targets()
                    .iter()
                    .zip(target.as_slice())
                    .all(|(a, b)| a.bits_eq(*b))
        }) && prepared.matches_runtime(&self.runtime)
    }

    pub(crate) fn same_declaration(&self, other: &Self) -> bool {
        self.target_count == other.target_count
            && self.allows_note_events == other.allows_note_events
            && self
                .targets()
                .iter()
                .zip(other.targets())
                .all(|(a, b)| a.bits_eq(*b))
            && self.runtime.same_declaration(&other.runtime)
    }

    pub(crate) fn same_routes_and_sources(&self, other: &Self) -> bool {
        self.target_ids() == other.target_ids() && self.runtime.same_declaration(&other.runtime)
    }

    pub(crate) fn target_ids(&self) -> ([u32; MAX_FOREIGN_MODULATION_TARGETS], u8, bool) {
        let mut ids = [0u32; MAX_FOREIGN_MODULATION_TARGETS];
        for (slot, target) in ids.iter_mut().zip(self.targets()) {
            *slot = target.raw_id;
        }
        (ids, self.target_count, self.allows_note_events)
    }

    pub(crate) fn inherit_committed(&mut self, previous: &Self) {
        self.runtime.inherit_graph_committed(&previous.runtime);
        let count = usize::from(self.target_count).min(usize::from(previous.target_count));
        for index in 0..count {
            if self.targets[index].raw_id == previous.targets[index].raw_id {
                self.committed_values[index] = previous.committed_values[index];
                self.staged_values[index] = self.committed_values[index];
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.runtime.reset_all();
        for index in 0..usize::from(self.target_count) {
            self.committed_values[index] = self.targets[index].normalized_base;
            self.staged_values[index] = self.targets[index].normalized_base;
        }
    }

    pub fn begin(&mut self) {
        self.runtime.begin();
        self.staged_values = self.committed_values;
    }

    /// Lend one normalized value per input frame for a single-target runtime.
    /// A refused consumer abandons the speculative source history for the entire
    /// surrounding transaction.
    pub fn render_chunk(
        &mut self,
        input: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output: &mut [f64],
        consume: impl FnOnce(&mut [StereoFrame], u32, &[f64]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.target_count != 1 {
            return Err(Error::InvalidDescriptor);
        }
        let target = self.targets[0];
        self.runtime
            .render_audio_chunk(input, None, None, clock, |input, values| {
                values.compose_foreign_target_at(0, target.normalized_base, output)?;
                consume(input, target.raw_id, output)
            })?;
        if let Some(&value) = output.last() {
            self.staged_values[0] = value;
        }
        Ok(())
    }

    /// Lend one normalized value lane per configured target (up to 16),
    /// optionally feeding ordered instrument note events to note-driven
    /// modulators. Zero heap allocations occur during rendering.
    pub fn render_chunk_multi(
        &mut self,
        input: &mut [StereoFrame],
        notes: Option<&[NativeInstrumentEvent]>,
        clock: Option<&dyn NativeInstrumentClock>,
        consume: impl FnOnce(&mut [StereoFrame], &[ForeignModulationSamples<'_>]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if notes.is_some() && !self.allows_note_events {
            return Err(Error::UnsupportedProcessingClass);
        }
        let count = usize::from(self.target_count);
        let frames_len = input.len();
        let stride = self.runtime.chunk_frames();
        if frames_len > stride || count * stride > self.scratch.len() {
            return Err(Error::InvalidParameterValue);
        }
        let targets = self.targets;
        let scratch = &mut self.scratch;
        let staged = &mut self.staged_values;
        self.runtime.render_audio_chunk_with_notes(
            input,
            None,
            None,
            clock,
            notes,
            |input, values| {
                for (index, staged_slot) in staged.iter_mut().enumerate().take(count) {
                    let start = index * stride;
                    let lane = &mut scratch[start..start + frames_len];
                    values.compose_foreign_target_at(
                        index,
                        targets[index].normalized_base,
                        lane,
                    )?;
                    if let Some(&last) = lane.last() {
                        *staged_slot = last;
                    }
                }
                let mut lanes = [ForeignModulationSamples {
                    id: 0,
                    normalized: &[],
                }; MAX_FOREIGN_MODULATION_TARGETS];
                for (index, lane_slot) in lanes.iter_mut().enumerate().take(count) {
                    let start = index * stride;
                    *lane_slot = ForeignModulationSamples {
                        id: targets[index].raw_id,
                        normalized: &scratch[start..start + frames_len],
                    };
                }
                consume(input, &lanes[..count])
            },
        )
    }

    pub(crate) fn process_effect_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
        frozen: bool,
    ) -> Result<(), Error> {
        let count = usize::from(self.target_count);
        let stride = self.runtime.chunk_frames();
        let chunk_frames = stride.min(MAX_FOREIGN_MODULATION_CHUNK);
        for (index, frames) in frames.chunks_mut(chunk_frames).enumerate() {
            let offset = index * chunk_frames;
            let physical_offset = frame_offset
                .checked_add(offset)
                .ok_or(Error::InvalidDescriptor)?;
            let shifted = clock.map(|clock| OffsetClock { clock, offset });
            let chunk_clock = shifted
                .as_ref()
                .map(|value| value as &dyn NativeInstrumentClock);
            let mut stage = |frames: &mut [StereoFrame], lanes: &[ForeignModulationSamples<'_>]| {
                foreign.process_segment_with_modulations(
                    node,
                    physical_offset,
                    frames,
                    lanes,
                    &[],
                    chunk_clock,
                )
            };
            if frozen {
                let len = frames.len();
                for (target_index, &committed) in
                    self.committed_values.iter().enumerate().take(count)
                {
                    let start = target_index * stride;
                    self.scratch[start..start + len].fill(committed);
                }
                let mut lanes = [ForeignModulationSamples {
                    id: 0,
                    normalized: &[],
                }; MAX_FOREIGN_MODULATION_TARGETS];
                for (target_index, lane_slot) in lanes.iter_mut().enumerate().take(count) {
                    let start = target_index * stride;
                    *lane_slot = ForeignModulationSamples {
                        id: self.targets[target_index].raw_id,
                        normalized: &self.scratch[start..start + len],
                    };
                }
                stage(frames, &lanes[..count])?;
            } else {
                self.render_chunk_multi(frames, None, chunk_clock, stage)?;
            }
        }
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), Error> {
        self.runtime.commit()?;
        self.committed_values = self.staged_values;
        Ok(())
    }

    pub fn rollback(&mut self) {
        self.runtime.rollback();
        self.staged_values = self.committed_values;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModulationSourceId;

    const BASE: f64 = 0.500_000_000_000_000_1;
    const ROUTES: [ForeignModulationRoute; 2] = [
        ForeignModulationRoute {
            source: 0,
            amount: -0.5,
            range: [-1.0, 1.0],
        },
        ForeignModulationRoute {
            source: 0,
            amount: 0.25,
            range: [-0.5, 0.5],
        },
    ];

    fn sources() -> Vec<PreparedModulationSource> {
        vec![
            PreparedModulationSource::free(
                ModulationSourceId::new(1).unwrap(),
                48_000.0,
                12_000.0,
                0.0,
            )
            .unwrap(),
        ]
    }

    fn read(runtime: &mut ForeignModulationRuntime, output: &mut [f64]) {
        let mut pcm = [[0.25, -0.25]; 8];
        runtime
            .render_chunk(&mut pcm[..output.len()], None, output, |pcm, id, values| {
                assert_eq!(id, 0);
                assert_eq!(pcm, &[[0.25, -0.25]; 8][..values.len()]);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn exact_foreign_base_and_signed_routes_survive_partitions_and_refusal() {
        let prepared = PreparedForeignModulation::new(0, BASE, sources(), &ROUTES, &[], 8).unwrap();
        let mut whole = prepared.prepare(48_000).unwrap();
        whole.begin();
        let mut values = [0.0; 4];
        read(&mut whole, &mut values);
        whole.commit().unwrap();
        for (actual, expected) in values
            .into_iter()
            .zip([BASE, BASE - 0.375, BASE, BASE + 0.375])
        {
            assert!((actual - expected).abs() <= 1e-15);
        }
        assert_eq!(values[0].to_bits(), BASE.to_bits());

        let mut split = prepared.prepare(48_000).unwrap();
        split.begin();
        let mut output = [0.0; 4];
        read(&mut split, &mut output[..3]);
        let refusal = split.render_chunk(&mut [[0.0; 2]], None, &mut output[3..], |_, _, _| {
            Err(Error::ForeignProcessingUnavailable)
        });
        assert_eq!(refusal, Err(Error::ForeignProcessingUnavailable));
        assert!(split.commit().is_err());

        split.begin();
        read(&mut split, &mut output[..1]);
        read(&mut split, &mut output[1..]);
        split.commit().unwrap();
        assert_eq!(output, values);
        whole.begin();
        split.begin();
        let mut after = [0.0; 1];
        read(&mut whole, &mut after);
        let mut resumed = [0.0; 1];
        read(&mut split, &mut resumed);
        assert_eq!(after, resumed);
    }

    #[test]
    fn clamps_only_after_summing_and_rejects_invalid_preparation() {
        let extreme = [ForeignModulationRoute {
            amount: 1.0,
            ..ROUTES[0]
        }];
        let mut runtime = PreparedForeignModulation::new(0, BASE, sources(), &extreme, &[], 8)
            .unwrap()
            .prepare(48_000)
            .unwrap();
        runtime.begin();
        let mut output = [0.0; 4];
        read(&mut runtime, &mut output);
        assert_eq!(output[1], 1.0);
        assert_eq!(output[3], 0.0);
        assert!(matches!(
            PreparedForeignModulation::new(0, f64::NAN, sources(), &ROUTES, &[], 8),
            Err(Error::InvalidParameterValue)
        ));
        assert!(matches!(
            PreparedForeignModulation::new(
                0,
                BASE,
                sources(),
                &[ForeignModulationRoute {
                    source: 1,
                    ..ROUTES[0]
                }],
                &[],
                8
            ),
            Err(Error::InvalidParameterValue)
        ));
    }

    #[test]
    fn supports_sixteen_simultaneous_foreign_targets_and_rejects_seventeenth() {
        let targets: Vec<_> = (0..MAX_FOREIGN_MODULATION_TARGETS as u32)
            .map(|id| ForeignModulationTarget {
                raw_id: id * 10,
                normalized_base: 0.5,
            })
            .collect();
        let routes: Vec<_> = targets
            .iter()
            .enumerate()
            .map(|(index, target)| ForeignTargetModulationRoute {
                target: target.raw_id,
                source: 0,
                amount: if index % 2 == 0 { 0.25 } else { -0.25 },
                range: [-1.0, 1.0],
            })
            .collect();
        let prepared =
            PreparedForeignModulation::new_multi(&targets, &routes, sources(), &[], 8, false)
                .unwrap();
        let mut runtime = prepared.prepare(48_000).unwrap();
        runtime.begin();
        let mut pcm = [[0.0; 2]; 4];
        runtime
            .render_chunk_multi(&mut pcm, None, None, |_, lanes| {
                assert_eq!(lanes.len(), MAX_FOREIGN_MODULATION_TARGETS);
                for (index, lane) in lanes.iter().enumerate() {
                    assert_eq!(lane.id, index as u32 * 10);
                    let expected = if index % 2 == 0 { 0.75 } else { 0.25 };
                    assert!((lane.normalized[1] - expected).abs() <= 1e-12);
                }
                Ok(())
            })
            .unwrap();
        runtime.commit().unwrap();

        let mut too_many = targets;
        too_many.push(ForeignModulationTarget {
            raw_id: 999,
            normalized_base: 0.5,
        });
        assert_eq!(
            PreparedForeignModulation::new_multi(&too_many, &routes, sources(), &[], 8, false)
                .err(),
            Some(Error::RuntimeBudgetExceeded)
        );
    }

    #[test]
    fn permits_note_driven_modulators_only_for_foreign_instruments() {
        let note_source = PreparedModulationSource::note_counter(
            ModulationSourceId::new(1).unwrap(),
            48_000,
            4,
            1,
            true,
        )
        .unwrap();
        let target = [ForeignModulationTarget {
            raw_id: 7,
            normalized_base: 0.0,
        }];
        let route = [ForeignTargetModulationRoute {
            target: 7,
            source: 0,
            amount: 1.0,
            range: [0.0, 1.0],
        }];
        assert_eq!(
            PreparedForeignModulation::new_multi(&target, &route, vec![note_source], &[], 8, false)
                .err(),
            Some(Error::UnsupportedProcessingClass)
        );

        let prepared =
            PreparedForeignModulation::new_multi(&target, &route, vec![note_source], &[], 8, true)
                .unwrap();
        let mut runtime = prepared.prepare(48_000).unwrap();
        runtime.begin();
        let mut pcm = [[0.0; 2]; 4];
        let notes = [NativeInstrumentEvent::NoteOn {
            frame_offset: 1,
            pitch: 60,
            velocity: 100,
            occurrence_id: 1,
        }];
        runtime
            .render_chunk_multi(&mut pcm, Some(&notes), None, |_, lanes| {
                assert_eq!(lanes.len(), 1);
                assert_eq!(lanes[0].id, 7);
                assert!(
                    lanes[0].normalized[2] > lanes[0].normalized[0],
                    "note-on must advance note-driven modulator on foreign instrument"
                );
                Ok(())
            })
            .unwrap();
        runtime.commit().unwrap();
    }
}
