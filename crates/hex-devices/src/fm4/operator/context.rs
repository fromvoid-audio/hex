//! Shared original DSP configuration with compact, independently prepared notes.
use super::retarget::OperatorFeedbackParameters;
use super::*;
use crate::fm4::PreparedFmConfiguration;
use crate::operator_waveform::{
    OperatorWaveform, OperatorWaveformBank, harmonic_cap_index, is_canonical_harmonic_cap,
    prepared_bank,
};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CONFIGURATION_ID: AtomicU64 = AtomicU64::new(1);
const MAX_CONFIGURATION_ID: u64 = u64::MAX >> 1;

/// Immutable rate-specific original Operator configuration, with no heap owners.
/// The caller keeps it alive for every note prepared against it. Native host
/// identity, ownership and retirement are separate Lattice responsibilities.
#[derive(Debug, Clone)]
pub struct PreparedOperatorConfiguration {
    sample_rate: u32,
    configuration_id: u64,
    ratios: [f64; OPERATORS],
    tuning: [OperatorTuning; OPERATORS],
    configuration: PreparedFmConfiguration,
    oscillator_levels: [f64; OPERATORS],
    envelopes: PreparedOperatorEnvelopes,
    waveforms: [OperatorWaveform; OPERATORS],
    amplitude_velocity: [f32; OPERATORS],
    waveform_bank: Option<&'static OperatorWaveformBank>,
}

/// Compact pitch/velocity data. Sample only against its preparing configuration.
/// No oscillator graph or envelope is copied into an individual note.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PreparedOperatorNote {
    steps: [u64; OPERATORS],
    velocity: f64,
    harmonic_caps: u32,
    amplitude_velocity_scales: [f32; OPERATORS],
    amplitude_velocity_active: bool,
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
        Self::new_with_tuning_and_amplitude_velocity(
            sample_rate,
            oscillators,
            routing,
            envelopes,
            waveforms,
            tuning,
            [0.0; OPERATORS],
        )
    }

    /// Prepare per-oscillator amplitude response to note-on velocity.
    pub fn new_with_tuning_and_amplitude_velocity(
        sample_rate: u32,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
        waveforms: [OperatorWaveform; OPERATORS],
        tuning: [OperatorTuning; OPERATORS],
        amplitude_velocity: [f32; OPERATORS],
    ) -> Result<Self, Invalid> {
        if amplitude_velocity
            .iter()
            .any(|amount| !amount.is_finite() || !(0.0..=1.0).contains(amount))
        {
            return Err(Invalid("invalid operator amplitude velocity amount"));
        }
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
        let configuration_id = NEXT_CONFIGURATION_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                if current < MAX_CONFIGURATION_ID {
                    current.checked_add(1)
                } else {
                    None
                }
            })
            .map_err(|_| Invalid("operator configuration identity exhausted"))?;
        Ok(Self {
            sample_rate,
            configuration_id,
            ratios: oscillators.map(|oscillator| oscillator.ratio),
            tuning,
            configuration: PreparedFmConfiguration::new(oscillators, routing),
            oscillator_levels: oscillators.map(|oscillator| oscillator.level),
            envelopes,
            waveforms,
            amplitude_velocity,
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
        let amplitude_velocity_scales =
            super::amplitude_velocity_scales(self.amplitude_velocity, f64::from(velocity) / 127.0);
        Ok(PreparedOperatorNote {
            steps,
            velocity: f64::from(velocity) / 127.0,
            harmonic_caps: pack_caps(caps),
            amplitude_velocity_active: amplitude_velocity_scales != [1.0; OPERATORS],
            amplitude_velocity_scales,
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
        let mut amplitudes = self.envelopes.levels(age_frames, gate_frames);
        if amplitudes.iter().all(|amplitude| *amplitude == 0.0) {
            return 0.0;
        }
        if note.amplitude_velocity_active {
            super::scale_operator_amplitudes(&mut amplitudes, note.amplitude_velocity_scales);
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

    /// Bounded sequential pitch integration, sharing the existing FM/envelope law.
    /// Harmonic caps conservatively cover the full one-octave LFO excursion.
    pub(crate) fn prepare_lfo_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedOperatorNote, Invalid> {
        let mut note = self.prepare_note(pitch, velocity)?;
        if note.steps.iter().any(|step| *step >= (1_u64 << 62)) {
            return Err(Invalid("LFO excursion must remain below nyquist"));
        }
        let mut caps = [0; OPERATORS];
        for (i, cap) in caps.iter_mut().enumerate() {
            if self.waveforms[i] != OperatorWaveform::Sine {
                *cap = harmonic_cap_index(note.steps[i] * 2).ok_or(Invalid("LFO harmonic cap"))?;
            }
        }
        note.harmonic_caps = pack_caps(caps);
        Ok(note)
    }

    pub(crate) fn lfo_steps(note: &PreparedOperatorNote) -> [u64; OPERATORS] {
        note.steps
    }

    #[allow(clippy::too_many_arguments)] // Explicit borrowed note, integrated phase and feedback recurrence.
    pub(crate) fn sample_lfo(
        &self,
        table: &SineTable,
        note: &PreparedOperatorNote,
        age: u64,
        gate: u64,
        phases: &[u64; OPERATORS],
        feedback: [f64; OPERATORS],
        previous: [f64; OPERATORS],
    ) -> (f32, [f64; OPERATORS]) {
        self.sample_lfo_with_levels(table, note, age, gate, phases, feedback, previous, None)
    }

    #[allow(clippy::too_many_arguments)] // Explicit integrated phases, recurrence and shared scalar override.
    pub(crate) fn sample_lfo_with_levels(
        &self,
        table: &SineTable,
        note: &PreparedOperatorNote,
        age: u64,
        gate: u64,
        phases: &[u64; OPERATORS],
        feedback: [f64; OPERATORS],
        previous: [f64; OPERATORS],
        levels: Option<[f64; OPERATORS]>,
    ) -> (f32, [f64; OPERATORS]) {
        let mut configuration = self.configuration;
        if let Some(levels) = levels {
            configuration.levels = levels;
        }
        let mut amplitudes = self.envelopes.levels(age, gate);
        if amplitudes == [0.0; OPERATORS] {
            return (0.0, [0.0; OPERATORS]);
        }
        if note.amplitude_velocity_active {
            super::scale_operator_amplitudes(&mut amplitudes, note.amplitude_velocity_scales);
        }
        let feedback = std::array::from_fn(|i| feedback[i] * previous[i]);
        let (routed, signals) = if self.waveforms == [OperatorWaveform::Sine; OPERATORS] {
            configuration
                .routed_output_with_feedback::<true>(table, 1, phases, amplitudes, feedback)
        } else if let Some(bank) = self.waveform_bank {
            configuration.routed_output_with_waveforms_and_feedback(
                table,
                bank,
                (1, phases),
                amplitudes,
                (&self.waveforms, &unpack_caps(note.harmonic_caps)),
                feedback,
            )
        } else {
            return (0.0, [0.0; OPERATORS]);
        };
        ((routed * note.velocity) as f32, signals)
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

    pub(super) fn feedback_amounts_are_valid(&self, feedback_turns: [f64; OPERATORS]) -> bool {
        if feedback_turns
            .iter()
            .any(|amount| !amount.is_finite() || !(0.0..=1.0).contains(amount))
        {
            return false;
        }
        (0..OPERATORS).all(|target| {
            feedback_turns[target] == 0.0
                || (0..OPERATORS).all(|source| {
                    source == target || self.configuration.routing.modulation[source][target] == 0.0
                })
        })
    }

    pub(crate) fn feedback_parameters(
        &self,
        feedback_turns: [f64; OPERATORS],
    ) -> Result<OperatorFeedbackParameters, Invalid> {
        if !self.feedback_amounts_are_valid(feedback_turns) {
            return Err(Invalid("invalid Operator feedback retarget values"));
        }
        Ok(OperatorFeedbackParameters {
            oscillator_levels: self.oscillator_levels,
            carrier_weights: self.configuration.routing.carriers,
            feedback_turns,
        })
    }

    /// Immutable enable continuation: borrow the unchanged note and override
    /// only validated levels, with zero feedback. No owners or voice limits.
    pub(crate) fn sample_with_oscillator_levels(
        &self,
        table: &SineTable,
        note: &PreparedOperatorNote,
        age_frames: u64,
        gate_frames: u64,
        levels: [f64; OPERATORS],
    ) -> Result<f32, Invalid> {
        if levels
            .iter()
            .any(|level| !level.is_finite() || !(0.0..=8.0).contains(level))
            || !self.note_matches_configuration(note)
        {
            return Err(Invalid("invalid Operator enable continuation"));
        }
        let mut parameters = self.feedback_parameters([0.0; OPERATORS])?;
        parameters.oscillator_levels = levels;
        Ok(self
            .sample_with_feedback_parameters(
                table,
                note,
                age_frames,
                gate_frames,
                parameters,
                [0.0; OPERATORS],
            )
            .0)
    }

    pub(crate) const fn configuration_id(&self) -> u64 {
        self.configuration_id
    }

    pub(crate) const fn effective_oscillator_levels(&self) -> [f64; OPERATORS] {
        self.oscillator_levels
    }

    pub(crate) fn retarget_compatible_with(&self, target: &PreparedOperatorConfiguration) -> bool {
        self.sample_rate == target.sample_rate
            && self.ratios == target.ratios
            && self.tuning == target.tuning
            && self.configuration.phases == target.configuration.phases
            && self.configuration.routing.modulation == target.configuration.routing.modulation
            && self.configuration.routing.order == target.configuration.routing.order
            && self.envelopes == target.envelopes
            && self.waveforms == target.waveforms
            && self.amplitude_velocity == target.amplitude_velocity
    }

    pub(super) fn identical_to(&self, target: &PreparedOperatorConfiguration) -> bool {
        self.retarget_compatible_with(target)
            && self.oscillator_levels == target.oscillator_levels
            && self.configuration.routing.carriers == target.configuration.routing.carriers
    }

    pub(super) fn sample_with_feedback(
        &self,
        table: &SineTable,
        note: &PreparedOperatorNote,
        age_frames: u64,
        gate_frames: u64,
        feedback_turns: [f64; OPERATORS],
        previous_outputs: [f64; OPERATORS],
    ) -> (f32, [f64; OPERATORS]) {
        let mut amplitudes = self.envelopes.levels(age_frames, gate_frames);
        if amplitudes.iter().all(|amplitude| *amplitude == 0.0) {
            return (0.0, [0.0; OPERATORS]);
        }
        if note.amplitude_velocity_active {
            super::scale_operator_amplitudes(&mut amplitudes, note.amplitude_velocity_scales);
        }
        let phase_modulation =
            std::array::from_fn(|index| previous_outputs[index] * feedback_turns[index]);
        let (routed, signals) = if self.waveforms == [OperatorWaveform::Sine; OPERATORS] {
            self.configuration.routed_output_with_feedback::<true>(
                table,
                age_frames,
                &note.steps,
                amplitudes,
                phase_modulation,
            )
        } else if let Some(bank) = self.waveform_bank {
            let harmonic_caps = unpack_caps(note.harmonic_caps);
            self.configuration
                .routed_output_with_waveforms_and_feedback(
                    table,
                    bank,
                    (age_frames, &note.steps),
                    amplitudes,
                    (&self.waveforms, &harmonic_caps),
                    phase_modulation,
                )
        } else {
            return (0.0, [0.0; OPERATORS]);
        };
        ((routed * note.velocity) as f32, signals)
    }

    #[inline]
    pub(super) fn sample_with_feedback_parameters(
        &self,
        table: &SineTable,
        note: &PreparedOperatorNote,
        age_frames: u64,
        gate_frames: u64,
        parameters: OperatorFeedbackParameters,
        previous_outputs: [f64; OPERATORS],
    ) -> (f32, [f64; OPERATORS]) {
        let mut configuration = self.configuration;
        configuration.levels = parameters.oscillator_levels;
        configuration.routing.carriers = parameters.carrier_weights;
        let mut amplitudes = self.envelopes.levels(age_frames, gate_frames);
        if amplitudes.iter().all(|amplitude| *amplitude == 0.0) {
            return (0.0, [0.0; OPERATORS]);
        }
        if note.amplitude_velocity_active {
            super::scale_operator_amplitudes(&mut amplitudes, note.amplitude_velocity_scales);
        }
        let phase_modulation =
            std::array::from_fn(|index| previous_outputs[index] * parameters.feedback_turns[index]);
        let (routed, signals) = if self.waveforms == [OperatorWaveform::Sine; OPERATORS] {
            configuration.routed_output_with_feedback::<true>(
                table,
                age_frames,
                &note.steps,
                amplitudes,
                phase_modulation,
            )
        } else if let Some(bank) = self.waveform_bank {
            let harmonic_caps = unpack_caps(note.harmonic_caps);
            configuration.routed_output_with_waveforms_and_feedback(
                table,
                bank,
                (age_frames, &note.steps),
                amplitudes,
                (&self.waveforms, &harmonic_caps),
                phase_modulation,
            )
        } else {
            return (0.0, [0.0; OPERATORS]);
        };
        ((routed * note.velocity) as f32, signals)
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
        payload[5] = u64::from(
            self.harmonic_caps
                | if self.amplitude_velocity_active {
                    1 << 31
                } else {
                    0
                },
        );
        payload[6] = u64::from(self.amplitude_velocity_scales[0].to_bits())
            | (u64::from(self.amplitude_velocity_scales[1].to_bits()) << 32);
        payload[7] = u64::from(self.amplitude_velocity_scales[2].to_bits())
            | (u64::from(self.amplitude_velocity_scales[3].to_bits()) << 32);
        payload
    }

    /// Decode the private, control-created payload without allocation. Reject
    /// malformed values defensively; supported notes are emitted by
    /// `PreparedOperatorConfiguration::prepare_note` above.
    pub(crate) fn from_shared_payload(
        payload: &[u64; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS],
    ) -> Option<Self> {
        let steps = std::array::from_fn(|index| payload[index]);
        let encoded_caps = u32::try_from(payload[5]).ok()?;
        let amplitude_velocity_active = encoded_caps & (1 << 31) != 0;
        let harmonic_caps = encoded_caps & !(1 << 31);
        if steps.contains(&0) || unpack_caps(harmonic_caps).iter().any(|cap| *cap >= 10) {
            return None;
        }
        let velocity = f64::from_bits(payload[OPERATORS]);
        if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
            return None;
        }
        let amplitude_velocity_scales =
            if payload[6] == 0 && payload[7] == 0 && !amplitude_velocity_active {
                // Earlier Operator contexts left these two words reserved-zero.
                [1.0; OPERATORS]
            } else {
                let unpack = |word: u64| {
                    [
                        f32::from_bits(word as u32),
                        f32::from_bits((word >> 32) as u32),
                    ]
                };
                let [a, b] = unpack(payload[6]);
                let [c, d] = unpack(payload[7]);
                [a, b, c, d]
            };
        if amplitude_velocity_scales
            .iter()
            .any(|scale| !scale.is_finite() || !(0.0..=1.0).contains(scale))
            || amplitude_velocity_active != (amplitude_velocity_scales != [1.0; OPERATORS])
        {
            return None;
        }
        Some(Self {
            steps,
            velocity,
            harmonic_caps,
            amplitude_velocity_active,
            amplitude_velocity_scales,
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
