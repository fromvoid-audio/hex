//! Standalone original four-operator DSP with independently shaped amplitudes.
use super::{Invalid, OPERATORS, Oscillator, PreparedFmCore, Routing, SineTable};
use crate::operator_envelope::{OperatorEnvelope, PreparedOperatorEnvelopes};

/// Original immutable acyclic FM note, not a registered native device.
/// Each envelope shapes its oscillator before modulation and carrier routing.
/// The caller supplies actual note-relative age and gate duration in frames.
#[derive(Debug, Clone, Copy)]
pub struct PreparedOperatorFm4 {
    core: PreparedFmCore,
    envelopes: PreparedOperatorEnvelopes,
    velocity: f64,
}

impl PreparedOperatorFm4 {
    pub fn new(
        sample_rate: u32,
        pitch: u8,
        velocity: u8,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
    ) -> Result<Self, Invalid> {
        if !(1..=127).contains(&velocity) {
            return Err(Invalid("invalid operator note-on velocity"));
        }
        Ok(Self {
            core: PreparedFmCore::new(sample_rate, pitch, oscillators, routing)?,
            envelopes: PreparedOperatorEnvelopes::new(sample_rate, envelopes)?,
            velocity: f64::from(velocity) / 127.0,
        })
    }

    /// Maximum envelope release duration; excludes feedback/filter/effect tails.
    pub fn release_frames(&self) -> u64 {
        self.envelopes.maximum_tail_frames()
    }

    /// Borrow a control-created table. No envelope replay, mutable voice state,
    /// allocation or lazy initialization occurs here. No limiter/antialiasing;
    /// amplitude is bounded by authored carrier weights/levels (at most 32).
    pub fn sample(&self, table: &SineTable, age_frames: u64, gate_frames: u64) -> f32 {
        let amplitudes = self.envelopes.levels(age_frames, gate_frames);
        if amplitudes.iter().all(|amplitude| *amplitude == 0.0) {
            return 0.0;
        }
        (self
            .core
            .routed_output::<true>(table, age_frames, amplitudes)
            * self.velocity) as f32
    }
}

#[cfg(test)]
mod tests;
