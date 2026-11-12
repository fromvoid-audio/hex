//! Compact control-side pitch policy. Reuse the fixed anchors instead of caching
//! log coefficients beside a second tuning representation. Log conversion occurs
//! during note preparation only; sampling consumes the existing phase steps.
use super::{Invalid, OPERATORS, OperatorTuning};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Prepared {
    frequency_hz: [f64; OPERATORS],
    multipliers: [f64; OPERATORS],
    amounts_percent: [f32; OPERATORS],
}

impl Prepared {
    /// Input modes were validated by the configuration constructor.
    pub(super) fn legacy(modes: [OperatorTuning; OPERATORS]) -> Self {
        let mut result = Self {
            frequency_hz: [0.0; OPERATORS],
            multipliers: [0.0; OPERATORS],
            amounts_percent: [100.0; OPERATORS],
        };
        for (i, mode) in modes.into_iter().enumerate() {
            if let OperatorTuning::Fixed {
                frequency_hz,
                multiplier,
            } = mode
            {
                result.frequency_hz[i] = frequency_hz;
                result.multipliers[i] = multiplier;
                result.amounts_percent[i] = 0.0;
            }
        }
        result
    }

    pub(super) fn percentages(self) -> [f32; OPERATORS] {
        self.amounts_percent
    }

    pub(super) fn fractional(self) -> bool {
        self.amounts_percent
            .iter()
            .any(|v| *v != 0.0 && *v != 100.0)
    }

    pub(super) fn legacy_modes(self) -> [OperatorTuning; OPERATORS] {
        std::array::from_fn(|i| {
            if self.amounts_percent[i] == 0.0 {
                OperatorTuning::Fixed {
                    frequency_hz: self.frequency_hz[i],
                    multiplier: self.multipliers[i],
                }
            } else {
                OperatorTuning::Ratio
            }
        })
    }

    pub(super) fn base_and_multiplier(self, i: usize, note_hz: f64, ratio: f64) -> (f64, f64) {
        let percent = self.amounts_percent[i];
        if percent == 100.0 {
            return (note_hz, ratio);
        }
        if percent == 0.0 {
            return (self.frequency_hz[i], self.multipliers[i]);
        }
        let amount = f64::from(percent) / 100.0;
        let base = self.frequency_hz[i].log2();
        let multiplier = self.multipliers[i].log2();
        (
            (base + amount * (note_hz.log2() - base)).exp2(),
            (multiplier + amount * (ratio.log2() - multiplier)).exp2(),
        )
    }
}

pub(super) fn prepare(
    amounts_percent: [f32; OPERATORS],
    fixed_anchors: [OperatorTuning; OPERATORS],
) -> Result<Prepared, Invalid> {
    let mut result = Prepared::legacy([OperatorTuning::Ratio; OPERATORS]);
    for (i, amount) in amounts_percent.into_iter().enumerate() {
        if !amount.is_finite() || !(0.0..=100.0).contains(&amount) {
            return Err(Invalid("invalid operator pitch keytrack amount"));
        }
        fixed_anchors[i].validate()?;
        let OperatorTuning::Fixed {
            frequency_hz,
            multiplier,
        } = fixed_anchors[i]
        else {
            return Err(Invalid(
                "operator keytrack requires fixed frequency anchors",
            ));
        };
        if amount != 100.0 {
            result.frequency_hz[i] = frequency_hz;
            result.multipliers[i] = multiplier;
            result.amounts_percent[i] = amount;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
