//! Device level displays; all signal readings and tooltip text come from the host.
//! The inter-device stereo peak meter is display-only: it owns no pointer, focus
//! or keyboard behavior and never edits a value. Clicks fall through to the host.
use crate::ControlStyle;
use gpui::{
    App, Context, Div, Entity, IntoElement, Render, RenderOnce, SharedString, Window, div,
    prelude::*, px, relative, rgb,
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
/// Tooltip text stays live across repaints while hovered without recreating the
/// tooltip view.
/// Selectors: `{id}`, `{id}-left`/`{id}-right`, `{id}-tooltip`, plus `-fill` and
/// `-peak` suffixes.
pub fn stereo_peak_meter(
    id: SharedString,
    meter: StereoPeakMeter,
    style: ControlStyle,
) -> impl IntoElement {
    StereoPeakMeterElement { id, meter, style }
}

#[derive(IntoElement)]
struct StereoPeakMeterElement {
    id: SharedString,
    meter: StereoPeakMeter,
    style: ControlStyle,
}

impl RenderOnce for StereoPeakMeterElement {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tooltip_state = window.use_keyed_state(
            SharedString::from(format!("{}-tooltip-state", self.id)),
            cx,
            |_, _| MeterTooltipState {
                text: self.meter.tooltip.clone(),
                style: self.style,
            },
        );
        tooltip_state.update(cx, |state, _| {
            state.text = self.meter.tooltip.clone();
            state.style = self.style;
        });
        let selector = self.id.to_string();
        let tooltip_selector = format!("{}-tooltip", self.id);
        let mut bars = div()
            .debug_selector(move || selector)
            .flex()
            .gap(px(STEREO_PEAK_BAR_GAP))
            .h_full();
        for (side, (level, held)) in ["left", "right"]
            .into_iter()
            .zip(self.meter.levels.into_iter().zip(self.meter.held))
        {
            bars = bars.child(channel_bar(
                format!("{}-{side}", self.id).into(),
                level,
                held,
                self.meter.colors,
                self.style,
            ));
        }
        div()
            .id(self.id)
            .h_full()
            .flex_shrink_0()
            .child(bars)
            .tooltip(move |_, cx| {
                let state = tooltip_state.clone();
                let selector = tooltip_selector.clone();
                cx.new(|_| MeterTooltip { state, selector }).into()
            })
    }
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

struct MeterTooltipState {
    text: SharedString,
    style: ControlStyle,
}

struct MeterTooltip {
    state: Entity<MeterTooltipState>,
    selector: String,
}

impl Render for MeterTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let selector = self.selector.clone();
        div()
            .debug_selector(move || selector.clone())
            .px(px(6.0))
            .py(px(4.0))
            .bg(rgb(state.style.raised))
            .text_color(rgb(state.style.text))
            .text_size(px(10.0))
            .child(state.text.clone())
    }
}
