//! Shared compact timeline presentation. Owners provide normalized positions,
//! authored labels and semantic states; this module owns no time conversion,
//! input, project commands or pending-edit custody.
use crate::theme;
use gpui::{Div, div, prelude::*, px, relative, rgb};

pub(super) const RULER_HEIGHT: f32 = 24.0;
pub(super) const LOOP_HEIGHT: f32 = 18.0;

pub(super) fn ruler_surface() -> Div {
    div()
        .relative()
        .h(px(RULER_HEIGHT))
        .flex_shrink_0()
        .overflow_hidden()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
}

pub(super) fn ruler_tick(
    position: f32,
    label: impl AsRef<str>,
    bar_start: bool,
    meter_marker: bool,
    fragmentary_bar: bool,
) -> Option<Div> {
    if !position.is_finite() || !(0.0..=1.0).contains(&position) {
        return None;
    }
    Some(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(position))
            .flex()
            .items_center()
            .border_l_1()
            .border_color(rgb(theme::ruler_tick_color(
                bar_start,
                meter_marker || fragmentary_bar,
            )))
            .pl_1()
            .when(position == 1.0, |tick| {
                tick.left_auto()
                    .right_0()
                    .border_l_0()
                    .border_r_1()
                    .pl_0()
                    .pr_1()
            })
            .pb(px(hex_widgets::TIME_POSITION_MARKER_HEIGHT))
            .text_size(px(10.0))
            .text_color(rgb(theme::color(if meter_marker {
                theme::PaletteRole::Text
            } else {
                theme::PaletteRole::Muted
            })))
            .child(theme::text(label)),
    )
}

pub(super) fn loop_surface() -> Div {
    div()
        .relative()
        .h(px(LOOP_HEIGHT))
        .flex_shrink_0()
        .overflow_hidden()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
}

/// Fractions may extend beyond the viewport. Clip the connecting rail, but
/// paint an endpoint only where its authored boundary is actually visible.
pub(super) fn loop_range(
    start: f64,
    end: f64,
    enabled: bool,
    endpoints: [&'static str; 2],
) -> Option<Div> {
    if !start.is_finite() || !end.is_finite() || start >= end || end < 0.0 || start > 1.0 {
        return None;
    }
    let first = start.clamp(0.0, 1.0);
    let last = end.clamp(0.0, 1.0);
    let color = theme::color(if enabled {
        theme::PaletteRole::Text
    } else {
        theme::PaletteRole::Muted
    });
    let mut rail = div()
        .absolute()
        .top(px(10.0))
        .h(px(3.0))
        .left(relative(first as f32))
        .w(relative((last - first) as f32))
        .bg(rgb(color))
        .opacity(if enabled { 0.94 } else { 0.62 });
    for (is_start, position) in [(true, start), (false, end)] {
        if (0.0..=1.0).contains(&position) {
            // A zero-width intersection can still have a visible endpoint.
            // Keep its cap inside the viewport, including exactly zero/one.
            let align_left = if is_start {
                position < 1.0
            } else {
                position == 0.0
            };
            rail = rail.child(
                div()
                    .debug_selector(move || endpoints[usize::from(!is_start)].into())
                    .absolute()
                    .top(px(-6.0))
                    .h(px(9.0))
                    .w(px(3.0))
                    .when(align_left, |d| d.left_0())
                    .when(!align_left, |d| d.right_0())
                    .bg(rgb(color)),
            );
        }
    }
    Some(rail)
}
