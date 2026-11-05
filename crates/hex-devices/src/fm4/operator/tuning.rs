//! Control-side oscillator tuning. No frequency conversion occurs while sampling.
use super::{Invalid, OPERATORS};

/// Original fixed-frequency bounds, distinct from measured reference laws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OperatorTuning {
    Ratio,
    Fixed { frequency_hz: f64, multiplier: f64 },
}

impl OperatorTuning {
    pub fn fixed(frequency_hz: f64, multiplier: f64) -> Result<Self, Invalid> {
        if !frequency_hz.is_finite()
            || !(0.1..=20_000.0).contains(&frequency_hz)
            || !multiplier.is_finite()
            || !(f64::from(0.01_f32)..=1_000.0).contains(&multiplier)
        {
            return Err(Invalid("invalid operator fixed frequency or multiplier"));
        }
        Ok(Self::Fixed {
            frequency_hz,
            multiplier,
        })
    }

    pub(super) fn validate(self) -> Result<(), Invalid> {
        if let Self::Fixed {
            frequency_hz,
            multiplier,
        } = self
        {
            Self::fixed(frequency_hz, multiplier)?;
        }
        Ok(())
    }
}

/// Per-note phase steps; `None` marks an oscillator silenced for this note.
pub(super) type NoteSteps = [Option<u64>; OPERATORS];

/// Stand-in phase step stored for a silenced oscillator. It is the smallest
/// representable step, so every harmonic cap and LFO excursion stays legal;
/// the note's zero amplitude scale keeps the oscillator out of every output.
pub(super) const SILENCED_STEP: u64 = 1;

/// One oscillator's quantized phase step on control. A frequency at or above
/// Nyquist silences only that oscillator for the note instead of refusing it.
pub(super) fn oscillator_step(sample_rate: u32, frequency: f64) -> Result<Option<u64>, Invalid> {
    if !frequency.is_finite() || frequency <= 0.0 {
        return Err(Invalid("invalid operator oscillator frequency"));
    }
    if frequency >= f64::from(sample_rate) * 0.5 {
        return Ok(None);
    }
    crate::quantized_phase_step(sample_rate, frequency).map(Some)
}

pub(super) fn prepared_steps(
    sample_rate: u32,
    pitch: u8,
    ratios: &[f64; OPERATORS],
    tuning: &[OperatorTuning; OPERATORS],
) -> Result<NoteSteps, Invalid> {
    prepared_steps_with_table(sample_rate, pitch, ratios, tuning, None)
}
pub(super) fn prepared_steps_with_table(
    sample_rate: u32,
    pitch: u8,
    ratios: &[f64; OPERATORS],
    tuning: &[OperatorTuning; OPERATORS],
    table: Option<&hex_lattice::TuningTable>,
) -> Result<NoteSteps, Invalid> {
    if !(8_000..=192_000).contains(&sample_rate) || pitch > 127 {
        return Err(Invalid("invalid operator sample rate or pitch"));
    }
    let note_hz = match table {
        Some(table) => table
            .frequency(pitch)
            .map_err(|_| Invalid("invalid operator pitch"))?,
        None => 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0),
    };
    let mut steps = [None; OPERATORS];
    for index in 0..OPERATORS {
        tuning[index].validate()?;
        // Ratio arithmetic matches the plain FM core, preserving quantization.
        let frequency = match tuning[index] {
            OperatorTuning::Ratio => note_hz * ratios[index],
            OperatorTuning::Fixed {
                frequency_hz,
                multiplier,
            } => frequency_hz * multiplier,
        };
        steps[index] = oscillator_step(sample_rate, frequency)?;
    }
    Ok(steps)
}
