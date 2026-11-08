//! Standalone original four-operator DSP with independently shaped amplitudes.
use super::{Invalid, OPERATORS, Oscillator, PreparedFmCore, Routing, SineTable};
use crate::operator_envelope::{OperatorEnvelope, PreparedOperatorEnvelopes};
use crate::operator_waveform::{
    OperatorWaveform, OperatorWaveformBank, harmonic_cap_index, prepared_bank,
};
mod context;
mod feedback;
mod retarget;
mod tuning;
pub use context::{PreparedOperatorConfiguration, PreparedOperatorNote};
pub use feedback::PreparedOperatorFeedbackVoice;
pub(crate) use retarget::OperatorFeedbackRetargetFrame;
pub use retarget::{OperatorFeedbackParameters, PreparedOperatorFeedbackRetarget};
pub(crate) use tuning::FINE_LIMIT_HZ;
pub use tuning::OperatorTuning;

/// Original immutable acyclic FM note, not a registered native device.
/// Each envelope shapes its oscillator before modulation and carrier routing.
/// The caller supplies actual note-relative age and gate duration in frames.
#[derive(Debug, Clone, Copy)]
pub struct PreparedOperatorFm4 {
    core: PreparedFmCore,
    envelopes: PreparedOperatorEnvelopes,
    velocity: f64,
    waveforms: [OperatorWaveform; OPERATORS],
    waveform_bank: Option<&'static OperatorWaveformBank>,
    harmonic_caps: [u8; OPERATORS],
    amplitude_velocity_scales: [f32; OPERATORS],
    amplitude_velocity_active: bool,
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
        Self::new_with_waveforms(
            sample_rate,
            pitch,
            velocity,
            oscillators,
            routing,
            envelopes,
            [OperatorWaveform::Sine; OPERATORS],
        )
    }

    /// Prepare a standalone Operator note with per-operator periodic shapes.
    /// Non-sine tables are initialized and retained on the control side only.
    pub fn new_with_waveforms(
        sample_rate: u32,
        pitch: u8,
        velocity: u8,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
        waveforms: [OperatorWaveform; OPERATORS],
    ) -> Result<Self, Invalid> {
        Self::new_with_tuning(
            sample_rate,
            pitch,
            velocity,
            oscillators,
            routing,
            envelopes,
            waveforms,
            [OperatorTuning::Ratio; OPERATORS],
        )
    }

    /// Prepare fixed or ratio tuning on control, retaining the legacy default.
    #[allow(clippy::too_many_arguments)] // Mirrors the existing standalone constructor plus tuning.
    pub fn new_with_tuning(
        sample_rate: u32,
        pitch: u8,
        velocity: u8,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
        waveforms: [OperatorWaveform; OPERATORS],
        tuning: [OperatorTuning; OPERATORS],
    ) -> Result<Self, Invalid> {
        Self::new_with_tuning_and_amplitude_velocity(
            sample_rate,
            pitch,
            velocity,
            oscillators,
            routing,
            envelopes,
            waveforms,
            tuning,
            [0.0; OPERATORS],
        )
    }

    /// Prepare a standalone note with per-oscillator velocity sensitivity.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_tuning_and_amplitude_velocity(
        sample_rate: u32,
        pitch: u8,
        velocity: u8,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelopes: [OperatorEnvelope; OPERATORS],
        waveforms: [OperatorWaveform; OPERATORS],
        tuning: [OperatorTuning; OPERATORS],
        amplitude_velocity: [f32; OPERATORS],
    ) -> Result<Self, Invalid> {
        if !(1..=127).contains(&velocity) {
            return Err(Invalid("invalid operator note-on velocity"));
        }
        if amplitude_velocity
            .iter()
            .any(|amount| !amount.is_finite() || !(0.0..=1.0).contains(amount))
        {
            return Err(Invalid("invalid operator amplitude velocity amount"));
        }
        let mut amplitude_velocity_scales =
            amplitude_velocity_scales(amplitude_velocity, f64::from(velocity) / 127.0);
        let steps = silence_unrepresentable(
            tuning::prepared_steps(
                sample_rate,
                pitch,
                &oscillators.map(|oscillator| oscillator.ratio),
                &tuning,
            )?,
            &mut amplitude_velocity_scales,
        );
        let core = PreparedFmCore {
            configuration: super::PreparedFmConfiguration::new(oscillators, routing),
            steps,
        };
        let mut harmonic_caps = [0_u8; OPERATORS];
        for index in 0..OPERATORS {
            if waveforms[index] != OperatorWaveform::Sine {
                harmonic_caps[index] = harmonic_cap_index(core.steps[index]).ok_or(Invalid(
                    "quantized operator phase step has no below-nyquist table",
                ))?;
            }
        }
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
            core,
            envelopes: PreparedOperatorEnvelopes::new(sample_rate, envelopes)?,
            velocity: f64::from(velocity) / 127.0,
            waveforms,
            waveform_bank,
            harmonic_caps,
            amplitude_velocity_active: amplitude_velocity_scales != [1.0; OPERATORS],
            amplitude_velocity_scales,
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
        let mut amplitudes = self.envelopes.levels(age_frames, gate_frames);
        if amplitudes.iter().all(|amplitude| *amplitude == 0.0) {
            return 0.0;
        }
        if self.amplitude_velocity_active {
            scale_operator_amplitudes(&mut amplitudes, self.amplitude_velocity_scales);
        }
        let routed = if self.waveforms == [OperatorWaveform::Sine; OPERATORS] {
            self.core
                .routed_output::<true>(table, age_frames, amplitudes)
        } else if let Some(bank) = self.waveform_bank {
            self.core.routed_output_with_waveforms(
                table,
                bank,
                age_frames,
                amplitudes,
                (&self.waveforms, &self.harmonic_caps),
            )
        } else {
            return 0.0;
        };
        (routed * self.velocity) as f32
    }
}

pub(super) fn amplitude_velocity_scales(
    amounts: [f32; OPERATORS],
    normalized_velocity: f64,
) -> [f32; OPERATORS] {
    let velocity = normalized_velocity as f32;
    amounts.map(|amount| 1.0 - amount + amount * velocity)
}

/// Resolve per-note steps. A silenced oscillator keeps a legal stand-in step
/// and a zero amplitude scale, so every sampling, feedback and filter path
/// leaves it out of the note while the remaining oscillators still render.
pub(super) fn silence_unrepresentable(
    steps: tuning::NoteSteps,
    amplitude_scales: &mut [f32; OPERATORS],
) -> [u64; OPERATORS] {
    std::array::from_fn(|index| {
        steps[index].unwrap_or_else(|| {
            amplitude_scales[index] = 0.0;
            tuning::SILENCED_STEP
        })
    })
}

pub(super) fn scale_operator_amplitudes(
    amplitudes: &mut [f64; OPERATORS],
    scales: [f32; OPERATORS],
) {
    for (amplitude, scale) in amplitudes.iter_mut().zip(scales) {
        *amplitude *= f64::from(scale);
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tuning_tests;
