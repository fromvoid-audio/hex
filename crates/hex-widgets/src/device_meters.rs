//! Device level displays; all signal readings and tooltip text come from the host.
//! The inter-device stereo peak meter is display-only: it owns no pointer, focus
//! or keyboard behavior and never edits a value. Clicks fall through to the host.
use crate::ControlStyle;
use gpui::{
    Context, Div, IntoElement, Render, SharedString, Window, div, prelude::*, px, relative, rgb,
};

/// Input-level evidence drawn inside a meter fader's rail.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct DeviceMeterReadout {
    /// Linear sample amplitude.
    pub level: f32,
    pub signal: u32,
    pub clipping: u32,
}

/// Width of each channel bar in the inter-device stereo meter.
pub const STEREO_PEAK_BAR_WIDTH: f32 = 3.0;
/// Gap between the left and right channel bars.
pub const STEREO_PEAK_BAR_GAP: f32 = 1.0;

/// One left/right reading for [`stereo_peak_meter`]. Levels and held peaks are
/// linear sample amplitudes; colors are caller/theme owned.
#[derive(Clone, PartialEq)]
pub struct StereoPeakMeter {
    pub levels: [f32; 2],
    pub held: [f32; 2],
    pub tooltip: SharedString,
    pub colors: crate::MeterColors,
}

/// Two thin vertical bars, left then right, each with a held-peak line. The
/// scale spans -60 to 0 dBFS; amplitudes above unity draw in the clipping color.
/// Selectors: `{id}`, `{id}-left`/`{id}-right`, plus `-fill` and `-peak` suffixes.
pub fn stereo_peak_meter(
    id: SharedString,
    meter: StereoPeakMeter,
    style: ControlStyle,
) -> gpui::Stateful<Div> {
    let selector = id.to_string();
    let mut bars = div()
        .debug_selector(move || selector)
        .flex()
        .gap(px(STEREO_PEAK_BAR_GAP))
        .h_full();
    for (side, (level, held)) in ["left", "right"]
        .into_iter()
        .zip(meter.levels.into_iter().zip(meter.held))
    {
        bars = bars.child(channel_bar(
            format!("{id}-{side}").into(),
            level,
            held,
            meter.colors,
            style,
        ));
    }
    let tooltip = meter.tooltip;
    div()
        .id(id)
        .h_full()
        .flex_shrink_0()
        .child(bars)
        .tooltip(move |_, cx| cx.new(|_| MeterTooltip(tooltip.clone(), style)).into())
}

fn channel_bar(
    id: SharedString,
    level: f32,
    held: f32,
    colors: crate::MeterColors,
    style: ControlStyle,
) -> Div {
    let peak_selector = format!("{id}-peak");
    let held_above = held.is_finite() && held > 1.0;
    crate::level_meter::signal_meter(
        id,
        crate::MeterLevel::fraction_from_amplitude(level),
        crate::LevelMeterStyle {
            background: style.rail,
            fill: colors.color(crate::MeterLevel::from_amplitude(level, false)),
            above: colors.clipping,
        },
    )
    .w(px(STEREO_PEAK_BAR_WIDTH))
    .h_full()
    .when(held.is_finite() && held > 0.0, |bar| {
        let top = 1.0 - crate::MeterLevel::fraction_from_amplitude(held);
        bar.child(
            div()
                .debug_selector(move || peak_selector.clone())
                .absolute()
                .top(relative(top))
                .mt(px(-top))
                .w_full()
                .h(px(1.0))
                .bg(rgb(
                    match crate::MeterLevel::from_amplitude(held, held_above) {
                        crate::MeterLevel::Normal => style.text,
                        level => colors.color(level),
                    },
                )),
        )
    })
}

struct MeterTooltip(SharedString, ControlStyle);
impl Render for MeterTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(6.0))
            .py(px(4.0))
            .bg(rgb(self.1.raised))
            .text_color(rgb(self.1.text))
            .text_size(px(10.0))
            .child(self.0.clone())
    }
}
