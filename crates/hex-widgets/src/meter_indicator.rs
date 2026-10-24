//! Read-only calibrated mono/stereo meter presentation. The caller supplies dBFS
//! levels, held peaks, overload state, text, palette, typography and update cadence.
//! No detector, peak history, audio state, scheduling or canonical editing lives here.
use crate::{ControlStyle, LevelMeterStyle, level_meter};
use gpui::{Div, SharedString, div, prelude::*, px, relative, rgb};

/// Fixed top-to-bottom positions, in dBFS. Scale text is supplied separately so
/// owners can choose signed dBFS or reference-style attenuation magnitudes.
pub const METER_INDICATOR_TICKS_DBFS: [f32; 9] =
    [0.0, -6.0, -12.0, -18.0, -24.0, -30.0, -36.0, -48.0, -60.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterChannelReadout {
    /// Current displayed level in dBFS; this is not linear amplitude.
    pub level_dbfs: f32,
    /// Caller-maintained held peak in dBFS. None omits the marker.
    pub held_peak_dbfs: Option<f32>,
    /// Explicit caller-owned overload indication, independent of display bounds.
    pub clipping: bool,
}

/// A fixed one- or two-channel value; no unbounded channel collection is accepted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeterIndicatorReadout {
    Mono(MeterChannelReadout),
    Stereo([MeterChannelReadout; 2]),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterIndicatorStyle {
    /// Reuses validated font size and injected rail/border/muted text colors.
    /// Font family is inherited from the owner, without a fallback override.
    pub controls: ControlStyle,
    pub signal: u32,
    pub peak: u32,
    pub overload: u32,
}
fn fraction(dbfs: f32) -> Option<f32> {
    dbfs.is_finite()
        .then(|| ((dbfs.clamp(-60.0, 0.0) + 60.0) / 60.0).clamp(0.0, 1.0))
}

/// Narrow vertical wells with a shared fixed -60..0 dBFS scale, bounded current
/// fills, held-peak lines and explicit overload caps. Finite values outside the
/// range clamp to its endpoints. Non-finite current values render empty; a
/// non-finite held peak omits that marker. Overload is never inferred or latched.
///
/// `tick_labels` correspond exactly to `METER_INDICATOR_TICKS_DBFS`; the owner
/// normalizes app-owned text. Width is intrinsic to the channel count and font
/// size. The caller supplies height; a compact minimum keeps scale text legible.
/// Invalid font sizes are rejected through the shared `ControlStyle` contract.
pub fn meter_indicator(
    id: SharedString,
    readout: MeterIndicatorReadout,
    tick_labels: [SharedString; 9],
    style: MeterIndicatorStyle,
) -> Result<Div, &'static str> {
    let controls = style.controls.validate()?;
    let channels = match &readout {
        MeterIndicatorReadout::Mono(channel) => std::slice::from_ref(channel),
        MeterIndicatorReadout::Stereo(channels) => channels.as_slice(),
    };
    let mut wells = div()
        .flex()
        .gap(px(1.0))
        .h_full()
        .py(px(controls.font_size * 0.5));
    for (index, channel) in channels.iter().enumerate() {
        let well_id = format!("{id}-channel-{index}");
        let peak_id = format!("{well_id}-peak");
        let peak = channel.held_peak_dbfs.and_then(fraction);
        wells = wells.child(
            level_meter(
                well_id.into(),
                fraction(channel.level_dbfs).unwrap_or(0.0),
                channel.clipping,
                LevelMeterStyle {
                    background: controls.rail,
                    fill: style.signal,
                    above: style.overload,
                },
            )
            .w(px(8.0))
            .h_full()
            .border_1()
            .border_color(rgb(controls.border))
            .when_some(peak, |well, peak| {
                // Interpolate over H - 1 pixels, keeping the whole one-pixel
                // marker inside the well even at the two scale endpoints.
                let top = 1.0 - peak;
                well.child(
                    div()
                        .debug_selector(move || peak_id.clone())
                        .absolute()
                        .top(relative(top))
                        .mt(px(-top))
                        .left_0()
                        .w_full()
                        .h(px(1.0))
                        .bg(rgb(style.peak)),
                )
            }),
        );
    }
    let mut scale = div().relative().w(px(controls.font_size * 2.4)).h_full();
    for (index, (dbfs, label)) in METER_INDICATOR_TICKS_DBFS
        .into_iter()
        .zip(tick_labels)
        .enumerate()
    {
        let top = 1.0 - fraction(dbfs).unwrap_or(0.0);
        let tick_id = format!("{id}-tick-{index}");
        scale = scale.child(
            div()
                .debug_selector(move || tick_id.clone())
                .absolute()
                .top(relative(top))
                .mt(px(-controls.font_size * top))
                .h(px(controls.font_size))
                .w_full()
                .flex()
                .items_center()
                .gap(px(2.0))
                .overflow_hidden()
                .child(
                    div()
                        .w(px(3.0))
                        .h(px(1.0))
                        .flex_shrink_0()
                        .bg(rgb(controls.muted)),
                )
                .child(label),
        );
    }
    let width =
        channels.len() as f32 * 8.0 + (channels.len() - 1) as f32 + 3.0 + controls.font_size * 2.4;
    Ok(div()
        .w(px(width))
        .debug_selector(move || id.to_string())
        .flex()
        .gap(px(3.0))
        .h_full()
        .min_h(px(controls.font_size * 12.0))
        .flex_shrink_0()
        .text_size(px(controls.font_size))
        .line_height(px(controls.font_size))
        .text_color(rgb(controls.muted))
        .child(wells)
        .child(scale))
}

#[cfg(test)]
#[path = "meter_indicator_tests.rs"]
mod tests;
