//! Read-only normalized vertical gauge. Owners supply signal meaning, colors,
//! dimensions and update cadence; this widget has no audio/timing state.
use gpui::{Div, SharedString, div, prelude::*, px, relative, rgb};

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
    #[test]
    fn invalid_and_out_of_range_levels_have_bounded_geometry() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0] {
            assert_eq!(super::bounded_fraction(value), 0.0);
        }
        assert_eq!(super::bounded_fraction(0.35), 0.35);
        assert_eq!(super::bounded_fraction(2.0), 1.0);
    }
}
