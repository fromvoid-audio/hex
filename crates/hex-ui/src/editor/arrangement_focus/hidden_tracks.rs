//! Decorative section-focus gap cues over the cached visibility owner. They
//! add no row space, focus, pointer behavior or independent visibility model.
use super::visibility::Projection;
use crate::theme;
use gpui::{Bounds, Div, Hsla, canvas, div, point, prelude::*, px, rgb, size};
use hex_project::TrackId;

const DASH_WIDTH: f32 = 4.0;
const DASH_GAP: f32 = 4.0;
const GAP_LINE_ALPHA: f32 = 0.55;

pub(in crate::editor) fn line_color() -> Hsla {
    Hsla::from(rgb(theme::color(theme::PaletteRole::Muted))).alpha(GAP_LINE_ALPHA)
}

fn line(selector: String, scale: f32, header_width: f32, bottom: bool) -> Div {
    let color = line_color();
    div()
        .debug_selector(move || selector.clone())
        .absolute()
        .left_0()
        .right(px((header_width + 1.0).max(0.0)))
        .h(px(1.0 / scale.max(1.0)))
        .when(bottom, |line| line.bottom_0())
        .when(!bottom, |line| line.top_0())
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let left = f32::from(bounds.left());
                    let right = f32::from(bounds.right());
                    if !left.is_finite() || !right.is_finite() || right <= left {
                        return;
                    }
                    let top = bounds.top();
                    let height = bounds.size.height;
                    let step = DASH_WIDTH + DASH_GAP;
                    let mut x = left;
                    while x < right {
                        let width = (right - x).min(DASH_WIDTH);
                        let rect = Bounds::new(point(px(x), top), size(px(width), height));
                        window.paint_quad(gpui::fill(rect, color));
                        x += step;
                    }
                },
            )
            .size_full(),
        )
}

pub(super) fn row(
    projection: &Projection,
    index: usize,
    track: TrackId,
    scale: f32,
    header_width: f32,
    row: Div,
) -> Div {
    let before = projection.hidden_before(index);
    let after = projection.hidden_after(index);
    if !before && !after {
        return row;
    }
    row.relative()
        .children(before.then(|| {
            line(
                format!("section-hidden-before-{}", track.value()),
                scale,
                header_width,
                false,
            )
        }))
        .children(after.then(|| {
            line(
                format!("section-hidden-after-{}", track.value()),
                scale,
                header_width,
                true,
            )
        }))
}

pub(super) fn empty(projection: &Projection, scale: f32, header_width: f32, content: Div) -> Div {
    content.when(projection.all_tracks_hidden(), |content| {
        let marker = line("section-hidden-all".into(), scale, header_width, false);
        content.relative().children([marker])
    })
}
