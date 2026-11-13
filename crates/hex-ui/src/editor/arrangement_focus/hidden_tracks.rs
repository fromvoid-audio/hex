//! Decorative section-focus gap cues over the cached visibility owner. They
//! add no row space, focus, pointer behavior or independent visibility model.
use super::visibility::Projection;
use crate::theme;
use gpui::{Div, div, prelude::*, px, rgb};
use hex_project::TrackId;

fn line(selector: String, scale: f32, bottom: bool) -> Div {
    div()
        .debug_selector(move || selector.clone())
        .absolute()
        .left_0()
        .right_0()
        .h(px(1.0 / scale.max(1.0)))
        .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
        .when(bottom, |line| line.bottom_0())
        .when(!bottom, |line| line.top_0())
}

pub(super) fn row(
    projection: &Projection,
    index: usize,
    track: TrackId,
    scale: f32,
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
                false,
            )
        }))
        .children(after.then(|| {
            line(
                format!("section-hidden-after-{}", track.value()),
                scale,
                true,
            )
        }))
}

pub(super) fn empty(projection: &Projection, scale: f32, content: Div) -> Div {
    content.when(projection.all_tracks_hidden(), |content| {
        let marker = line("section-hidden-all".into(), scale, false);
        content.relative().children([marker])
    })
}
