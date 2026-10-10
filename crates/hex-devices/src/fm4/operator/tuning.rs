//! Control-side oscillator tuning. No frequency conversion occurs while sampling.
use super::{Invalid, OPERATORS};
use crate::PHASE_SCALE;

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

pub(super) fn prepared_steps(
    sample_rate: u32,
    pitch: u8,
    ratios: &[f64; OPERATORS],
    tuning: &[OperatorTuning; OPERATORS],
) -> Result<[u64; OPERATORS], Invalid> {
    if !(8_000..=192_000).contains(&sample_rate) || pitch > 127 {
        return Err(Invalid("invalid operator sample rate or pitch"));
    }
    if tuning == &[OperatorTuning::Ratio; OPERATORS] {
        // Preserve legacy ratio quantization and PCM exactly.
        return crate::fm4::prepared_steps(sample_rate, pitch, ratios);
    }
    let note_hz = 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0);
    let mut steps = [0; OPERATORS];
    for index in 0..OPERATORS {
        tuning[index].validate()?;
        let frequency = match tuning[index] {
            OperatorTuning::Ratio => note_hz * ratios[index],
            OperatorTuning::Fixed {
                frequency_hz,
                multiplier,
            } => frequency_hz * multiplier,
        };
        if !frequency.is_finite() || frequency <= 0.0 || frequency >= f64::from(sample_rate) * 0.5 {
            return Err(Invalid(
                "operator oscillator frequency must be below nyquist",
            ));
        }
        steps[index] = (frequency / f64::from(sample_rate) * PHASE_SCALE).round() as u64;
    }
    Ok(steps)
}
