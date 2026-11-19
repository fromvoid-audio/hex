//! Display-only geometry for control-prepared Arrangement note samples.
//! Source projection and tempo conversion never run here.

/// A one-pitch clip draws its notes one third of the body tall, centred.
const MIN_FIT_ROWS: u8 = 3;

/// Vertical zoom of an Arrangement MIDI thumbnail onto the pitches its clip
/// actually uses, like Live: a wide range draws thin notes, a single pitch
/// draws thick ones. Derive it from the clip's whole prepared samples, not the
/// visible crop, so the zoom stays put while scrolling and during gestures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PitchFit {
    low: u8,
    rows: u8,
}

impl PitchFit {
    pub(super) fn from_pitches(pitches: impl IntoIterator<Item = u8>) -> Self {
        let mut used: Option<(u8, u8)> = None;
        for pitch in pitches.into_iter().filter(|pitch| *pitch <= 127) {
            used = Some(used.map_or((pitch, pitch), |(low, high)| {
                (low.min(pitch), high.max(pitch))
            }));
        }
        let Some((low, high)) = used else {
            return Self { low: 0, rows: 128 };
        };
        let span = high - low + 1;
        let rows = span.max(MIN_FIT_ROWS);
        // Centre a short range, keeping every row inside 0..=127.
        let low = low.saturating_sub((rows - span) / 2).min(128 - rows);
        Self { low, rows }
    }

    /// Top and height of `pitch`'s row as fractions of the note body.
    fn row(self, pitch: u8) -> Option<(f32, f32)> {
        let index = pitch
            .checked_sub(self.low)
            .filter(|index| *index < self.rows)?;
        let rows = f32::from(self.rows);
        Some((f32::from(self.rows - 1 - index) / rows, 1.0 / rows))
    }
}

/// Note rectangle top and height in body pixels, with a 1px gap between rows
/// once rows are tall enough and a 1px minimum for dense ranges.
pub(super) fn note_rows(fit: PitchFit, pitch: u8, body_height: f32) -> Option<(f32, f32)> {
    let (top, height) = fit.row(pitch)?;
    let row = height * body_height;
    let gap = if row >= 4.0 { 1.0 } else { 0.0 };
    Some((top * body_height, (row - gap).max(1.0)))
}

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
    let left = f32::from(bounds.left());
    let top = f32::from(bounds.top());
    let body_width = f32::from(bounds.size.width);
    let body_height = f32::from(bounds.size.height);
    if ![left, top, body_width, body_height, x, width, scale]
        .iter()
        .all(|v| v.is_finite())
        || body_width <= 0.0
        || body_height <= 0.0
        || scale <= 0.0
        || !(0.0..1.0).contains(&x)
        || width <= 0.0
        || x + width > 1.0
        || pitch > 127
    {
        return None;
    }
    let (row_top, row_height) = note_rows(fit, pitch, body_height)?;
    let snap = |value: f32| (value * scale).round() / scale;
    let start = snap(left + x * body_width);
    let end = snap(left + (x + width) * body_width);
    let y = snap(top + row_top);
    let bottom = snap(top + row_top + row_height).max(y + 1.0 / scale);
    if ![start, end, y, bottom].iter().all(|v| v.is_finite()) || end <= start {
        return None;
    }
    Some(gpui::Bounds::new(
        gpui::point(gpui::px(start), gpui::px(y)),
        gpui::size(gpui::px(end - start), gpui::px(bottom - y)),
    ))
}

pub(super) fn note_shape(
    frame_start: u64,
    frame_gate_end: u64,
    pitch: u8,
    visible_start: u64,
    visible_end: u64,
) -> Option<(f32, f32, u8)> {
    if visible_end <= visible_start || frame_gate_end <= frame_start || pitch > 127 {
        return None;
    }
    let start = frame_start.max(visible_start);
    let end = frame_gate_end.min(visible_end);
    if end <= start {
        return None;
    }
    let span = (visible_end - visible_start) as f64;
    // Preserve a positive display sliver when a huge frame range rounds x to 1.
    let x =
        (((start - visible_start) as f64 / span) as f32).min(f32::from_bits(1.0_f32.to_bits() - 1));
    let width = (((end - start) as f64 / span) as f32)
        .max(0.005)
        .min(1.0 - x);
    Some((x, width, pitch))
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
