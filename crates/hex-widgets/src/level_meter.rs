//! Read-only normalized vertical gauge. Owners supply signal meaning, colors,
//! dimensions and update cadence; this widget has no audio/timing state.
use gpui::{Div, SharedString, div, prelude::*, px, relative, rgb};

/// Display caution begins at -6 dBFS, independently of overload evidence.
pub const METER_NEAR_PEAK_DBFS: f32 = -6.0;
/// Smallest f32 linear sample amplitude at or above -6 dBFS (20 log10(amplitude)).
pub const METER_NEAR_PEAK_AMPLITUDE: f32 = 0.501_187_26;

/// Pure display classification. Clipping is explicit caller-owned evidence,
/// never inferred from normalized height or a high dBFS value. No state is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterLevel {
    Normal,
    NearPeak,
    Clipping,
}
impl MeterLevel {
    pub fn from_dbfs(dbfs: f32, clipping: bool) -> Self {
        if clipping {
            Self::Clipping
        } else if dbfs.is_finite() && dbfs >= METER_NEAR_PEAK_DBFS {
            Self::NearPeak
        } else {
            Self::Normal
        }
    }
    pub fn from_amplitude(amplitude: f32, clipping: bool) -> Self {
        if clipping {
            Self::Clipping
        } else if amplitude.is_finite() && amplitude >= METER_NEAR_PEAK_AMPLITUDE {
            Self::NearPeak
        } else {
            Self::Normal
        }
    }
}

/// Theme/caller-owned colors shared by calibrated and uncalibrated meter owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeterColors {
    pub signal: u32,
    pub near_peak: u32,
    pub clipping: u32,
}
impl MeterColors {
    pub fn color(self, level: MeterLevel) -> u32 {
        match level {
            MeterLevel::Normal => self.signal,
            MeterLevel::NearPeak => self.near_peak,
            MeterLevel::Clipping => self.clipping,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelMeterStyle {
    pub background: u32,
    pub fill: u32,
    pub above: u32,
}
fn bounded_fraction(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
/// Empty for non-finite fractions, bounded for finite out-of-range inputs.
/// The separate above marker describes the owner's threshold, not a detector.
pub fn level_meter(id: SharedString, fraction: f32, above: bool, style: LevelMeterStyle) -> Div {
    let fill_id = format!("{id}-fill");
    let above_id = format!("{id}-above");
    div()
        .debug_selector(move || id.to_string())
        .relative()
        .size_full()
        .flex_shrink_0()
        .overflow_hidden()
        .bg(rgb(style.background))
        .child(
            div()
                .debug_selector(move || fill_id.clone())
                .absolute()
                .bottom_0()
                .w_full()
                .h(relative(bounded_fraction(fraction)))
                .bg(rgb(style.fill)),
        )
        .when(above, |bar| {
            bar.child(
                div()
                    .debug_selector(move || above_id.clone())
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(2.0))
                    .bg(rgb(style.above)),
            )
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn near_peak_units_and_explicit_overload_are_independent_of_display_height() {
        for (dbfs, expected) in [
            (-30.0, MeterLevel::Normal),
            (-6.000_001, MeterLevel::Normal),
            (-6.0, MeterLevel::NearPeak),
            (0.0, MeterLevel::NearPeak),
            (12.0, MeterLevel::NearPeak),
            (f32::NAN, MeterLevel::Normal),
            (f32::INFINITY, MeterLevel::Normal),
            (f32::NEG_INFINITY, MeterLevel::Normal),
        ] {
            assert_eq!(MeterLevel::from_dbfs(dbfs, false), expected);
            assert_eq!(MeterLevel::from_dbfs(dbfs, true), MeterLevel::Clipping);
        }
        for (amplitude, expected) in [
            (0.0, MeterLevel::Normal),
            (-1.0, MeterLevel::Normal),
            (
                f32::from_bits(METER_NEAR_PEAK_AMPLITUDE.to_bits() - 1),
                MeterLevel::Normal,
            ),
            (METER_NEAR_PEAK_AMPLITUDE, MeterLevel::NearPeak),
            (1.0, MeterLevel::NearPeak),
            (2.0, MeterLevel::NearPeak),
            (f32::NAN, MeterLevel::Normal),
            (f32::INFINITY, MeterLevel::Normal),
        ] {
            assert_eq!(MeterLevel::from_amplitude(amplitude, false), expected);
            assert_eq!(
                MeterLevel::from_amplitude(amplitude, true),
                MeterLevel::Clipping
            );
        }
    }
    #[test]
    fn invalid_and_out_of_range_levels_have_bounded_geometry() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0] {
            assert_eq!(super::bounded_fraction(value), 0.0);
        }
        assert_eq!(super::bounded_fraction(0.35), 0.35);
        assert_eq!(super::bounded_fraction(2.0), 1.0);
    }
}
