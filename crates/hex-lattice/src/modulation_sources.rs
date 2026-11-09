//! Transactional sample generation for a host-owned bank of modulators.
//!
//! Construction and destruction are control-side. The bank owns fixed source
//! state and preallocated output buffers, but no clock, graph, transport,
//! scheduling, source registry or publication authority. The enclosing graph
//! begins one transaction, renders as many bounded chunks as needed, and then
//! commits or abandons it with the rest of the audio block. Each chunk's samples
//! remain available until the next render. The host splits longer segments and
//! defines whether bypass, stop, seek, loops and retrigger advance or reset a
//! source; this helper never infers those events from block boundaries.
//! Tempo sources preserve phase while following sample-local host tempo. One
//! preallocated cache shares clock reads across sources; absent clock values use
//! the prepared tempo. Rate/mode retargeting follows the same block transaction
//! as phase, and graph inheritance retains the new graph's prepared timing mode.
use crate::{NativeInstrumentClock, modulation_source_routes::PreparedSourceRoute};
use hex_dsp::{EnvelopeFollower, LfoRate, LfoWaveform, ModulationLfo, RandomHold, StereoFrame};
use hex_lattice_contract::{DeviceContractError as Error, ModulationSourceId};

pub const MAX_MODULATION_SOURCE_SOURCES: usize = 32;
pub const MAX_MODULATION_SOURCE_STORAGE_BYTES: usize = 262_144;
pub const MAX_MODULATION_SOURCE_CHUNK_FRAMES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Timing {
    Free,
    Tempo {
        beats_per_cycle: f64,
        fallback: LfoRate,
    },
}
impl Timing {
    fn rate(self, sample_rate: f64, tempo_micro_bpm: u32) -> Result<Option<LfoRate>, Error> {
        match self {
            Self::Free => Ok(None),
            Self::Tempo {
                beats_per_cycle,
                fallback,
            } => {
                if tempo_micro_bpm == 0 {
                    return Ok(Some(fallback));
                }
                LfoRate::tempo(
                    sample_rate,
                    f64::from(tempo_micro_bpm) / 1_000_000.0,
                    beats_per_cycle,
                )
                .map(Some)
                .map_err(|_| Error::InvalidParameterValue)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SourceState {
    Lfo {
        oscillator: ModulationLfo,
        timing: Timing,
    },
    Follower(EnvelopeFollower),
    Macro(f32),
    Random(RandomHold),
}

impl SourceState {
    fn tempo_timing(self) -> Option<Timing> {
        match self {
            Self::Lfo {
                timing: timing @ Timing::Tempo { .. },
                ..
            } => Some(timing),
            _ => None,
        }
    }

    fn inherit_history(&mut self, previous: Self) -> bool {
        match (self, previous) {
            (
                Self::Lfo { oscillator, .. },
                Self::Lfo {
                    oscillator: old, ..
                },
            ) => oscillator.inherit_phase(&old),
            (Self::Follower(follower), Self::Follower(old)) => follower.inherit_envelope(&old),
            // The candidate's authored value is complete. There is no
            // temporal history to carry from the prior graph.
            (Self::Macro(_), Self::Macro(_)) => {}
            (Self::Random(random), Self::Random(old)) => random.inherit_history(&old),
            _ => return false,
        }
        true
    }
}

/// A source prepared on control with validated parameters and initial state.
#[derive(Clone, Copy, Debug)]
pub struct PreparedModulationSource {
    pub id: ModulationSourceId,
    initial: SourceState,
    sample_rate: f64,
}
impl PreparedModulationSource {
    pub(crate) fn is_macro(&self) -> bool {
        matches!(self.initial, SourceState::Macro(_))
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Prepare a free-running source without exposing DSP internals to the host.
    pub fn free(
        id: ModulationSourceId,
        sample_rate: f64,
        frequency_hz: f64,
        phase: f64,
    ) -> Result<Self, Error> {
        Self::free_with_waveform(id, sample_rate, frequency_hz, phase, LfoWaveform::Sine)
    }

    /// Prepare a free-running oscillator with an explicit output shape.
    pub fn free_with_waveform(
        id: ModulationSourceId,
        sample_rate: f64,
        frequency_hz: f64,
        phase: f64,
        waveform: LfoWaveform,
    ) -> Result<Self, Error> {
        let rate = hex_dsp::LfoRate::free(sample_rate, frequency_hz)
            .map_err(|_| Error::InvalidParameterValue)?;
        let lfo = ModulationLfo::new_with_waveform(rate, phase, waveform)
            .map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            sample_rate,
            initial: SourceState::Lfo {
                oscillator: lfo,
                timing: Timing::Free,
            },
        })
    }

    /// Prepare a tempo-synced source using quarter-note beats per cycle.
    pub fn tempo(
        id: ModulationSourceId,
        sample_rate: f64,
        beats_per_minute: f64,
        beats_per_cycle: f64,
        phase: f64,
    ) -> Result<Self, Error> {
        Self::tempo_with_waveform(
            id,
            sample_rate,
            beats_per_minute,
            beats_per_cycle,
            phase,
            LfoWaveform::Sine,
        )
    }

    /// Prepare a tempo-synced oscillator with an explicit output shape.
    pub fn tempo_with_waveform(
        id: ModulationSourceId,
        sample_rate: f64,
        beats_per_minute: f64,
        beats_per_cycle: f64,
        phase: f64,
        waveform: LfoWaveform,
    ) -> Result<Self, Error> {
        let rate = hex_dsp::LfoRate::tempo(sample_rate, beats_per_minute, beats_per_cycle)
            .map_err(|_| Error::InvalidParameterValue)?;
        let lfo = ModulationLfo::new_with_waveform(rate, phase, waveform)
            .map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            sample_rate,
            initial: SourceState::Lfo {
                oscillator: lfo,
                timing: Timing::Tempo {
                    beats_per_cycle,
                    fallback: rate,
                },
            },
        })
    }

    /// A follower consumes the owning processor's pre-device audio for each
    /// frame. The graph supplies the borrowed signal during rendering.
    pub fn envelope_follower(
        id: ModulationSourceId,
        sample_rate: u32,
        attack_seconds: f64,
        release_seconds: f64,
    ) -> Result<Self, Error> {
        let follower = EnvelopeFollower::new(sample_rate, attack_seconds, release_seconds)
            .map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            initial: SourceState::Follower(follower),
            sample_rate: f64::from(sample_rate),
        })
    }
    /// A manual source holds its finite normalized value at each sample.
    pub fn macro_value(
        id: ModulationSourceId,
        sample_rate: u32,
        value: f32,
    ) -> Result<Self, Error> {
        if sample_rate == 0 || !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            id,
            initial: SourceState::Macro(value),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Prepare a free-running, seeded sample-and-hold source.
    pub fn random_hold(
        id: ModulationSourceId,
        sample_rate: u32,
        seed: u64,
        rate_hz: f64,
    ) -> Result<Self, Error> {
        let random = RandomHold::new(seed, sample_rate, rate_hz)
            .map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            initial: SourceState::Random(random),
            sample_rate: f64::from(sample_rate),
        })
    }
}

struct Source {
    id: ModulationSourceId,
    sample_rate: f64,
    initial: SourceState,
    committed: SourceState,
    scratch: SourceState,
    samples: Vec<f32>,
}

/// Source state and scratch storage borrowed exclusively by the graph owner.
pub struct ModulationSourceBank {
    sources: Vec<Source>,
    tempo_cache: Vec<u32>,
    chunk_frames: usize,
    last_frames: usize,
    active: bool,
    follows_input: bool,
}
impl ModulationSourceBank {
    pub(crate) fn quoted_heap_bytes(sources: usize, frames: usize) -> Option<usize> {
        sources
            .checked_mul(std::mem::size_of::<Source>())?
            .checked_add(
                sources
                    .checked_mul(frames)?
                    .checked_mul(std::mem::size_of::<f32>())?,
            )?
            .checked_add(if sources == 0 {
                0
            } else {
                frames.checked_mul(std::mem::size_of::<u32>())?
            })
    }
    pub(crate) fn matches_prepared(
        &self,
        sources: &[PreparedModulationSource],
        chunk_frames: usize,
    ) -> bool {
        self.chunk_frames == chunk_frames
            && self.sources.len() == sources.len()
            && self.sources.iter().zip(sources).all(|(runtime, prepared)| {
                runtime.id == prepared.id
                    && runtime.sample_rate == prepared.sample_rate
                    && runtime.initial == prepared.initial
            })
    }

    pub(crate) fn same_declaration(&self, other: &Self) -> bool {
        self.chunk_frames == other.chunk_frames
            && self.sources.len() == other.sources.len()
            && self
                .sources
                .iter()
                .zip(&other.sources)
                .all(|(left, right)| {
                    left.id == right.id
                        && left.sample_rate == right.sample_rate
                        && left.initial == right.initial
                })
    }

    pub(crate) fn validate_sources(
        sources: &[PreparedModulationSource],
        chunk_frames: usize,
    ) -> Result<(), Error> {
        if sources.len() > MAX_MODULATION_SOURCE_SOURCES
            || chunk_frames == 0
            || chunk_frames > MAX_MODULATION_SOURCE_CHUNK_FRAMES
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let minimum = Self::quoted_heap_bytes(sources.len(), chunk_frames)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .ok_or(Error::RuntimeBudgetExceeded)?;
        if minimum > MAX_MODULATION_SOURCE_STORAGE_BYTES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        if sources.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err(Error::InvalidId);
        }
        Ok(())
    }

    /// IDs must be unique and strictly increasing. The host accounts this
    /// storage alongside its existing paired-runtime budget before publication.
    pub fn new(sources: &[PreparedModulationSource], chunk_frames: usize) -> Result<Self, Error> {
        Self::validate_sources(sources, chunk_frames)?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(sources.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        for source in sources {
            let mut samples = Vec::new();
            samples
                .try_reserve_exact(chunk_frames)
                .map_err(|_| Error::RuntimeAllocation)?;
            samples.resize(chunk_frames, 0.0);
            prepared.push(Source {
                id: source.id,
                sample_rate: source.sample_rate,
                initial: source.initial,
                committed: source.initial,
                scratch: source.initial,
                samples,
            });
        }
        let mut tempo_cache = Vec::new();
        let cache_frames = if sources.is_empty() { 0 } else { chunk_frames };
        tempo_cache
            .try_reserve_exact(cache_frames)
            .map_err(|_| Error::RuntimeAllocation)?;
        tempo_cache.resize(cache_frames, 0);
        let bank = Self {
            sources: prepared,
            tempo_cache,
            chunk_frames,
            last_frames: 0,
            active: false,
            follows_input: sources
                .iter()
                .any(|source| matches!(source.initial, SourceState::Follower(_))),
        };
        if bank
            .storage_bytes()?
            .checked_add(std::mem::size_of::<Self>())
            .is_none_or(|bytes| bytes > MAX_MODULATION_SOURCE_STORAGE_BYTES)
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        Ok(bank)
    }

    /// Reserved vector capacities for host admission, excluding the inline
    /// bank header and allocator bookkeeping.
    pub fn storage_bytes(&self) -> Result<usize, Error> {
        let mut bytes = self
            .sources
            .capacity()
            .checked_mul(std::mem::size_of::<Source>())
            .ok_or(Error::RuntimeBudgetExceeded)?;
        bytes = bytes
            .checked_add(
                self.tempo_cache
                    .capacity()
                    .checked_mul(std::mem::size_of::<u32>())
                    .ok_or(Error::RuntimeBudgetExceeded)?,
            )
            .ok_or(Error::RuntimeBudgetExceeded)?;
        for source in &self.sources {
            bytes = source
                .samples
                .capacity()
                .checked_mul(std::mem::size_of::<f32>())
                .and_then(|size| size.checked_add(bytes))
                .ok_or(Error::RuntimeBudgetExceeded)?;
        }
        Ok(bytes)
    }

    /// Source count for comparing this bank with an admitted route program.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// The stable identity at this bank's sorted, transient source index.
    pub fn source_id(&self, index: usize) -> Option<ModulationSourceId> {
        self.sources.get(index).map(|source| source.id)
    }

    /// Maximum frames in one render chunk; longer segments require chunking.
    pub fn chunk_frames(&self) -> usize {
        self.chunk_frames
    }

    /// Start or restart a transaction from committed source state.
    pub fn begin(&mut self) {
        for source in &mut self.sources {
            source.scratch = source.committed;
        }
        self.last_frames = 0;
        self.active = true;
    }

    /// Generate one chunk. Refusal leaves both phase and last samples untouched.
    pub fn render_chunk(&mut self, frames: usize) -> Result<(), Error> {
        self.render_chunk_with_clock(frames, None)
    }

    /// Tempo sources follow the borrowed sample-local clock, retaining phase.
    /// Missing tempo uses the prepared fallback; invalid tempo refuses the
    /// entire chunk before source state or the previous samples are changed.
    pub fn render_chunk_with_clock(
        &mut self,
        frames: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), Error> {
        self.render_chunk_with_input(frames, clock, None)
    }

    pub(crate) fn follows_input(&self) -> bool {
        self.follows_input
    }

    pub(crate) fn render_chunk_with_input(
        &mut self,
        frames: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        input: Option<&[StereoFrame]>,
    ) -> Result<(), Error> {
        if !self.active || frames > self.chunk_frames {
            return Err(Error::InvalidParameterValue);
        }
        if self
            .sources
            .iter()
            .any(|source| source.samples.len() < frames)
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        if self.follows_input
            && input.is_none_or(|input| {
                input.len() != frames || input.iter().flatten().any(|sample| !sample.is_finite())
            })
        {
            return Err(Error::InvalidParameterValue);
        }
        let follows_tempo = self
            .sources
            .iter()
            .any(|source| source.scratch.tempo_timing().is_some());
        if follows_tempo {
            let cache = self
                .tempo_cache
                .get_mut(..frames)
                .ok_or(Error::RuntimeBudgetExceeded)?;
            for (offset, cached) in cache.iter_mut().enumerate() {
                *cached = match clock.and_then(|clock| clock.tempo_micro_bpm_at_offset(offset)) {
                    Some(0) => return Err(Error::InvalidParameterValue),
                    Some(tempo) => tempo,
                    None => 0,
                };
            }
            for source in &self.sources {
                if let Some(timing) = source.scratch.tempo_timing() {
                    for &tempo in cache.iter() {
                        timing.rate(source.sample_rate, tempo)?;
                    }
                }
            }
        }
        for source in &mut self.sources {
            let output = source
                .samples
                .get_mut(..frames)
                .ok_or(Error::RuntimeBudgetExceeded)?;
            match &mut source.scratch {
                SourceState::Lfo {
                    oscillator,
                    timing: Timing::Free,
                } => oscillator.render(output),
                SourceState::Lfo { oscillator, timing } => {
                    let tempos = self
                        .tempo_cache
                        .get(..frames)
                        .ok_or(Error::RuntimeBudgetExceeded)?;
                    for (sample, &tempo) in output.iter_mut().zip(tempos) {
                        if let Some(rate) = timing.rate(source.sample_rate, tempo)? {
                            oscillator.set_rate(rate);
                        }
                        *sample = oscillator.next_sample();
                    }
                }
                SourceState::Follower(follower) => follower
                    .render(input.ok_or(Error::InvalidParameterValue)?, output)
                    .map_err(|_| Error::InvalidParameterValue)?,
                SourceState::Macro(value) => output.fill(*value),
                SourceState::Random(random) => random.render(output),
            }
        }
        self.last_frames = frames;
        Ok(())
    }

    /// Compose in dependency order after the chunk's raw sources are rendered.
    /// Control preparation ties every index and non-self edge to this bank.
    pub(crate) fn apply_source_routes(
        &mut self,
        routes: &[PreparedSourceRoute],
    ) -> Result<(), Error> {
        for incoming in routes.chunk_by(|a, b| a.target == b.target) {
            let index = incoming[0].target;
            let (before, target_and_after) = self
                .sources
                .split_at_mut_checked(index)
                .ok_or(Error::InvalidDescriptor)?;
            let (target, after) = target_and_after
                .split_first_mut()
                .ok_or(Error::InvalidDescriptor)?;
            for (frame, value) in target.samples[..self.last_frames].iter_mut().enumerate() {
                let mut sum = 0.0_f64;
                for route in incoming {
                    let source = if route.source < index {
                        &before[route.source]
                    } else {
                        &after[route.source - index - 1]
                    };
                    sum += route.contribution.sample(source.samples[frame]);
                }
                if sum != 0.0 {
                    *value = (f64::from(*value) + sum).clamp(0.0, 1.0) as f32;
                }
            }
        }
        Ok(())
    }

    /// Change parameters at a chunk boundary, preserving speculative history.
    /// Rollback restores the previously committed parameters and history.
    pub fn retarget(&mut self, prepared: PreparedModulationSource) -> Result<(), Error> {
        if !self.active {
            return Err(Error::InvalidParameterValue);
        }
        let index = self
            .sources
            .binary_search_by_key(&prepared.id, |source| source.id)
            .map_err(|_| Error::InvalidId)?;
        let source = self.sources.get_mut(index).ok_or(Error::InvalidId)?;
        if source.sample_rate != prepared.sample_rate {
            return Err(Error::InvalidParameterValue);
        }
        let mut next = prepared.initial;
        if !next.inherit_history(source.scratch) {
            return Err(Error::UnsupportedProcessingClass);
        }
        source.scratch = next;
        Ok(())
    }

    /// Borrow the last rendered chunk for one source, without transferring ownership.
    pub fn samples(&self, id: ModulationSourceId) -> Option<&[f32]> {
        if !self.active {
            return None;
        }
        let index = self
            .sources
            .binary_search_by_key(&id, |source| source.id)
            .ok()?;
        self.sources.get(index)?.samples.get(..self.last_frames)
    }

    /// Install all speculative phases with the enclosing graph transaction.
    pub fn commit(&mut self) -> Result<(), Error> {
        if !self.active {
            return Err(Error::InvalidParameterValue);
        }
        for source in &mut self.sources {
            source.committed = source.scratch;
        }
        self.active = false;
        self.last_frames = 0;
        Ok(())
    }

    /// Abandon a speculative transaction; the next begin starts at committed phase.
    pub fn rollback(&mut self) {
        self.active = false;
        self.last_frames = 0;
    }

    pub(crate) fn reset_all(&mut self) {
        for source in &mut self.sources {
            source.committed = source.initial;
            source.scratch = source.initial;
        }
        self.active = false;
        self.last_frames = 0;
    }

    /// Copy committed history by identity while retaining candidate parameters.
    /// Called only by the existing graph owner after its replacement preflight.
    pub fn inherit_committed(&mut self, previous: &Self) -> Result<(), Error> {
        if self.active || previous.active {
            return Err(Error::InvalidParameterValue);
        }
        self.inherit_graph_committed(previous);
        Ok(())
    }

    pub(crate) fn inherit_graph_committed(&mut self, previous: &Self) {
        for source in &mut self.sources {
            if let Some(old) = previous
                .sources
                .iter()
                .find(|old| old.id == source.id && old.sample_rate == source.sample_rate)
                && source.committed.inherit_history(old.committed)
            {
                source.scratch = source.committed;
            }
        }
        self.active = false;
        self.last_frames = 0;
    }

    /// Explicit discontinuity or retrigger; never acts on a speculative block.
    pub fn reset(&mut self, id: ModulationSourceId, phase: f64) -> Result<(), Error> {
        if self.active {
            return Err(Error::InvalidParameterValue);
        }
        let index = self
            .sources
            .binary_search_by_key(&id, |source| source.id)
            .map_err(|_| Error::InvalidId)?;
        let source = self.sources.get_mut(index).ok_or(Error::InvalidId)?;
        let SourceState::Lfo { oscillator, .. } = &mut source.committed else {
            return Err(Error::UnsupportedProcessingClass);
        };
        oscillator
            .reset(phase)
            .map_err(|_| Error::InvalidParameterValue)?;
        source.scratch = source.committed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_modulation_rolls_back_and_restarts_from_the_authored_seed() {
        let id = ModulationSourceId::new(1).unwrap();
        let source = PreparedModulationSource::random_hold(id, 8, 0, 2.0).unwrap();
        let mut bank = ModulationSourceBank::new(&[source], 8).unwrap();
        let observe = |bank: &ModulationSourceBank| {
            bank.samples(id)
                .unwrap()
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>()
        };
        let expected = [0x3f44_4150; 4]
            .into_iter()
            .chain([0xbe0c_3b10; 4])
            .collect::<Vec<_>>();
        bank.begin();
        bank.render_chunk(8).unwrap();
        assert_eq!(observe(&bank), expected);
        bank.rollback();
        bank.begin();
        bank.render_chunk(8).unwrap();
        assert_eq!(observe(&bank), expected);
        bank.commit().unwrap();
        bank.begin();
        bank.render_chunk(4).unwrap();
        assert_eq!(observe(&bank), [0xbf72_7746; 4]);
        bank.rollback();
        bank.reset_all();
        bank.begin();
        bank.render_chunk(8).unwrap();
        assert_eq!(observe(&bank), expected);
    }

    #[test]
    fn follower_requires_finite_input_and_refusal_leaves_other_sources_untouched() {
        let oscillator = source(1, 12_000.0, 0.0);
        let follower = PreparedModulationSource::envelope_follower(
            ModulationSourceId::new(2).unwrap(),
            48_000,
            0.0,
            0.0,
        )
        .unwrap();
        let mut bank = ModulationSourceBank::new(&[oscillator, follower], 4).unwrap();
        bank.begin();
        assert_eq!(bank.render_chunk(2), Err(Error::InvalidParameterValue));
        assert_eq!(
            bank.render_chunk_with_input(2, None, Some(&[[0.25, -0.5], [f32::NAN, 0.0]])),
            Err(Error::InvalidParameterValue),
        );
        let signal = [[0.25, -0.5], [0.0, -2.0]];
        bank.render_chunk_with_input(signal.len(), None, Some(&signal))
            .unwrap();
        assert_eq!(bank.samples(follower.id), Some([0.5, 1.0].as_slice()));
        assert_eq!(bank.samples(oscillator.id), Some([0.0, 1.0].as_slice()));
        bank.rollback();
        bank.begin();
        bank.render_chunk_with_input(signal.len(), None, Some(&signal))
            .unwrap();
        assert_eq!(bank.samples(oscillator.id), Some([0.0, 1.0].as_slice()));
    }

    fn source(id: u64, rate: f64, phase: f64) -> PreparedModulationSource {
        PreparedModulationSource::free(ModulationSourceId::new(id).unwrap(), 48_000.0, rate, phase)
            .unwrap()
    }

    #[test]
    fn rollback_replays_and_chunking_keeps_the_sample_sequence() {
        let prepared = source(7, 97.0, 0.2);
        let mut bank = ModulationSourceBank::new(&[prepared], 64).unwrap();
        let SourceState::Lfo {
            oscillator: mut oracle,
            ..
        } = prepared.initial
        else {
            panic!("lfo fixture");
        };
        let mut expected = [0.0; 64];
        oracle.render(&mut expected);
        bank.begin();
        bank.render_chunk(17).unwrap();
        assert_eq!(bank.samples(prepared.id).unwrap(), &expected[..17]);
        bank.render_chunk(47).unwrap();
        assert_eq!(bank.samples(prepared.id).unwrap(), &expected[17..]);
        // Simulate a later stage refusing after both chunks were rendered.
        bank.rollback();
        bank.begin();
        bank.render_chunk(64).unwrap();
        assert_eq!(bank.samples(prepared.id).unwrap(), expected);
        bank.commit().unwrap();
        oracle.render(&mut expected);
        bank.begin();
        bank.render_chunk(64).unwrap();
        assert_eq!(bank.samples(prepared.id).unwrap(), expected);
    }

    #[test]
    fn replacement_matches_identity_and_retains_candidate_rates() {
        let old_sources = [source(1, 12_000.0, 0.0), source(3, 6_000.0, 0.0)];
        let mut previous = ModulationSourceBank::new(&old_sources, 8).unwrap();
        previous.begin();
        previous.render_chunk(1).unwrap();
        previous.commit().unwrap();
        let next_sources = [source(1, 6_000.0, 0.0), source(2, 12_000.0, 0.0)];
        let mut next = ModulationSourceBank::new(&next_sources, 8).unwrap();
        next.inherit_committed(&previous).unwrap();
        next.begin();
        next.render_chunk(2).unwrap();
        let retained = next.samples(next_sources[0].id).unwrap();
        assert!((retained[0] - 1.0).abs() < 1e-6);
        assert!((retained[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        let inserted = next.samples(next_sources[1].id).unwrap();
        assert!(inserted[0].abs() < 1e-6);
        assert!((inserted[1] - 1.0).abs() < 1e-6);
        assert!(next.samples(old_sources[1].id).is_none());
    }

    #[test]
    fn waveform_replacement_keeps_phase_through_tempo_and_rollback() {
        let id = ModulationSourceId::new(1).unwrap();
        let sine = PreparedModulationSource::tempo(id, 8.0, 60.0, 1.0, 0.0).unwrap();
        let square = PreparedModulationSource::tempo_with_waveform(
            id,
            8.0,
            60.0,
            1.0,
            0.0,
            LfoWaveform::Square,
        )
        .unwrap();
        let mut previous = ModulationSourceBank::new(&[sine], 8).unwrap();
        previous.begin();
        previous.render_chunk(3).unwrap();
        previous.commit().unwrap();
        let mut candidate = ModulationSourceBank::new(&[square], 8).unwrap();
        candidate.inherit_committed(&previous).unwrap();
        candidate.begin();
        candidate.render_chunk(1).unwrap();
        assert_eq!(candidate.samples(id).unwrap(), &[1.0]);
        candidate.render_chunk(5).unwrap();
        assert_eq!(
            candidate.samples(id).unwrap(),
            &[-1.0, -1.0, -1.0, -1.0, 1.0]
        );
        candidate.rollback();
        candidate.begin();
        candidate.render_chunk(6).unwrap();
        assert_eq!(
            candidate.samples(id).unwrap(),
            &[1.0, -1.0, -1.0, -1.0, -1.0, 1.0]
        );
        candidate.commit().unwrap();
    }

    #[test]
    fn tempo_change_is_speculative_and_keeps_phase() {
        let id = ModulationSourceId::new(1).unwrap();
        let original = PreparedModulationSource::tempo(id, 8.0, 120.0, 1.0, 0.0).unwrap();
        let changed = PreparedModulationSource::tempo(id, 8.0, 60.0, 1.0, 0.0).unwrap();
        let mut bank = ModulationSourceBank::new(&[original], 8).unwrap();
        assert!(bank.retarget(changed).is_err());
        bank.begin();
        bank.render_chunk(1).unwrap();
        bank.retarget(changed).unwrap();
        bank.render_chunk(2).unwrap();
        let samples = bank.samples(original.id).unwrap();
        assert!((samples[0] - 1.0).abs() < 1e-6);
        assert!((samples[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        bank.rollback();
        bank.begin();
        bank.render_chunk(2).unwrap();
        assert_eq!(bank.samples(original.id).unwrap(), &[0.0, 1.0]);
        bank.retarget(changed).unwrap();
        bank.commit().unwrap();
        bank.begin();
        bank.render_chunk(2).unwrap();
        let samples = bank.samples(original.id).unwrap();
        assert!(samples[0].abs() < 1e-6);
        assert!((samples[1] + std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn invalid_requests_do_not_advance_committed_or_speculative_state() {
        assert!(ModulationSourceId::new(0).is_err());
        let prepared = source(1, 12_000.0, 0.0);
        assert!(
            PreparedModulationSource::tempo(prepared.id, 48_000.0, f64::NAN, 1.0, 0.0).is_err()
        );
        assert!(PreparedModulationSource::tempo(prepared.id, 48_000.0, 120.0, 0.0, 0.0).is_err());
        assert!(ModulationSourceBank::new(&[prepared, prepared], 8).is_err());
        assert!(ModulationSourceBank::new(&[prepared], 0).is_err());
        assert!(ModulationSourceBank::new(&[prepared], usize::MAX).is_err());
        let many: Vec<_> = (1..=32).map(|id| source(id, 1.0, 0.0)).collect();
        assert!(ModulationSourceBank::new(&many, 8192).is_err());
        let mut bank = ModulationSourceBank::new(&[prepared], 8).unwrap();
        assert_eq!(bank.source_count(), 1);
        assert_eq!(bank.source_id(0), Some(prepared.id));
        assert_eq!(bank.source_id(1), None);
        assert_eq!(bank.chunk_frames(), 8);
        assert!(bank.render_chunk(1).is_err());
        assert!(bank.commit().is_err());
        assert!(bank.reset(prepared.id, f64::NAN).is_err());
        bank.begin();
        assert!(bank.render_chunk(9).is_err());
        assert!(bank.reset(prepared.id, 0.0).is_err());
        assert!(bank.retarget(source(2, 100.0, 0.0)).is_err());
        let wrong_rate = PreparedModulationSource::free(prepared.id, 44_100.0, 100.0, 0.0).unwrap();
        assert!(bank.retarget(wrong_rate).is_err());
        let mut replacement = ModulationSourceBank::new(&[prepared], 8).unwrap();
        assert!(replacement.inherit_committed(&bank).is_err());
        bank.render_chunk(2).unwrap();
        assert_eq!(bank.samples(prepared.id).unwrap(), &[0.0, 1.0]);
        assert!(bank.render_chunk(9).is_err());
        assert_eq!(bank.samples(prepared.id).unwrap(), &[0.0, 1.0]);
        bank.render_chunk(1).unwrap();
        assert!(bank.samples(prepared.id).unwrap()[0].abs() < 1e-6);
        bank.rollback();
        bank.reset(prepared.id, 0.25).unwrap();
        bank.begin();
        bank.render_chunk(1).unwrap();
        assert_eq!(bank.samples(prepared.id).unwrap(), &[1.0]);
    }
    struct TempoClock {
        start: usize,
        invalid: bool,
        queries: std::cell::Cell<usize>,
    }
    impl NativeInstrumentClock for TempoClock {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
            self.queries.set(self.queries.get() + 1);
            let frame = self.start + offset;
            if self.invalid && frame == 3 {
                return Some(0);
            }
            match frame % 5 {
                0..=2 => Some(120_000_000),
                3 => Some(60_000_000),
                _ => None,
            }
        }
    }
    fn clock(start: usize, invalid: bool) -> TempoClock {
        TempoClock {
            start,
            invalid,
            queries: std::cell::Cell::new(0),
        }
    }

    #[test]
    fn host_tempo_changes_and_missing_clock_are_sample_exact_and_partition_invariant() {
        let id = ModulationSourceId::new(1).unwrap();
        let sources = [
            PreparedModulationSource::tempo(id, 8.0, 90.0, 1.0, 0.125).unwrap(),
            PreparedModulationSource::tempo(
                ModulationSourceId::new(2).unwrap(),
                8.0,
                90.0,
                1.0,
                0.125,
            )
            .unwrap(),
        ];
        let mut whole = ModulationSourceBank::new(&sources, 8).unwrap();
        let clock = clock(0, false);
        whole.begin();
        whole.render_chunk_with_clock(8, Some(&clock)).unwrap();
        assert_eq!(clock.queries.get(), 8);
        let actual = whole.samples(id).unwrap().to_vec();
        let mut phase = 0.125_f64;
        for (frame, &sample) in actual.iter().enumerate() {
            assert!((f64::from(sample) - (std::f64::consts::TAU * phase).sin()).abs() < 1e-6);
            let bpm = match frame % 5 {
                0..=2 => 120.0,
                3 => 60.0,
                _ => 90.0,
            };
            phase = (phase + bpm / 60.0 / 8.0).fract();
        }
        let mut partitioned = ModulationSourceBank::new(&sources, 8).unwrap();
        partitioned.begin();
        let mut pieces = Vec::new();
        for (start, frames) in [(0, 3), (3, 1), (4, 4)] {
            partitioned
                .render_chunk_with_clock(frames, Some(&self::clock(start, false)))
                .unwrap();
            pieces.extend_from_slice(partitioned.samples(id).unwrap());
        }
        assert_eq!(pieces, actual);
        partitioned.rollback();
        partitioned.begin();
        partitioned
            .render_chunk_with_clock(8, Some(&self::clock(0, false)))
            .unwrap();
        assert_eq!(partitioned.samples(id).unwrap(), actual);
        let before = whole.samples(id).unwrap().to_vec();
        assert!(
            whole
                .render_chunk_with_clock(8, Some(&self::clock(0, true)))
                .is_err()
        );
        assert_eq!(whole.samples(id).unwrap(), before);
        whole
            .render_chunk_with_clock(8, Some(&self::clock(8, false)))
            .unwrap();
        partitioned
            .render_chunk_with_clock(8, Some(&self::clock(8, false)))
            .unwrap();
        assert_eq!(whole.samples(id), partitioned.samples(id));
    }

    #[test]
    fn timing_mode_retarget_commit_rollback_reset_and_inheritance_preserve_phase() {
        let id = ModulationSourceId::new(1).unwrap();
        let free = PreparedModulationSource::free(id, 8.0, 2.0, 0.0).unwrap();
        let tempo = PreparedModulationSource::tempo(id, 8.0, 60.0, 1.0, 0.0).unwrap();
        let mut bank = ModulationSourceBank::new(&[free], 3).unwrap();
        bank.begin();
        bank.render_chunk(1).unwrap();
        bank.retarget(tempo).unwrap();
        bank.render_chunk(1).unwrap();
        assert_eq!(bank.samples(id).unwrap(), &[1.0]);
        bank.rollback();
        bank.begin();
        bank.render_chunk(3).unwrap();
        for (&actual, expected) in bank.samples(id).unwrap().iter().zip([0.0, 1.0, 0.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        bank.rollback();
        bank.begin();
        bank.render_chunk(1).unwrap();
        bank.retarget(tempo).unwrap();
        bank.render_chunk(1).unwrap();
        bank.commit().unwrap();
        bank.begin();
        bank.render_chunk(2).unwrap();
        for (&actual, phase) in bank.samples(id).unwrap().iter().zip([0.375_f64, 0.5]) {
            assert!((f64::from(actual) - (std::f64::consts::TAU * phase).sin()).abs() < 1e-6);
        }
        bank.commit().unwrap();
        let mut candidate = ModulationSourceBank::new(&[free], 3).unwrap();
        candidate.inherit_committed(&bank).unwrap();
        candidate.begin();
        candidate.render_chunk(2).unwrap();
        for (&actual, phase) in candidate
            .samples(id)
            .unwrap()
            .iter()
            .zip([0.625_f64, 0.875])
        {
            assert!((f64::from(actual) - (std::f64::consts::TAU * phase).sin()).abs() < 1e-6);
        }
        bank.reset_all();
        bank.begin();
        // The initial free mode ignores the deliberately invalid tempo query.
        bank.render_chunk_with_clock(3, Some(&clock(3, true)))
            .unwrap();
        for (&actual, expected) in bank.samples(id).unwrap().iter().zip([0.0, 1.0, 0.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn tempo_above_a_source_limit_refuses_the_entire_bank_before_mutation() {
        struct Fast;
        impl NativeInstrumentClock for Fast {
            fn beat_at_offset(&self, _: usize) -> Option<u64> {
                None
            }
            fn tempo_micro_bpm_at_offset(&self, _: usize) -> Option<u32> {
                Some(u32::MAX)
            }
        }
        let first = PreparedModulationSource::tempo(
            ModulationSourceId::new(1).unwrap(),
            48_000.0,
            60.0,
            4.0,
            0.0,
        )
        .unwrap();
        let second = PreparedModulationSource::tempo(
            ModulationSourceId::new(2).unwrap(),
            48_000.0,
            60.0,
            0.001,
            0.0,
        )
        .unwrap();
        let mut bank = ModulationSourceBank::new(&[first, second], 4).unwrap();
        let mut oracle = ModulationSourceBank::new(&[first, second], 4).unwrap();
        bank.begin();
        oracle.begin();
        bank.render_chunk(4).unwrap();
        oracle.render_chunk(4).unwrap();
        let before = bank.samples(first.id).unwrap().to_vec();
        assert!(bank.render_chunk_with_clock(4, Some(&Fast)).is_err());
        assert_eq!(bank.samples(first.id).unwrap(), before);
        bank.render_chunk(4).unwrap();
        oracle.render_chunk(4).unwrap();
        assert_eq!(bank.samples(first.id), oracle.samples(first.id));
        assert_eq!(bank.samples(second.id), oracle.samples(second.id));
    }
}
