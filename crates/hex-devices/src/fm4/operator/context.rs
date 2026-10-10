//! Shared original DSP configuration with compact, independently prepared notes.
use super::*;
use crate::fm4::{PreparedFmConfiguration, prepared_steps};

/// Immutable rate-specific original Operator configuration, with no heap owners.
/// The caller keeps it alive for every note prepared against it. Native host
/// identity, ownership and retirement are separate Lattice responsibilities.
#[derive(Debug, Clone)]
pub struct PreparedOperatorConfiguration {
    sample_rate: u32,
    ratios: [f64; OPERATORS],
    configuration: PreparedFmConfiguration,
    envelopes: PreparedOperatorEnvelopes,
}

/// Compact pitch/velocity data. Sample only against its preparing configuration.
/// No oscillator graph or envelope is copied into an individual note.
#[derive(Debug, Clone, Copy)]
pub struct PreparedOperatorNote {
    steps: [u64; OPERATORS],
    velocity: f64,
}

impl PreparedOperatorConfiguration {
    pub fn new(
        sample_rate: u32,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
    ) -> Result<Self, Invalid> {
        let envelopes = PreparedOperatorEnvelopes::new(sample_rate, envelopes)?;
        Ok(Self {
            sample_rate,
            ratios: oscillators.map(|oscillator| oscillator.ratio),
            configuration: PreparedFmConfiguration::new(oscillators, routing),
            envelopes,
        })
    }

    /// Validate every oscillator's Nyquist limit and compute pitch on control.
    pub fn prepare_note(&self, pitch: u8, velocity: u8) -> Result<PreparedOperatorNote, Invalid> {
        if !(1..=127).contains(&velocity) {
            return Err(Invalid("invalid operator note-on velocity"));
        }
        Ok(PreparedOperatorNote {
            steps: prepared_steps(self.sample_rate, pitch, &self.ratios)?,
            velocity: f64::from(velocity) / 127.0,
        })
    }

    pub fn release_frames(&self) -> u64 {
        self.envelopes.maximum_tail_frames()
    }

    /// Borrow context and note; no replay, owners, allocation or initialization.
    pub fn sample(
        &self,
        table: &SineTable,
        note: &PreparedOperatorNote,
        age_frames: u64,
        gate_frames: u64,
    ) -> f32 {
        let amplitudes = self.envelopes.levels(age_frames, gate_frames);
        if amplitudes.iter().all(|amplitude| *amplitude == 0.0) {
            return 0.0;
        }
        (self
            .configuration
            .routed_output::<true>(table, age_frames, &note.steps, amplitudes)
            * note.velocity) as f32
    }
}

impl PreparedOperatorNote {
    /// Encode only the already prepared phase increments and velocity. The
    /// shared-context host supplies the configuration separately.
    pub(crate) fn into_shared_payload(
        self,
    ) -> [u64; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS] {
        let mut payload = [0; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[..OPERATORS].copy_from_slice(&self.steps);
        payload[OPERATORS] = self.velocity.to_bits();
        payload
    }

    /// Decode the private, control-created payload without allocation. Reject
    /// malformed values defensively; supported notes are emitted by
    /// `PreparedOperatorConfiguration::prepare_note` above.
    pub(crate) fn from_shared_payload(
        payload: &[u64; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS],
    ) -> Option<Self> {
        if payload[OPERATORS + 1..].iter().any(|word| *word != 0) {
            return None;
        }
        let steps = std::array::from_fn(|index| payload[index]);
        if steps.contains(&0) {
            return None;
        }
        let velocity = f64::from_bits(payload[OPERATORS]);
        if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
            return None;
        }
        Some(Self { steps, velocity })
    }
}

#[cfg(test)]
mod tests;
