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

fn next_configuration_id() -> Result<u64, Invalid> {
    NEXT_CONFIGURATION_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            if current < MAX_CONFIGURATION_ID {
                current.checked_add(1)
            } else {
                None
            }
        })
        .map_err(|_| Invalid("operator configuration identity exhausted"))
}

/// Immutable rate-specific original Operator configuration, with no heap owners.
/// The caller keeps it alive for every note prepared against it. Native host
/// identity, ownership and retirement are separate Lattice responsibilities.
#[derive(Debug, Clone)]
pub struct PreparedOperatorConfiguration {
    sample_rate: u32,
    configuration_id: u64,
    ratios: [f64; OPERATORS],
    tuning: [OperatorTuning; OPERATORS],
    fine_offsets_hz: [f32; OPERATORS],
    pitch_keytrack: super::keytrack::Amounts,
    configuration: PreparedFmConfiguration,
    oscillator_levels: [f64; OPERATORS],
    envelopes: PreparedOperatorEnvelopes,
    waveforms: [OperatorWaveform; OPERATORS],
    amplitude_velocity: [f32; OPERATORS],
    waveform_bank: Option<&'static OperatorWaveformBank>,
    key_amounts: [f32; OPERATORS],
    oscillator_velocity: [f32; OPERATORS],
    oscillator_quantize: [bool; OPERATORS],
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
    key_scaling: bool,
    pitch: u8,
    midi_velocity: u8,
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
        let configuration_id = next_configuration_id()?;
        Ok(Self {
            sample_rate,
            configuration_id,
            ratios: oscillators.map(|oscillator| oscillator.ratio),
            tuning,
            fine_offsets_hz: [0.0; OPERATORS],
            pitch_keytrack: [None; OPERATORS],
            configuration: PreparedFmConfiguration::new(oscillators, routing),
            oscillator_levels: oscillators.map(|oscillator| oscillator.level),
            envelopes,
            waveforms,
            amplitude_velocity,
            waveform_bank,
            key_amounts: [0.0; OPERATORS],
            oscillator_velocity: [0.0; OPERATORS],
            oscillator_quantize: [false; OPERATORS],
        })
    }

    /// Prepare independent 0–100% note-pitch tracking. Every anchor must be a
    /// validated `Fixed` tuning: 0% uses it exactly, 100% uses note Hz × ratio.
    /// Fractional amounts interpolate base frequency and multiplier in log space;
    /// velocity/quantization then modify the multiplier, followed by fine Hz.
    /// A changed policy receives a new configuration identity before publication.
    pub fn with_pitch_keytrack(
        mut self,
        amounts_percent: [f32; OPERATORS],
        fixed_anchors: [OperatorTuning; OPERATORS],
    ) -> Result<Self, Invalid> {
        let prepared = super::keytrack::prepare(amounts_percent, fixed_anchors)?;
        if self.tuning != prepared.tuning || self.pitch_keytrack != prepared.tracking {
            self.configuration_id = next_configuration_id()?;
            self.tuning = prepared.tuning;
            self.pitch_keytrack = prepared.tracking;
        }
        Ok(self)
    }

    /// Add signed Hz after ratio/fixed multiplication during control-side note preparation.
    pub(crate) fn with_fine_offsets(mut self, offsets: [f32; OPERATORS]) -> Result<Self, Invalid> {
        if offsets
            .iter()
            .any(|value| !value.is_finite() || value.abs() > super::tuning::FINE_LIMIT_HZ)
        {
            return Err(Invalid("invalid operator fine frequency offset"));
        }
        self.fine_offsets_hz = offsets;
        Ok(self)
    }

    pub(crate) const fn fine_offsets_hz(&self) -> [f32; OPERATORS] {
        self.fine_offsets_hz
    }

    /// Control-prepared modifiers retain the existing compact note payload.
    pub(crate) fn with_note_modifiers(
        mut self,
        key: [f32; 4],
        oscillator_velocity: [f32; 4],
        quantize: [bool; 4],
    ) -> Result<Self, Invalid> {
        if key
            .iter()
            .chain(&oscillator_velocity)
            .any(|v| !v.is_finite() || !(-1.0..=1.0).contains(v))
        {
            return Err(Invalid("invalid operator note modifier"));
        }
        self.key_amounts = key;
        self.oscillator_velocity = oscillator_velocity;
        self.oscillator_quantize = quantize;
        Ok(self)
    }
    pub(crate) fn standalone_note(
        &self,
        pitch: u8,
        velocity: u8,
        envelopes: [OperatorEnvelope; 4],
    ) -> Result<PreparedOperatorFm4, Invalid> {
        let note = self.prepare_note(pitch, velocity)?;
        Ok(PreparedOperatorFm4 {
            core: PreparedFmCore {
                configuration: self.configuration,
                steps: note.steps,
            },
            envelopes: PreparedOperatorEnvelopes::new(self.sample_rate, envelopes)?,
            velocity: note.velocity,
            waveforms: self.waveforms,
            waveform_bank: self.waveform_bank,
            harmonic_caps: unpack_caps(note.harmonic_caps),
            amplitude_velocity_scales: note.amplitude_velocity_scales,
            amplitude_velocity_active: note.amplitude_velocity_active,
        })
    }
    fn modified_steps(
        &self,
        pitch: u8,
        velocity: u8,
        table: Option<&hex_lattice::TuningTable>,
    ) -> Result<super::tuning::NoteSteps, Invalid> {
        if self.oscillator_velocity == [0.0; 4]
            && self.fine_offsets_hz == [0.0; 4]
            && self.pitch_keytrack == [None; OPERATORS]
        {
            return super::tuning::prepared_steps_with_table(
                self.sample_rate,
                pitch,
                &self.ratios,
                &self.tuning,
                table,
            );
        }
        if pitch > 127 {
            return Err(Invalid("invalid operator pitch"));
        }
        let note_hz = match table {
            Some(table) => table
                .frequency(pitch)
                .map_err(|_| Invalid("invalid operator pitch"))?,
            None => 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0),
        };
        let mut steps = [None; 4];
        for (i, step) in steps.iter_mut().enumerate() {
            let amount = f64::from(self.oscillator_velocity[i]);
            let factor = if amount == 0.0 {
                1.0
            } else {
                2.0_f64.powf(4.0 * amount * f64::from(velocity) / 127.0)
            };
            let (base, multiplier) = if let Some(tracking) = self.pitch_keytrack[i] {
                tracking.base_and_multiplier(note_hz, self.ratios[i])
            } else {
                match self.tuning[i] {
                    OperatorTuning::Ratio => (note_hz, self.ratios[i]),
                    OperatorTuning::Fixed {
                        frequency_hz,
                        multiplier,
                    } => (frequency_hz, multiplier),
                }
            };
            let mut multiplier = multiplier * factor;
            if amount != 0.0 && self.oscillator_quantize[i] {
                multiplier = multiplier.round().max(1.0);
            }
            let frequency = base * multiplier + f64::from(self.fine_offsets_hz[i]);
            *step = if frequency <= 0.0 {
                None
            } else {
                super::tuning::oscillator_step(self.sample_rate, frequency)?
            };
        }
        Ok(steps)
    }

    /// Compute pitch on control. An oscillator at/below zero or at/above Nyquist is silenced
    /// for this note only; the note and its other oscillators are still admitted.
    pub fn prepare_note(&self, pitch: u8, velocity: u8) -> Result<PreparedOperatorNote, Invalid> {
        self.prepare_note_with_tuning(pitch, velocity, None)
    }

    pub(crate) fn prepare_note_with_tuning(
        &self,
        pitch: u8,
        velocity: u8,
        table: Option<&hex_lattice::TuningTable>,
    ) -> Result<PreparedOperatorNote, Invalid> {
        if !(1..=127).contains(&velocity) {
            return Err(Invalid("invalid operator note-on velocity"));
        }
        let steps = self.modified_steps(pitch, velocity, table)?;
        let mut amplitude_velocity_scales =
            super::amplitude_velocity_scales(self.amplitude_velocity, f64::from(velocity) / 127.0);
        let steps = super::silence_unrepresentable(steps, &mut amplitude_velocity_scales);
        let mut caps = [0_u8; OPERATORS];
        for index in 0..OPERATORS {
            if self.waveforms[index] != OperatorWaveform::Sine {
                caps[index] = harmonic_cap_index(steps[index]).ok_or(Invalid(
                    "quantized operator phase step has no below-nyquist table",
                ))?;
            }
        }
        let key_scaling = self.key_amounts.iter().any(|amount| *amount != 0.0);
        if key_scaling {
            for (i, scale) in amplitude_velocity_scales.iter_mut().enumerate() {
                let gain = 2.0_f64
                    .powf(f64::from(self.key_amounts[i]) * (f64::from(pitch) - 60.0) / 12.0)
                    .clamp(1.0 / 64.0, 8.0);
                *scale = (f64::from(*scale) * gain) as f32;
            }
        }
        Ok(PreparedOperatorNote {
            pitch,
            midi_velocity: velocity,
            key_scaling,
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
    /// Harmonic caps conservatively cover the full one-octave LFO excursion; an
    /// oscillator whose excursion could reach Nyquist is silenced for this note.
    pub(crate) fn prepare_lfo_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedOperatorNote, Invalid> {
        self.prepare_lfo_note_with_tuning(pitch, velocity, None)
    }

    pub(crate) fn prepare_lfo_note_with_tuning(
        &self,
        pitch: u8,
        velocity: u8,
        table: Option<&hex_lattice::TuningTable>,
    ) -> Result<PreparedOperatorNote, Invalid> {
        let mut note = self.prepare_note_with_tuning(pitch, velocity, table)?;
        for index in 0..OPERATORS {
            if note.steps[index] >= (1_u64 << 62) {
                note.silence_oscillator(index);
            }
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

    pub(crate) fn envelope_levels(&self, age: u64, gate: u64) -> [f64; 4] {
        self.envelopes.levels(age, gate)
    }

    pub(crate) fn has_uniform_envelopes(&self) -> bool {
        self.envelopes.is_uniform()
    }

    pub(crate) fn uniform_envelope_levels(&self, age: u64, gate: u64) -> [f64; 4] {
        self.envelopes.uniform_levels(age, gate)
    }

    /// Borrow a provider-owned waveform law while preserving original routing,
    /// envelope, velocity and phase arithmetic. No preparation occurs here.
    // Specialize the render loop for its borrowed waveform sampler.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sample_with_waveform_sampler(
        &self,
        note: &PreparedOperatorNote,
        mut amplitudes: [f64; 4],
        phases: &[u64; 4],
        previous: [f64; 4],
        levels: [f64; 4],
        carriers: [f64; 4],
        feedback: [f64; 4],
        filter: Option<(
            &mut [crate::operator_filter::Carrier; 4],
            crate::operator_filter::Frame,
        )>,
        mut sample: impl FnMut(usize, u64, u64) -> f32,
    ) -> (f32, [f64; 4]) {
        if note.amplitude_velocity_active {
            super::scale_operator_amplitudes(&mut amplitudes, note.amplitude_velocity_scales);
        }
        let mut signals = [0.0; 4];
        let mut raw = [0.0; 4];
        for &target in &self.configuration.routing.order {
            if levels[target] == 0.0 {
                continue;
            }
            let mut modulation: f64 = signals
                .iter()
                .enumerate()
                .map(|(source, signal)| {
                    signal * self.configuration.routing.modulation[source][target]
                })
                .sum();
            if feedback[target] != 0.0 {
                modulation += feedback[target] * previous[target];
            }
            let modulation_phase = if modulation == 0.0 {
                0
            } else {
                crate::fm4::phase_from_turns(modulation)
            };
            let phase = phases[target]
                .wrapping_add(self.configuration.phases[target])
                .wrapping_add(modulation_phase);
            raw[target] = f64::from(sample(target, phase, note.steps[target])) * levels[target];
            signals[target] = raw[target] * amplitudes[target];
        }
        let output: f64 = if let Some((filters, frame)) = filter {
            (0..4)
                .map(|i| filters[i].process(raw[i], frame) * amplitudes[i] * carriers[i])
                .sum()
        } else {
            signals
                .iter()
                .zip(carriers)
                .map(|(signal, weight)| signal * weight)
                .sum()
        };
        ((output * note.velocity) as f32, signals)
    }

    pub(crate) fn harmonic_initial_phases(note: &PreparedOperatorNote, age: u64) -> [u64; 4] {
        note.steps.map(|step| step.wrapping_mul(age))
    }

    pub(crate) fn note_matches_configuration(&self, note: &PreparedOperatorNote) -> bool {
        if note.key_scaling != self.key_amounts.iter().any(|amount| *amount != 0.0)
            || note
                .amplitude_velocity_scales
                .iter()
                .enumerate()
                .any(|(i, scale)| *scale > if self.key_amounts[i] == 0.0 { 1.0 } else { 8.0 })
        {
            return false;
        }
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
        Ok(self.validated_feedback_parameters(feedback_turns))
    }

    /// Build parameters for feedback already accepted by `feedback_parameters`
    /// on control. Render paths use this so they never revalidate or fail.
    pub(crate) fn validated_feedback_parameters(
        &self,
        feedback_turns: [f64; OPERATORS],
    ) -> OperatorFeedbackParameters {
        OperatorFeedbackParameters {
            oscillator_levels: self.oscillator_levels,
            carrier_weights: self.configuration.routing.carriers,
            feedback_turns,
        }
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

    pub(crate) const fn sample_rate(&self) -> u32 {
        self.sample_rate
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
            && self.fine_offsets_hz == target.fine_offsets_hz
            && self.pitch_keytrack == target.pitch_keytrack
            && self.configuration.phases == target.configuration.phases
            && self.configuration.routing.modulation == target.configuration.routing.modulation
            && self.configuration.routing.order == target.configuration.routing.order
            && self.envelopes == target.envelopes
            && self.waveforms == target.waveforms
            && self.amplitude_velocity == target.amplitude_velocity
            && self.key_amounts == target.key_amounts
            && self.oscillator_velocity == target.oscillator_velocity
            && self.oscillator_quantize == target.oscillator_quantize
    }

    pub(crate) fn identical_to(&self, target: &PreparedOperatorConfiguration) -> bool {
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
    /// Leave one oscillator out of this note with a legal stand-in step.
    fn silence_oscillator(&mut self, index: usize) {
        self.steps[index] = super::tuning::SILENCED_STEP;
        self.amplitude_velocity_scales[index] = 0.0;
        self.amplitude_velocity_active = true;
    }

    pub(crate) fn midi_origin(self) -> (u8, u8) {
        (self.pitch, self.midi_velocity)
    }
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
                | if self.key_scaling { 1 << 30 } else { 0 }
                | if self.amplitude_velocity_active {
                    1 << 31
                } else {
                    0
                },
        );
        payload[5] |= (0x4f50_u64 << 48)
            | (u64::from(self.pitch) << 32)
            | (u64::from(self.midi_velocity) << 40);
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
        if payload[5] >> 48 != 0x4f50 {
            return None;
        }
        let pitch = ((payload[5] >> 32) & 255) as u8;
        let midi_velocity = ((payload[5] >> 40) & 255) as u8;
        if pitch > 127 || !(1..=127).contains(&midi_velocity) {
            return None;
        }
        let encoded_caps = payload[5] as u32;
        let amplitude_velocity_active = encoded_caps & (1 << 31) != 0;
        let key_scaling = encoded_caps & (1 << 30) != 0;
        let harmonic_caps = encoded_caps & !((1 << 31) | (1 << 30));
        if steps.contains(&0) || unpack_caps(harmonic_caps).iter().any(|cap| *cap >= 10) {
            return None;
        }
        let velocity = f64::from_bits(payload[OPERATORS]);
        if !velocity.is_finite() || velocity != f64::from(midi_velocity) / 127.0 {
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
        if amplitude_velocity_scales.iter().any(|scale| {
            !scale.is_finite() || !(0.0..=if key_scaling { 8.0 } else { 1.0 }).contains(scale)
        }) || amplitude_velocity_active != (amplitude_velocity_scales != [1.0; OPERATORS])
        {
            return None;
        }
        Some(Self {
            pitch,
            midi_velocity,
            key_scaling,
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

mod filter;
