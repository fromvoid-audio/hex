//! Compact device meter gain controls; all labels and tooltip text come from the host.
//! The clipped 8px signal well sits inside a 10px interactive surface. The gain
//! handle occupies the extra 2px without expanding the host's compact meter strip.
//! Its 2px height remains inside the well at both gain endpoints; read-only level
//! meters retain their own clipped presentation and have no gain handle.
use crate::ControlStyle;
use gpui::{
    Context, Div, IntoElement, Render, SharedString, Window, div, prelude::*, px, relative, rgb,
};

#[derive(Clone, Default, PartialEq)]
pub struct DeviceMeterReadout {
    pub height: f32,
    pub index: usize,
    pub gain_db: f32,
    pub levels: [f32; 2],
    pub held: [f32; 2],
    pub tooltips: [SharedString; 2],
    pub signal: u32,
    pub warning: u32,
    pub clipping: u32,
}

pub(crate) fn well(
    id: SharedString,
    index: usize,
    readout: &DeviceMeterReadout,
    style: ControlStyle,
) -> gpui::Stateful<Div> {
    let level = readout.levels[index];
    let held = readout.held[index];
    let height = fraction(level);
    let peak_height = fraction(held);
    let tooltip = readout.tooltips[index].clone();
    let selector = format!("{id}-{}-meter", if index == 0 { "input" } else { "output" });
    let gain = if readout.gain_db.is_finite() {
        ((readout.gain_db + 60.0) / 84.0).clamp(0.0, 1.0)
    } else {
        60.0 / 84.0
    };
    let handle_selector = format!("{selector}-gain-handle");
    let well = crate::level_meter(
        selector.clone().into(),
        height,
        false,
        crate::LevelMeterStyle {
            background: style.rail,
            fill: if level >= 1.0 {
                readout.clipping
            } else if level >= 0.5 {
                readout.warning
            } else {
                readout.signal
            },
            above: readout.clipping,
        },
    )
    .w(px(8.0))
    .h_full()
    .when(held > 0.0, |well| {
        well.child(
            div()
                .absolute()
                .bottom(relative(peak_height))
                .w_full()
                .h(px(1.0))
                .bg(rgb(if held >= 1.0 {
                    readout.clipping
                } else {
                    style.text
                })),
        )
    })
    .child(
        div()
            .absolute()
            .bottom(relative(60.0 / 84.0))
            .w_full()
            .h(px(1.0))
            .bg(rgb(style.text))
            .opacity(0.35),
    );
    div()
        .id(SharedString::from(selector))
        .relative()
        .w(px(10.0))
        .h_full()
        .flex_shrink_0()
        .child(well)
        .child(
            // Travel excludes the handle's height, so neither endpoint clips.
            div()
                .absolute()
                .top_0()
                .bottom(px(2.0))
                .left_0()
                .w_full()
                .child(
                    div()
                        .debug_selector(move || handle_selector.clone())
                        .absolute()
                        .top(relative(1.0 - gain))
                        .w_full()
                        .h(px(2.0))
                        .bg(rgb(readout.warning)),
                ),
        )
        .tooltip(move |_, cx| cx.new(|_| MeterTooltip(tooltip.clone(), style)).into())
}
fn fraction(amplitude: f32) -> f32 {
    if !amplitude.is_finite() || amplitude <= 0.0 {
        return 0.0;
    }
    ((20.0 * amplitude.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
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
