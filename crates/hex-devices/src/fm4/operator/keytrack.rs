//! Control-prepared pitch tracking between fixed and keyed oscillator tuning.
//! Exact endpoints retain their original arithmetic; fractional tracking scales
//! played pitch intervals in log frequency space. Sampling consumes phase steps.
use super::{Invalid, OPERATORS, OperatorTuning};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Tracking {
    amount: f64,
    fixed_frequency_log2: f64,
    fixed_multiplier_log2: f64,
}

impl Tracking {
    pub(super) fn base_and_multiplier(self, note_hz: f64, ratio: f64) -> (f64, f64) {
        (
            (self.fixed_frequency_log2
                + self.amount * (note_hz.log2() - self.fixed_frequency_log2))
                .exp2(),
            (self.fixed_multiplier_log2
                + self.amount * (ratio.log2() - self.fixed_multiplier_log2))
                .exp2(),
        )
    }
}

pub(super) type Amounts = [Option<Tracking>; OPERATORS];

pub(super) struct Prepared {
    pub(super) tuning: [OperatorTuning; OPERATORS],
    pub(super) tracking: Amounts,
}

pub(super) fn prepare(
    amounts_percent: [f32; OPERATORS],
    fixed_anchors: [OperatorTuning; OPERATORS],
) -> Result<Prepared, Invalid> {
    let mut result = Prepared {
        tuning: [OperatorTuning::Ratio; OPERATORS],
        tracking: [None; OPERATORS],
    };
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
        if amount == 0.0 {
            result.tuning[i] = fixed_anchors[i];
        } else if amount != 100.0 {
            result.tracking[i] = Some(Tracking {
                amount: f64::from(amount) / 100.0,
                fixed_frequency_log2: frequency_hz.log2(),
                fixed_multiplier_log2: multiplier.log2(),
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
