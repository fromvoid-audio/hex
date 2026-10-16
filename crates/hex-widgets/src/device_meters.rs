//! Compact device meter gain controls; all labels and tooltip text come from the host.
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
    let fill_selector = format!("{selector}-fill");
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .relative()
        .w(px(8.0))
        .flex_shrink_0()
        .h_full()
        .bg(rgb(style.rail))
        .child(
            div()
                .debug_selector(move || fill_selector.clone())
                .absolute()
                .bottom(px(0.0))
                .w_full()
                .h(relative(height))
                .bg(rgb(if level >= 1.0 {
                    readout.clipping
                } else if level >= 0.5 {
                    readout.warning
                } else {
                    readout.signal
                })),
        )
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
        )
        .child(
            div()
                .absolute()
                .bottom(relative(((readout.gain_db + 60.0) / 84.0).clamp(0.0, 1.0)))
                .w_full()
                .h(px(2.0))
                .bg(rgb(readout.warning)),
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
