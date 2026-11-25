//! Pre-conversion output sample-peak presentation from the editor status snapshot.
//!
//! `render_unified` shares peak-hold, smooth decay and callback-latched between-poll
//! clip presentation with `editor::track_meters`. Standalone `render(MeterInput)`
//! remains available for stateless single-snapshot inspection.
use crate::theme;
use gpui::{Div, SharedString, Stateful, div, prelude::*, px, relative, rgb};
use std::time::Duration;

pub const WIDTH: f32 = 248.0;
pub const HEIGHT: f32 = 22.0;
const BAR_HEIGHT: f32 = 16.0;
/// Shared peak-hold retention window before the peak marker decays at -12 dB/s.
pub const PEAK_HOLD_DURATION: Duration = Duration::from_millis(1_500);

/// Unified stereo meter display reading shared by Main, Track, Return, and status
/// bar output meters: smooth decaying level, held peak marker, and callback-latched
/// between-poll clip flags (`peak > 1.0`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterReading {
    pub level: [f32; 2],
    pub peak_hold: [f32; 2],
    pub clipped: [bool; 2],
}

impl MeterReading {
    pub fn from_peaks(peaks: [f32; 2]) -> Self {
        Self {
            level: peaks,
            peak_hold: peaks,
            clipped: peaks.map(|p| p.is_finite() && p > 1.0),
        }
    }
}

/// Shared vertical level bar with click-to-clear clip cap and optional 1px peak-hold
/// hairline (`{id}-peak-hold`). Used by both `output_meter` and `track_meters`.
pub fn render_meter_bar(
    id: SharedString,
    fraction: f32,
    hold_fraction: f32,
    above: bool,
    style: hex_widgets::LevelMeterStyle,
    hold_color: u32,
) -> Div {
    let hold_selector = format!("{id}-peak-hold");
    let show_hold = hold_fraction.is_finite() && hold_fraction > 0.0;
    let clamped_hold = if hold_fraction.is_finite() {
        hold_fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    hex_widgets::level_meter(id, fraction, above, style).when(show_hold, |bar| {
        bar.child(
            div()
                .debug_selector(move || hold_selector.clone())
                .absolute()
                .top(relative(1.0 - clamped_hold))
                .w_full()
                .h(px(1.0))
                .bg(rgb(hold_color)),
        )
    })
}

use hex_ui_model::output_meter_logic as logic;
pub use logic::{MeterInput, description, readout};
use logic::{Peak, peak};

fn field(input: MeterInput, latched_clip: bool) -> Div {
    let surface = theme::color(theme::PaletteRole::Surface);
    let warning = latched_clip
        || matches!(input, MeterInput::Fault)
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

fn control(channel: &'static str, value: Peak, hold: Peak, clipped: bool) -> Div {
    let background = theme::color(theme::PaletteRole::Raised);
    let colors = theme::meter_colors();
    let above = clipped || value.above();
    let level = match value {
        Peak::Level {
            db,
            above: val_above,
            ..
        } => hex_widgets::MeterLevel::from_dbfs(db as f32, above || val_above),
        _ if above => hex_widgets::MeterLevel::Clipping,
        _ => hex_widgets::MeterLevel::Normal,
    };
    let hold_level = match hold {
        Peak::Level {
            db,
            above: hold_above,
            ..
        } => hex_widgets::MeterLevel::from_dbfs(db as f32, clipped || hold_above),
        _ => hex_widgets::MeterLevel::Normal,
    };
    // Canonical availability/peak interpretation stays here; the shared gauge
    // only receives bounded display geometry and caller-resolved colors.
    render_meter_bar(
        format!("output-meter-{channel}").into(),
        value.fraction(),
        hold.fraction(),
        above,
        hex_widgets::LevelMeterStyle {
            background,
            fill: colors.color(level),
            above: colors.clipping,
        },
        colors.color(hold_level),
    )
    .w(px(6.0))
    .h(px(BAR_HEIGHT))
}

/// Fixed status-slot component for stateless single-snapshot inspection.
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
        .child(control("left", left, Peak::Zero, false))
        .child(control("right", right, Peak::Zero, false))
        .child(field(input, false))
}

/// Unified status-slot output meter sharing smooth decay, peak-hold hairlines,
/// and callback-latched between-poll clip telemetry with `editor::track_meters`.
pub fn render_unified(input: MeterInput, reading: MeterReading) -> Stateful<Div> {
    let active = !matches!(input, MeterInput::Offline | MeterInput::Fault);
    let (left, right, hold_left, hold_right, clip_left, clip_right) = if active {
        (
            peak(reading.level[0]),
            peak(reading.level[1]),
            peak(reading.peak_hold[0]),
            peak(reading.peak_hold[1]),
            reading.clipped[0]
                || matches!(input, MeterInput::Last { left, .. } if peak(left).above()),
            reading.clipped[1]
                || matches!(input, MeterInput::Last { right, .. } if peak(right).above()),
        )
    } else {
        (
            Peak::Invalid,
            Peak::Invalid,
            Peak::Zero,
            Peak::Zero,
            false,
            false,
        )
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
        .child(control("left", left, hold_left, clip_left))
        .child(control("right", right, hold_right, clip_right))
        .child(field(input, clip_left || clip_right))
}

#[cfg(test)]
#[path = "output_meter/tests.rs"]
mod tests;
