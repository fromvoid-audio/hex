//! Paint-only Arrangement detail for clips a few device pixels wide. The
//! ordinary clip elements retain the exact interval and own pointer input.
use gpui::{Bounds, Div, Window, canvas, div, fill, point, prelude::*, px, rgb, size};
use std::{collections::BTreeMap, ops::Range};

const MAX_BAR_WIDTH_DEVICE_PIXELS: f32 = 4.0;
const MAX_HIDDEN_BODY_WIDTH: f64 = 12.0;
const MAX_DENSE_COLUMNS: usize = 16_384;

pub(in crate::editor) fn is_narrow(width: f32, scale: f32) -> bool {
    width > 0.0 && width * scale <= MAX_BAR_WIDTH_DEVICE_PIXELS
}

/// Use the full clip width so moving an otherwise detailed clip through a
/// viewport edge does not change its content. A missing lane keeps detail.
pub(in crate::editor) fn shows_body_detail(
    lane_width: f32,
    clip_frames: u64,
    viewport_frames: u64,
) -> bool {
    lane_width <= 0.0
        || f64::from(lane_width) * clip_frames as f64
            > MAX_HIDDEN_BODY_WIDTH * viewport_frames as f64
}

#[derive(Clone, Copy)]
struct Ink {
    color: u32,
    selected: bool,
}

#[derive(Default)]
pub(super) struct Bars(Vec<(Range<f64>, Ink)>);

impl Bars {
    pub(super) fn push(&mut self, span: Range<f64>, color: u32, selected: bool) {
        self.0.push((span, Ink { color, selected }));
    }

    pub(super) fn view(self, track: hex_project::TrackId) -> Option<Div> {
        (!self.0.is_empty()).then(|| {
            div()
                .debug_selector(move || format!("narrow-clip-bars-{}", track.value()))
                .absolute()
                .inset_0()
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| self.paint(bounds, window),
                    )
                    .absolute()
                    .inset_0(),
                )
        })
    }

    pub(super) fn paint(&self, bounds: Bounds<gpui::Pixels>, window: &mut Window) {
        let visible = bounds.intersect(&window.content_mask().bounds);
        if visible.is_empty() || self.0.is_empty() {
            return;
        }
        let scale = f64::from(window.scale_factor());
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let left = f64::from(bounds.left()) * scale;
        let width = f64::from(bounds.size.width) * scale;
        let first = (f64::from(visible.left()) * scale).floor() as i32;
        let last = (f64::from(visible.right()) * scale).ceil() as i32;
        if last <= first {
            return;
        }
        let emit_run = |window: &mut Window, start: i32, end: i32, color: u32| {
            let bar = Bounds::new(
                point(px((f64::from(start) / scale) as f32), bounds.top()),
                size(
                    px((f64::from(end - start) / scale) as f32),
                    bounds.size.height,
                ),
            )
            .intersect(&visible);
            if !bar.is_empty() {
                window.paint_quad(fill(bar, rgb(color)));
            }
        };
        let paint_bars = |window: &mut Window| {
            let column_count = usize::try_from(last.saturating_sub(first)).unwrap_or(usize::MAX);
            if column_count <= MAX_DENSE_COLUMNS {
                let mut columns: Vec<Option<Ink>> = vec![None; column_count];
                for (span, ink) in &self.0 {
                    // One touched device pixel is enough to make a subpixel clip visible.
                    let start = (left + span.start * width).floor() as i32;
                    let end = (left + span.end * width).ceil() as i32;
                    let clamped_start = start.max(first);
                    let clamped_end = end.min(last);
                    if clamped_start >= clamped_end {
                        continue;
                    }
                    let slice_start = (clamped_start - first) as usize;
                    let slice_end = (clamped_end - first) as usize;
                    for slot in &mut columns[slice_start..slice_end] {
                        match slot {
                            Some(existing) if !ink.selected && existing.selected => {}
                            _ => *slot = Some(*ink),
                        }
                    }
                }
                let mut idx = 0;
                while idx < columns.len() {
                    let Some(ink) = columns[idx] else {
                        idx += 1;
                        continue;
                    };
                    let start_idx = idx;
                    idx += 1;
                    while idx < columns.len()
                        && columns[idx].is_some_and(|next| next.color == ink.color)
                    {
                        idx += 1;
                    }
                    emit_run(
                        window,
                        first + start_idx as i32,
                        first + idx as i32,
                        ink.color,
                    );
                }
            } else {
                let mut columns = BTreeMap::<i32, Ink>::new();
                for (span, ink) in &self.0 {
                    let start = (left + span.start * width).floor() as i32;
                    let end = (left + span.end * width).ceil() as i32;
                    for column in start.max(first)..end.min(last) {
                        let existing = columns.entry(column).or_insert(*ink);
                        if ink.selected || !existing.selected {
                            *existing = *ink;
                        }
                    }
                }
                let mut columns = columns.into_iter().peekable();
                while let Some((start, ink)) = columns.next() {
                    let mut end = start + 1;
                    while columns
                        .next_if(|(column, next)| *column == end && next.color == ink.color)
                        .is_some()
                    {
                        end += 1;
                    }
                    emit_run(window, start, end, ink.color);
                }
            }
        };
        let scaled_visible = visible.scale(window.scale_factor());
        if !scaled_visible.is_empty()
            && [
                scaled_visible.left(),
                scaled_visible.right(),
                scaled_visible.top(),
                scaled_visible.bottom(),
            ]
            .into_iter()
            .all(|edge| f64::from(edge).is_finite())
        {
            window.paint_layer(visible, paint_bars);
        } else {
            paint_bars(window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shows_body_detail;

    #[test]
    fn full_clip_body_detail_switches_after_twelve_logical_pixels() {
        let lane = 512.5;
        let viewport = 20_500;
        assert!(!shows_body_detail(lane, 479, viewport));
        assert!(!shows_body_detail(lane, 480, viewport));
        assert!(shows_body_detail(lane, 481, viewport));
        assert!(shows_body_detail(0.0, 479, viewport));
    }
}
