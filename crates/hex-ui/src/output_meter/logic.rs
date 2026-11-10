//! Pure conversion of independent output sample peaks to display values.
const FLOOR_DB: f64 = -60.0;

/// Availability is supplied by the consumer; unchanged values never imply stale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeterInput {
    Offline,
    Stopped,
    Fault,
    /// Raw, independently published channel sample peaks; their age is unknown.
    Last {
        left: f32,
        right: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Peak {
    Zero,
    BelowFloor,
    Level { db: f64, fraction: f32, above: bool },
    Invalid,
}

pub(super) fn peak(amplitude: f32) -> Peak {
    if !amplitude.is_finite() || amplitude < 0.0 {
        return Peak::Invalid;
    }
    if amplitude == 0.0 {
        return Peak::Zero;
    }
    let db = if amplitude == 0.001 {
        FLOOR_DB
    } else {
        20.0 * f64::from(amplitude).log10()
    };
    // Compare in the input domain so the representable floor is not lost to
    // log rounding, and above-unity indication survives rounded 0.0 readouts.
    if amplitude < 0.001 {
        Peak::BelowFloor
    } else {
        Peak::Level {
            db,
            fraction: ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0) as f32,
            above: amplitude > 1.0,
        }
    }
}

impl Peak {
    fn text(self) -> String {
        match self {
            Self::Zero => "-inf".into(),
            Self::BelowFloor => "<-60".into(),
            Self::Invalid => "invalid".into(),
            Self::Level { db, above, .. } => {
                let value = if db > 99.9 {
                    ">99.9".into()
                } else if db.abs() < 0.05 {
                    if above { "+0.0".into() } else { "0.0".into() }
                } else if db > 0.0 {
                    format!("+{db:.1}")
                } else {
                    format!("{db:.1}")
                };
                format!("{value}{}", if above { "!" } else { "" })
            }
        }
    }

    pub(super) fn fraction(self) -> f32 {
        match self {
            Self::Level { fraction, .. } => fraction,
            _ => 0.0,
        }
    }

    pub(super) fn above(self) -> bool {
        matches!(self, Self::Level { above: true, .. })
    }
}

/// Bounded visible text; `!` indicates only a reported sample magnitude above 1.
pub fn readout(input: MeterInput) -> String {
    match input {
        MeterInput::Offline => "sample peak · offline".into(),
        MeterInput::Stopped => "sample peak · stopped".into(),
        MeterInput::Fault => "sample peak · fault".into(),
        MeterInput::Last { left, right } => {
            format!("last l {} r {} dbfs", peak(left).text(), peak(right).text())
        }
    }
}

/// Descriptive textual state for a consumer; this does not register OS AX roles.
pub fn description(input: MeterInput) -> String {
    match input {
        MeterInput::Last { .. } => format!(
            "{} · pre-conversion sample peaks · age unknown · channels independently reported · scale -60 to 0 dbfs · ! means sample above unity",
            readout(input)
        ),
        _ => readout(input),
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[path = "logic_tests.rs"]
mod tests;
