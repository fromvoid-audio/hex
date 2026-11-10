//! Sample-local normalized composition for a continuous foreign effect target.
//! The audio host admits the actual source and capability, owns the borrowed
//! output scratch and decides when the surrounding callback commits.
use crate::{
    ForeignModulationRoute, ForeignProcessorNode, ForeignProcessors, ModulationRuntime,
    NativeInstrumentClock, OffsetClock, PreparedModulation, PreparedModulationSource,
    PreparedModulationState, SourceModulationRoute, StereoFrame,
};
use hex_lattice_contract::{DeviceContractError as Error, ForeignProcessorBinding};

pub(crate) const MAX_FOREIGN_MODULATION_CHUNK: usize = 256;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ForeignEffectTarget {
    pub binding: ForeignProcessorBinding,
    pub raw_id: u32,
    pub normalized_base: f64,
}

/// Per-frame normalized SDK values for one exact borrowed effect segment.
#[derive(Clone, Copy)]
pub struct ForeignModulationSamples<'a> {
    pub id: u32,
    pub normalized: &'a [f64],
}

/// One foreign raw ID and its exact restored f64 base, paired with a validated
/// effect-local bank. The provider host still proves that this target exists,
/// is continuous and accepts sample-rate input on the actual prepared source.
#[derive(Clone, Debug)]
pub struct PreparedForeignModulation {
    raw_id: u32,
    normalized_base: f64,
    state: PreparedModulationState,
}

impl PreparedForeignModulation {
    pub fn new(
        raw_id: u32,
        normalized_base: f64,
        sources: Vec<PreparedModulationSource>,
        routes: &[ForeignModulationRoute],
        source_routes: &[SourceModulationRoute],
        chunk_frames: usize,
    ) -> Result<Self, Error> {
        if !(0.0..=1.0).contains(&normalized_base) {
            return Err(Error::InvalidParameterValue);
        }
        if sources
            .iter()
            .any(PreparedModulationSource::reads_note_events)
        {
            return Err(Error::UnsupportedProcessingClass);
        }
        let program = PreparedModulation::for_foreign_target(routes, sources.len())?;
        let state = PreparedModulationState::with_source_routes(
            sources,
            program,
            chunk_frames,
            source_routes,
        )?;
        Ok(Self {
            raw_id,
            normalized_base,
            state,
        })
    }

    pub fn quoted_runtime_heap_bytes(&self) -> Option<usize> {
        self.state.quoted_runtime_heap_bytes()
    }

    /// Bind the declaration to a specific prepared effect endpoint. The
    /// provider must still admit the exact source and raw parameter capability.
    pub fn into_effect_modulation(
        self,
        binding: ForeignProcessorBinding,
    ) -> Result<PreparedModulationState, Error> {
        if !binding.is_valid() {
            return Err(Error::InvalidId);
        }
        Ok(self.state.bind_foreign_effect(ForeignEffectTarget {
            binding,
            raw_id: self.raw_id,
            normalized_base: self.normalized_base,
        }))
    }

    pub fn prepare(&self, sample_rate: u32) -> Result<ForeignModulationRuntime, Error> {
        self.state.validate_sample_rate(sample_rate)?;
        Ok(ForeignModulationRuntime::new(
            self.raw_id,
            self.normalized_base,
            self.state.prepare()?,
        ))
    }
}

/// Separate from native provider automation so a raw ID cannot masquerade as
/// a native typed parameter or grant access to a provider's controls.
pub struct ForeignModulationRuntime {
    raw_id: u32,
    normalized_base: f64,
    runtime: ModulationRuntime,
    committed_value: f64,
    staged_value: f64,
}

impl ForeignModulationRuntime {
    pub(crate) fn new(raw_id: u32, normalized_base: f64, runtime: ModulationRuntime) -> Self {
        Self {
            raw_id,
            normalized_base,
            runtime,
            committed_value: normalized_base,
            staged_value: normalized_base,
        }
    }

    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>().checked_add(self.runtime.heap_storage_bytes()?)
    }

    pub(crate) fn matches_prepared(&self, prepared: &PreparedModulationState) -> bool {
        prepared.foreign_effect().is_some_and(|target| {
            self.raw_id == target.raw_id
                && self.normalized_base.to_bits() == target.normalized_base.to_bits()
        }) && prepared.matches_runtime(&self.runtime)
    }

    pub(crate) fn same_declaration(&self, other: &Self) -> bool {
        self.raw_id == other.raw_id
            && self.normalized_base.to_bits() == other.normalized_base.to_bits()
            && self.runtime.same_declaration(&other.runtime)
    }

    pub(crate) fn target_id(&self) -> u32 {
        self.raw_id
    }

    pub(crate) fn inherit_committed(&mut self, previous: &Self) {
        self.runtime.inherit_graph_committed(&previous.runtime);
        self.committed_value = previous.committed_value;
        self.staged_value = self.committed_value;
    }

    pub(crate) fn reset(&mut self) {
        self.runtime.reset_all();
        self.committed_value = self.normalized_base;
        self.staged_value = self.normalized_base;
    }

    pub fn begin(&mut self) {
        self.runtime.begin();
        self.staged_value = self.committed_value;
    }

    /// Lend one normalized value per input frame. A refused consumer abandons
    /// the speculative source history for the entire surrounding transaction.
    pub fn render_chunk(
        &mut self,
        input: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output: &mut [f64],
        consume: impl FnOnce(&mut [StereoFrame], u32, &[f64]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let base = self.normalized_base;
        let raw_id = self.raw_id;
        self.runtime
            .render_audio_chunk(input, None, None, clock, |input, values| {
                values.compose_foreign_target(base, output)?;
                consume(input, raw_id, output)
            })?;
        if let Some(&value) = output.last() {
            self.staged_value = value;
        }
        Ok(())
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
        let mut normalized = [0.0; MAX_FOREIGN_MODULATION_CHUNK];
        let chunk_frames = self.runtime.chunk_frames().min(normalized.len());
        for (index, frames) in frames.chunks_mut(chunk_frames).enumerate() {
            let offset = index * chunk_frames;
            let physical_offset = frame_offset
                .checked_add(offset)
                .ok_or(Error::InvalidDescriptor)?;
            let shifted = clock.map(|clock| OffsetClock { clock, offset });
            let chunk_clock = shifted
                .as_ref()
                .map(|value| value as &dyn NativeInstrumentClock);
            let normalized = &mut normalized[..frames.len()];
            let mut stage = |frames: &mut [StereoFrame], id, normalized: &[f64]| {
                foreign.process_segment_with_modulation(
                    node,
                    physical_offset,
                    frames,
                    ForeignModulationSamples { id, normalized },
                    chunk_clock,
                )
            };
            if frozen {
                normalized.fill(self.committed_value);
                stage(frames, self.raw_id, normalized)?;
            } else {
                self.render_chunk(frames, chunk_clock, normalized, stage)?;
            }
        }
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), Error> {
        self.runtime.commit()?;
        self.committed_value = self.staged_value;
        Ok(())
    }

    pub fn rollback(&mut self) {
        self.runtime.rollback();
        self.staged_value = self.committed_value;
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
}
