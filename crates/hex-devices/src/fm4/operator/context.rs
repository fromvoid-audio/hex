//! Shared original DSP configuration with compact, independently prepared notes.
use super::*;
use crate::fm4::PreparedFmConfiguration;
use crate::operator_waveform::{
    OperatorWaveform, OperatorWaveformBank, harmonic_cap_index, is_canonical_harmonic_cap,
    prepared_bank,
};

/// Immutable rate-specific original Operator configuration, with no heap owners.
/// The caller keeps it alive for every note prepared against it. Native host
/// identity, ownership and retirement are separate Lattice responsibilities.
#[derive(Debug, Clone)]
pub struct PreparedOperatorConfiguration {
    sample_rate: u32,
    ratios: [f64; OPERATORS],
    tuning: [OperatorTuning; OPERATORS],
    configuration: PreparedFmConfiguration,
    envelopes: PreparedOperatorEnvelopes,
    waveforms: [OperatorWaveform; OPERATORS],
    waveform_bank: Option<&'static OperatorWaveformBank>,
}

/// Compact pitch/velocity data. Sample only against its preparing configuration.
/// No oscillator graph or envelope is copied into an individual note.
#[derive(Debug, Clone, Copy)]
pub struct PreparedOperatorNote {
    steps: [u64; OPERATORS],
    velocity: f64,
    harmonic_caps: u32,
}

impl PreparedOperatorConfiguration {
    pub fn new(
        sample_rate: u32,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
    ) -> Result<Self, Invalid> {
        Self::new_with_waveforms(
            sample_rate,
            oscillators,
            routing,
            envelopes,
            [OperatorWaveform::Sine; OPERATORS],
        )
    }

    /// Prepare a waveform-aware configuration. Any shared table-bank
    /// initialization occurs here on the caller's control thread.
    pub fn new_with_waveforms(
        sample_rate: u32,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
        waveforms: [OperatorWaveform; OPERATORS],
    ) -> Result<Self, Invalid> {
        Self::new_with_tuning(
            sample_rate,
            oscillators,
            routing,
            envelopes,
            waveforms,
            [OperatorTuning::Ratio; OPERATORS],
        )
    }

    /// Immutable per-oscillator tuning. Fixed frequencies ignore note pitch.
    pub fn new_with_tuning(
        sample_rate: u32,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
        waveforms: [OperatorWaveform; OPERATORS],
        tuning: [OperatorTuning; OPERATORS],
    ) -> Result<Self, Invalid> {
        for mode in tuning {
            mode.validate()?;
        }
        let envelopes = PreparedOperatorEnvelopes::new(sample_rate, envelopes)?;
        let waveform_bank = if waveforms
            .iter()
            .any(|waveform| *waveform != OperatorWaveform::Sine)
        {
            Some(
                prepared_bank()
                    .map_err(|_| Invalid("periodic waveform table allocation failed"))?,
            )
        } else {
            None
        };
        Ok(Self {
            sample_rate,
            ratios: oscillators.map(|oscillator| oscillator.ratio),
            tuning,
            configuration: PreparedFmConfiguration::new(oscillators, routing),
            envelopes,
            waveforms,
            waveform_bank,
        })
    }

    /// Validate every oscillator's Nyquist limit and compute pitch on control.
    pub fn prepare_note(&self, pitch: u8, velocity: u8) -> Result<PreparedOperatorNote, Invalid> {
        if !(1..=127).contains(&velocity) {
            return Err(Invalid("invalid operator note-on velocity"));
        }
        let steps =
            super::tuning::prepared_steps(self.sample_rate, pitch, &self.ratios, &self.tuning)?;
        let mut caps = [0_u8; OPERATORS];
        for index in 0..OPERATORS {
            if self.waveforms[index] != OperatorWaveform::Sine {
                caps[index] = harmonic_cap_index(steps[index]).ok_or(Invalid(
                    "quantized operator phase step has no below-nyquist table",
                ))?;
            }
        }
        Ok(PreparedOperatorNote {
            steps,
            velocity: f64::from(velocity) / 127.0,
            harmonic_caps: pack_caps(caps),
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
        let routed = if self.waveforms == [OperatorWaveform::Sine; OPERATORS] {
            // Preserve the previous arithmetic and lookup path bit-for-bit.
            self.configuration
                .routed_output::<true>(table, age_frames, &note.steps, amplitudes)
        } else if let Some(bank) = self.waveform_bank {
            let harmonic_caps = unpack_caps(note.harmonic_caps);
            self.configuration.routed_output_with_waveforms(
                table,
                bank,
                age_frames,
                &note.steps,
                amplitudes,
                (&self.waveforms, &harmonic_caps),
            )
        } else {
            return 0.0;
        };
        (routed * note.velocity) as f32
    }

    pub(crate) fn note_matches_configuration(&self, note: &PreparedOperatorNote) -> bool {
        if self.waveforms == [OperatorWaveform::Sine; OPERATORS] {
            return note.harmonic_caps == 0;
        }
        let caps = unpack_caps(note.harmonic_caps);
        (0..OPERATORS).all(|index| {
            if self.waveforms[index] == OperatorWaveform::Sine {
                caps[index] == 0
            } else {
                is_canonical_harmonic_cap(note.steps[index], caps[index])
            }
        })
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
        payload[5] = u64::from(self.harmonic_caps);
        payload
    }

    /// Decode the private, control-created payload without allocation. Reject
    /// malformed values defensively; supported notes are emitted by
    /// `PreparedOperatorConfiguration::prepare_note` above.
    pub(crate) fn from_shared_payload(
        payload: &[u64; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS],
    ) -> Option<Self> {
        if payload[6..].iter().any(|word| *word != 0) {
            return None;
        }
        let steps = std::array::from_fn(|index| payload[index]);
        let harmonic_caps = u32::try_from(payload[5]).ok()?;
        if steps.contains(&0) || unpack_caps(harmonic_caps).iter().any(|cap| *cap >= 10) {
            return None;
        }
        let velocity = f64::from_bits(payload[OPERATORS]);
        if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
            return None;
        }
        Some(Self {
            steps,
            velocity,
            harmonic_caps,
        })
    }
}

const fn pack_caps(caps: [u8; OPERATORS]) -> u32 {
    u32::from_le_bytes(caps)
}

const fn unpack_caps(packed: u32) -> [u8; OPERATORS] {
    packed.to_le_bytes()
}

#[cfg(test)]
mod tests;
