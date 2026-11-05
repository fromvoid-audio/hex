//! Pre-conversion output sample-peak presentation from the editor status snapshot.
//!
//! Inputs are the last reported independent channel values. This component has
//! no timestamp, coherent-pair guarantee, peak hold, decay or clipping detector.
//! Shared meter widgets retain only click-clear acknowledgement of observed
//! above-unity readouts; they cannot recover clipping between UI observations.
use crate::theme;
use gpui::{Div, Stateful, div, prelude::*, px, rgb};

pub const WIDTH: f32 = 248.0;
pub const HEIGHT: f32 = 22.0;
const BAR_HEIGHT: f32 = 16.0;
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
enum Peak {
    Zero,
    BelowFloor,
    Level { db: f64, fraction: f32, above: bool },
    Invalid,
}

fn peak(amplitude: f32) -> Peak {
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

    fn fraction(self) -> f32 {
        match self {
            Self::Level { fraction, .. } => fraction,
            _ => 0.0,
        }
    }

    fn above(self) -> bool {
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

fn field(input: MeterInput) -> Div {
    let surface = theme::color(theme::PaletteRole::Surface);
    let warning = matches!(input, MeterInput::Fault)
        || matches!(input, MeterInput::Last { left, right } if peak(left).above() || peak(right).above() || matches!(peak(left), Peak::Invalid) || matches!(peak(right), Peak::Invalid));
    let preferred = theme::color(if warning {
        theme::PaletteRole::Primary
    } else {
        theme::PaletteRole::Muted
    });
    div()
        .debug_selector(|| "output-meter-readout".into())
        .flex_1()
        .min_w_0()
        .truncate()
        .text_xs()
        .text_color(rgb(theme::readable_text(surface, preferred)))
        .child(theme::text(readout(input)))
}

fn control(channel: &'static str, value: Peak) -> Div {
    let background = theme::color(theme::PaletteRole::Raised);
    let colors = theme::meter_colors();
    let level = match value {
        Peak::Level { db, above, .. } => hex_widgets::MeterLevel::from_dbfs(db as f32, above),
        _ => hex_widgets::MeterLevel::Normal,
    };
    // Canonical availability/peak interpretation stays here; the shared gauge
    // only receives bounded display geometry and caller-resolved colors.
    hex_widgets::level_meter(
        format!("output-meter-{channel}").into(),
        value.fraction(),
        value.above(),
        hex_widgets::LevelMeterStyle {
            background,
            fill: colors.color(level),
            above: colors.clipping,
        },
    )
    .w(px(6.0))
    .h(px(BAR_HEIGHT))
}

/// Fixed status-slot component; no timestamp, decay or native AX contract.
pub fn render(input: MeterInput) -> Stateful<Div> {
    let (left, right) = match input {
        MeterInput::Last { left, right } => (peak(left), peak(right)),
        _ => (Peak::Invalid, Peak::Invalid),
    };
    div()
        .id("output-meter")
        .debug_selector(|| "output-meter".into())
        .w(px(WIDTH))
        .h(px(HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .child(control("left", left))
        .child(control("right", right))
        .child(field(input))
}

#[cfg(test)]
#[path = "output_meter/tests.rs"]
mod tests;
