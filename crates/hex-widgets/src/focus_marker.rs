//! Small, noninteractive corner marks. Absolute children paint inside the tile
//! and do not change its layout or its knob/value hit targets.
use gpui::{Div, div, prelude::*, px, rgb};

pub(crate) fn corner_markers(id: &str, color: u32) -> Div {
    let mut marks = div().absolute().inset_0().size_full();
    for (name, right, bottom) in [
        ("top-left", false, false),
        ("top-right", true, false),
        ("bottom-left", false, true),
        ("bottom-right", true, true),
    ] {
        for (axis, width, height) in [("horizontal", 3.0, 1.0), ("vertical", 1.0, 3.0)] {
            let selector = format!("{id}-focus-{name}-{axis}");
            let mark = div()
                .absolute()
                .when(right, |mark| mark.right(px(0.0)))
                .when(!right, |mark| mark.left(px(0.0)))
                .when(bottom, |mark| mark.bottom(px(0.0)))
                .when(!bottom, |mark| mark.top(px(0.0)))
                .w(px(width))
                .h(px(height))
                .bg(rgb(color))
                .debug_selector(move || selector.clone());
            marks = marks.child(mark);
        }
    }
    marks
}
