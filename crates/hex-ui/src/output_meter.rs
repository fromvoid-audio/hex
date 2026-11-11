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
#[path = "output_meter/logic.rs"]
mod logic;
pub use logic::{MeterInput, description, readout};
use logic::{Peak, peak};

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
