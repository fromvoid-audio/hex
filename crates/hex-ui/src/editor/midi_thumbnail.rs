//! GPUI bounds adapter over [`hex_ui_model::midi_thumbnail`]; pure geometry and unit tests live in `hex-ui-model`.
pub(super) use hex_ui_model::midi_thumbnail::{PitchFit, note_rows, note_shape};

/// Snap display samples to physical pixels just as the former layout leaves
/// did. The enclosing body still clips notes below the clip name and at edges.
pub(super) fn paint_bounds(
    bounds: gpui::Bounds<gpui::Pixels>,
    x: f32,
    width: f32,
    pitch: u8,
    fit: PitchFit,
    scale: f32,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    let rect = hex_ui_model::midi_thumbnail::snapped_note_rect(
        [
            f32::from(bounds.left()),
            f32::from(bounds.top()),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        ],
        x,
        width,
        pitch,
        fit,
        scale,
    )?;
    Some(gpui::Bounds::new(
        gpui::point(gpui::px(rect[0]), gpui::px(rect[1])),
        gpui::size(gpui::px(rect[2]), gpui::px(rect[3])),
    ))
}

/// Inspect completed canvas rectangles, including replay and ancestor clipping.
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn rendered_notes(
    visual: &mut gpui::VisualTestContext,
    clip: hex_project::ClipId,
    dragging: bool,
) -> Vec<(gpui::Bounds<gpui::Pixels>, gpui::Background)> {
    rendered_note_layer(
        visual,
        &format!(
            "clip-{}notes-{}",
            if dragging { "drag-" } else { "" },
            clip.value()
        ),
    )
}

/// Completed note rectangles of the note layer named `selector`, in logical
/// pixels.
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn rendered_note_layer(
    visual: &mut gpui::VisualTestContext,
    selector: &str,
) -> Vec<(gpui::Bounds<gpui::Pixels>, gpui::Background)> {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    let scale = visual.update(|window, _| window.scale_factor());
    visual
        .rendered_filled_quads(selector)
        .into_iter()
        .filter(|(_, color)| *color == gpui::Background::from(gpui::rgb(0x000000)))
        .map(|(bounds, color)| {
            (
                gpui::Bounds::new(
                    gpui::point(
                        gpui::px(f64::from(bounds.origin.x) as f32 / scale),
                        gpui::px(f64::from(bounds.origin.y) as f32 / scale),
                    ),
                    gpui::size(
                        gpui::px(f64::from(bounds.size.width) as f32 / scale),
                        gpui::px(f64::from(bounds.size.height) as f32 / scale),
                    ),
                ),
                color,
            )
        })
        .collect()
}
