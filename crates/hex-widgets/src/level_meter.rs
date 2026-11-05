//! Normalized vertical gauge with a click-clear clipping cap. Owners supply
//! signal meaning, colors, dimensions and update cadence. The cap remembers observed clipping;
//! there is no audio detector or signal timing state.
use gpui::{Div, SharedString, div, prelude::*, relative, rgb};

/// Display caution begins at the dBFS equivalent of 0.95 f32 amplitude.
pub const METER_NEAR_PEAK_DBFS: f32 = -0.445_528;
/// Near-unity linear sample amplitude warning threshold.
pub const METER_NEAR_PEAK_AMPLITUDE: f32 = 0.95;

/// Pure display classification from actual amplitude/dBFS or explicit overload
/// evidence. Normalized display height never implies clipping. No state is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterLevel {
    Normal,
    NearPeak,
    Clipping,
}
impl MeterLevel {
    /// Height for a linear sample amplitude on the shared -60 to 0 dBFS
    /// display scale. Nonpositive and non-finite readings draw empty.
    pub fn fraction_from_amplitude(amplitude: f32) -> f32 {
        if !amplitude.is_finite() || amplitude <= 0.0 {
            return 0.0;
        }
        ((20.0 * amplitude.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
    }

    pub fn from_dbfs(dbfs: f32, clipping: bool) -> Self {
        if clipping || (dbfs.is_finite() && dbfs > 0.0) {
            Self::Clipping
        } else if dbfs.is_finite() && dbfs >= METER_NEAR_PEAK_DBFS {
            Self::NearPeak
        } else {
            Self::Normal
        }
    }
    pub fn from_amplitude(amplitude: f32, clipping: bool) -> Self {
        if clipping || (amplitude.is_finite() && amplitude > 1.0) {
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
/// `above` is caller evidence, independent of normalized height. Its top cap
/// latches until clicked, then rearms on a subsequently observed false→true edge.
pub fn level_meter(id: SharedString, fraction: f32, above: bool, style: LevelMeterStyle) -> Div {
    let cap = crate::meter_clip::ClipIndicator {
        id: format!("{id}-above").into(),
        clipping: above,
        held_clipping: false,
        color: style.above,
    };
    meter_body(id, fraction, style).child(cap)
}

/// Shared signal well; device gain meters install their cap after the gain handle.
pub(crate) fn meter_body(id: SharedString, fraction: f32, style: LevelMeterStyle) -> Div {
    meter_body_direction(id, fraction, style, false)
}

/// Read-only attenuation gauge, growing down from zero at the top. No clipping
/// cap is inferred from gain reduction; caller supplies a normalized dB extent.
pub fn reduction_meter(id: SharedString, fraction: f32, style: LevelMeterStyle) -> Div {
    meter_body_direction(id, fraction, style, true)
}

fn meter_body_direction(
    id: SharedString,
    fraction: f32,
    style: LevelMeterStyle,
    downward: bool,
) -> Div {
    let fill_id = format!("{id}-fill");
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
                .when(downward, |fill| fill.top_0())
                .when(!downward, |fill| fill.bottom_0())
                .w_full()
                .h(relative(bounded_fraction(fraction)))
                .bg(rgb(style.fill)),
        )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn amplitude_fraction_uses_sixty_db_floor_and_bounds_invalid_readings() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0, 0.001] {
            assert_eq!(MeterLevel::fraction_from_amplitude(value), 0.0);
        }
        for (amplitude, expected) in [(0.01, 1.0 / 3.0), (0.1, 2.0 / 3.0), (1.0, 1.0), (2.0, 1.0)] {
            assert!((MeterLevel::fraction_from_amplitude(amplitude) - expected).abs() < 1e-6);
        }
    }
    #[test]
    fn near_peak_units_and_explicit_overload_are_independent_of_display_height() {
        for (dbfs, expected) in [
            (-30.0, MeterLevel::Normal),
            (-6.0, MeterLevel::Normal),
            (-0.45, MeterLevel::Normal),
            (METER_NEAR_PEAK_DBFS, MeterLevel::NearPeak),
            (0.0, MeterLevel::NearPeak),
            (12.0, MeterLevel::Clipping),
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
            (f32::from_bits(1.0_f32.to_bits() + 1), MeterLevel::Clipping),
            (2.0, MeterLevel::Clipping),
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
