//! Transactional sample generation for a host-owned bank of reusable LFOs.
//!
//! Construction and destruction are control-side. The bank owns fixed source
//! state and preallocated output buffers, but no clock, graph, transport,
//! scheduling, source registry or publication authority. The enclosing graph
//! begins one transaction, renders as many bounded chunks as needed, and then
//! commits or abandons it with the rest of the audio block. Each chunk's samples
//! remain available until the next render. The host splits longer segments and
//! defines whether bypass, stop, seek, loops and retrigger advance or reset a
//! source; this helper never infers those events from block boundaries.
use hex_dsp::ModulationLfo;
use hex_lattice_contract::DeviceContractError as Error;

pub const MAX_MODULATION_LFO_SOURCES: usize = 32;
pub const MAX_MODULATION_LFO_STORAGE_BYTES: usize = 262_144;
pub const MAX_MODULATION_LFO_CHUNK_FRAMES: usize = 8192;

/// Stable, nonzero source identity, scoped by the owning device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModulationSourceId(u64);
impl ModulationSourceId {
    pub fn new(value: u64) -> Result<Self, Error> {
        if value == 0 {
            return Err(Error::InvalidId);
        }
        Ok(Self(value))
    }
}

/// A source prepared on control; its initial phase and rate have been validated.
#[derive(Clone, Copy, Debug)]
pub struct PreparedLfoSource {
    pub id: ModulationSourceId,
    pub lfo: ModulationLfo,
}
impl PreparedLfoSource {
    /// Prepare a free-running source without exposing DSP internals to the host.
    pub fn free(
        id: ModulationSourceId,
        sample_rate: f64,
        frequency_hz: f64,
        phase: f64,
    ) -> Result<Self, Error> {
        let rate = hex_dsp::LfoRate::free(sample_rate, frequency_hz)
            .map_err(|_| Error::InvalidParameterValue)?;
        let lfo = ModulationLfo::new(rate, phase).map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self { id, lfo })
    }

    /// Prepare a tempo-synced source using quarter-note beats per cycle.
    pub fn tempo(
        id: ModulationSourceId,
        sample_rate: f64,
        beats_per_minute: f64,
        beats_per_cycle: f64,
        phase: f64,
    ) -> Result<Self, Error> {
        let rate = hex_dsp::LfoRate::tempo(sample_rate, beats_per_minute, beats_per_cycle)
            .map_err(|_| Error::InvalidParameterValue)?;
        let lfo = ModulationLfo::new(rate, phase).map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self { id, lfo })
    }
}

struct Source {
    id: ModulationSourceId,
    committed: ModulationLfo,
    scratch: ModulationLfo,
    samples: Vec<f32>,
}

/// Source state and scratch storage borrowed exclusively by the graph owner.
pub struct ModulationLfoBank {
    sources: Vec<Source>,
    chunk_frames: usize,
    last_frames: usize,
    active: bool,
}
impl ModulationLfoBank {
    /// IDs must be unique and strictly increasing. The host accounts this
    /// storage alongside its existing paired-runtime budget before publication.
    pub fn new(sources: &[PreparedLfoSource], chunk_frames: usize) -> Result<Self, Error> {
        if sources.len() > MAX_MODULATION_LFO_SOURCES
            || chunk_frames == 0
            || chunk_frames > MAX_MODULATION_LFO_CHUNK_FRAMES
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let bytes = sources
            .len()
            .checked_mul(chunk_frames)
            .and_then(|samples| samples.checked_mul(std::mem::size_of::<f32>()))
            .ok_or(Error::RuntimeBudgetExceeded)?;
        let minimum = sources
            .len()
            .checked_mul(std::mem::size_of::<Source>())
            .and_then(|size| size.checked_add(bytes))
            .and_then(|size| size.checked_add(std::mem::size_of::<Self>()))
            .ok_or(Error::RuntimeBudgetExceeded)?;
        if minimum > MAX_MODULATION_LFO_STORAGE_BYTES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        if sources.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err(Error::InvalidId);
        }
        let mut prepared = Vec::with_capacity(sources.len());
        for source in sources {
            prepared.push(Source {
                id: source.id,
                committed: source.lfo,
                scratch: source.lfo,
                samples: vec![0.0; chunk_frames],
            });
        }
        let bank = Self {
            sources: prepared,
            chunk_frames,
            last_frames: 0,
            active: false,
        };
        if bank.storage_bytes()? > MAX_MODULATION_LFO_STORAGE_BYTES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        Ok(bank)
    }

    /// Actual reserved element capacities and inline state, for host admission.
    /// Allocator bookkeeping is outside this count.
    pub fn storage_bytes(&self) -> Result<usize, Error> {
        let mut bytes = self
            .sources
            .capacity()
            .checked_mul(std::mem::size_of::<Source>())
            .and_then(|size| size.checked_add(std::mem::size_of::<Self>()))
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

    /// Start or restart a transaction from committed phase, discarding scratch.
    pub fn begin(&mut self) {
        for source in &mut self.sources {
            source.scratch = source.committed;
        }
        self.last_frames = 0;
        self.active = true;
    }

    /// Generate one chunk. Refusal leaves both phase and last samples untouched.
    pub fn render_chunk(&mut self, frames: usize) -> Result<(), Error> {
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
        for source in &mut self.sources {
            let output = source
                .samples
                .get_mut(..frames)
                .ok_or(Error::RuntimeBudgetExceeded)?;
            source.scratch.render(output);
        }
        self.last_frames = frames;
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

    /// Copy committed phase by identity while retaining each candidate's rate.
    /// Called only by the existing graph owner after its replacement preflight.
    pub fn inherit_committed(&mut self, previous: &Self) -> Result<(), Error> {
        if self.active || previous.active {
            return Err(Error::InvalidParameterValue);
        }
        for source in &mut self.sources {
            if let Ok(index) = previous
                .sources
                .binary_search_by_key(&source.id, |old| old.id)
            {
                let Some(old) = previous.sources.get(index) else {
                    return Err(Error::InvalidId);
                };
                source.committed.inherit_phase(&old.committed);
                source.scratch = source.committed;
            }
        }
        Ok(())
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
        source
            .committed
            .reset(phase)
            .map_err(|_| Error::InvalidParameterValue)?;
        source.scratch = source.committed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_dsp::LfoRate;

    fn source(id: u64, rate: f64, phase: f64) -> PreparedLfoSource {
        PreparedLfoSource {
            id: ModulationSourceId::new(id).unwrap(),
            lfo: ModulationLfo::new(LfoRate::free(48_000.0, rate).unwrap(), phase).unwrap(),
        }
    }

    #[test]
    fn rollback_replays_and_chunking_keeps_the_sample_sequence() {
        let prepared = source(7, 97.0, 0.2);
        let mut bank = ModulationLfoBank::new(&[prepared], 64).unwrap();
        let mut oracle = prepared.lfo;
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
        let mut previous = ModulationLfoBank::new(&old_sources, 8).unwrap();
        previous.begin();
        previous.render_chunk(1).unwrap();
        previous.commit().unwrap();
        let next_sources = [source(1, 6_000.0, 0.0), source(2, 12_000.0, 0.0)];
        let mut next = ModulationLfoBank::new(&next_sources, 8).unwrap();
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
    fn invalid_requests_do_not_advance_committed_or_speculative_state() {
        assert!(ModulationSourceId::new(0).is_err());
        let prepared = source(1, 12_000.0, 0.0);
        assert!(ModulationLfoBank::new(&[prepared, prepared], 8).is_err());
        assert!(ModulationLfoBank::new(&[prepared], 0).is_err());
        assert!(ModulationLfoBank::new(&[prepared], usize::MAX).is_err());
        let many: Vec<_> = (1..=32).map(|id| source(id, 1.0, 0.0)).collect();
        assert!(ModulationLfoBank::new(&many, 8192).is_err());
        let mut bank = ModulationLfoBank::new(&[prepared], 8).unwrap();
        assert!(bank.render_chunk(1).is_err());
        assert!(bank.commit().is_err());
        assert!(bank.reset(prepared.id, f64::NAN).is_err());
        bank.begin();
        assert!(bank.render_chunk(9).is_err());
        assert!(bank.reset(prepared.id, 0.0).is_err());
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
}
