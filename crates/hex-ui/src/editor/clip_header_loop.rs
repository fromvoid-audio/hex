//! Paint-only Arrangement header ramps. Project queries own the cycle clock
//! and phase, including retained reads. Search the visible range before
//! admitting paint, and leave the ordinary fill in place for dense repeats.
use super::{midi_source_controls::SourceValues, trim::MidiEdgePreview};
use crate::theme;
use gpui::{
    Background, Bounds, Pixels, Window, fill, linear_color_stop, linear_gradient, point, px, rgb,
};
use hex_project::{
    BeatTime, Clip, MidiClip, MidiSourcePlayback, RepeatCycles, TempoMap, audio_projection_frame,
};
use std::{ops::Range, rc::Rc};

const MIN_CYCLE_PIXELS: f32 = 4.0;
const MAX_SEGMENTS: u64 = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Segment {
    left: f32,
    right: f32,
    first: f32,
    last: f32,
}

#[derive(Clone)]
pub(super) struct HeaderGradient(Rc<[Segment]>);

pub(super) fn audio(
    clip: &Clip,
    tempo: &TempoMap,
    visible: Range<u64>,
    pixels: f32,
) -> Option<HeaderGradient> {
    if !super::trim::loop_handles::audio_repeats(clip) {
        return None;
    }
    let cycles = clip.repeat_cycles(tempo).ok()??;
    let start = clip.start();
    let end = start.checked_add(clip.duration())?;
    segments(cycles.cycles(), start..end, visible, pixels, |elapsed| {
        cycles.frame_at_elapsed(elapsed).ok()
    })
}

pub(super) fn midi(
    clip: &MidiClip,
    preview: Option<MidiEdgePreview>,
    tempo: &TempoMap,
    visible: Range<u64>,
    pixels: f32,
) -> Option<HeaderGradient> {
    if !preview.map_or(clip.looping(), |p| p.looping(clip)) {
        return None;
    }
    let playback = match preview {
        Some(preview) => preview.source_playback(clip),
        None => clip.source_playback(),
    }
    .ok()?;
    midi_playback(
        playback,
        preview.map_or(clip.start(), |p| p.start()),
        tempo,
        visible,
        pixels,
    )
}

pub(super) fn midi_source(
    clip: &MidiClip,
    values: SourceValues,
    tempo: &TempoMap,
    visible: Range<u64>,
    pixels: f32,
) -> Option<HeaderGradient> {
    if !values.looping {
        return None;
    }
    let beat = BeatTime::from_raw;
    let playback = MidiSourcePlayback::new(
        beat(values.source_length),
        beat(values.source_start),
        Some((beat(values.loop_start), beat(values.loop_end))),
        beat(values.arrangement_length),
    )
    .ok()?
    .with_playback_offset(clip.playback_offset())
    .ok()?;
    let playback = if let Some(window) = clip.content_view() {
        playback
            .with_content_window(window.resized(0, values.arrangement_length).ok()?)
            .ok()?
    } else {
        playback
    };
    midi_playback(
        playback,
        beat(values.arrangement_start),
        tempo,
        visible,
        pixels,
    )
}

fn midi_playback(
    playback: MidiSourcePlayback,
    origin: BeatTime,
    tempo: &TempoMap,
    visible: Range<u64>,
    pixels: f32,
) -> Option<HeaderGradient> {
    let cycles = playback.repeat_cycles().ok()??;
    let origin = i128::from(origin.raw());
    let frame = |elapsed: i128| audio_projection_frame(tempo, origin.checked_add(elapsed)?).ok();
    let start = u64::try_from(frame(0)?).ok()?;
    let end = u64::try_from(frame(i128::from(playback.arrangement_duration().raw()))?).ok()?;
    segments(cycles, start..end, visible, pixels, frame)
}

fn segments(
    cycles: RepeatCycles,
    outer: Range<u64>,
    visible: Range<u64>,
    pixels: f32,
    frame: impl Fn(i128) -> Option<i128>,
) -> Option<HeaderGradient> {
    let width = visible.end.checked_sub(visible.start).filter(|n| *n > 0)? as f64;
    if !pixels.is_finite() || pixels < MIN_CYCLE_PIXELS {
        return None;
    }
    let count = cycles.count();
    let edge = |index: u64| {
        if index == 0 {
            Some(outer.start)
        } else if index == count {
            Some(outer.end)
        } else {
            let elapsed = cycles.seams()?.boundary(index - 1)?;
            u64::try_from(frame(i128::from(elapsed))?).ok()
        }
    };
    // The first visible cycle is the first whose right edge exceeds the
    // viewport start. The last search skips cycles that start at the end.
    let search = |mut lo: u64, mut hi: u64, before: &dyn Fn(u64) -> Option<bool>| {
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if before(mid)? {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Some(lo)
    };
    let first = search(0, count, &|i| Some(edge(i + 1)? <= visible.start))?;
    let end = search(first, count, &|i| Some(edge(i)? < visible.end))?;
    let budget = ((pixels / MIN_CYCLE_PIXELS) as u64)
        .saturating_add(2)
        .min(MAX_SEGMENTS);
    if end - first > budget {
        return None;
    }
    let mut result = Vec::with_capacity((end - first) as usize);
    for index in first..end {
        let left = edge(index)?.max(visible.start);
        let right = edge(index + 1)?.min(visible.end);
        let (full_start, full_end) = cycles.full_bounds(index)?;
        let (full_start, full_end) = (frame(full_start)?, frame(full_end)?);
        let span = (full_end - full_start) as f64;
        if left >= right || span * f64::from(pixels) / width < f64::from(MIN_CYCLE_PIXELS) {
            continue;
        }
        let phase =
            |frame: u64| (((i128::from(frame) - full_start) as f64 / span).clamp(0.0, 1.0)) as f32;
        result.push(Segment {
            left: ((left - visible.start) as f64 / width) as f32,
            right: ((right - visible.start) as f64 / width) as f32,
            first: phase(left),
            last: phase(right),
        });
    }
    (!result.is_empty()).then(|| HeaderGradient(result.into()))
}

fn background(ramp: theme::ClipHeaderRamp, first: f32, last: f32) -> Background {
    linear_gradient(
        90.0,
        linear_color_stop(rgb(ramp.color(first)), 0.0),
        linear_color_stop(rgb(ramp.color(last)), 1.0),
    )
}

impl HeaderGradient {
    /// Match the backdrop's temporal width and inset inside its outline. A
    /// segment crossing the upper-right cut has a small clipped top polygon.
    pub(super) fn paint(
        &self,
        bounds: Bounds<Pixels>,
        header_height: f32,
        stroke: f32,
        cut: f32,
        color: u32,
        window: &mut Window,
    ) {
        let top = bounds.top() + px(1.0);
        let bottom = (bounds.top() + px(header_height)).min(bounds.bottom() - px(1.0));
        if top >= bottom {
            return;
        }
        let ramp = theme::ClipHeaderRamp::new(color);
        let cut_x = bounds.right() - px(cut);
        let diagonal =
            |x: Pixels| bounds.top() + (x - cut_x) + px(stroke * std::f32::consts::SQRT_2);
        for segment in self.0.iter() {
            let x = bounds.left() + bounds.size.width * segment.left;
            let right = bounds.left() + bounds.size.width * segment.right;
            let left = x.max(bounds.left() + px(stroke));
            if left >= right {
                continue;
            }
            let phase = |edge: Pixels| {
                let fraction = (edge - x) / (right - x);
                segment.first + fraction * (segment.last - segment.first)
            };
            let quad_top = if cut > 0.0 {
                top.max(diagonal(right)).min(bottom)
            } else {
                top
            };
            let color = background(ramp, phase(left), segment.last);
            if quad_top < bottom {
                window.paint_quad(fill(
                    Bounds::from_corners(point(left, quad_top), point(right, bottom)),
                    color,
                ));
            }
            if quad_top > top {
                let mut path = gpui::PathBuilder::fill();
                path.move_to(point(left, top.max(diagonal(left))));
                let kink = cut_x + (top - diagonal(cut_x));
                if left < kink && kink < right {
                    path.line_to(point(kink, top));
                }
                let path_right = right.min(cut_x + (quad_top - diagonal(cut_x)));
                if left < path_right {
                    path.line_to(point(path_right, top.max(diagonal(path_right))));
                    path.line_to(point(path_right, quad_top));
                    path.line_to(point(left, quad_top));
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, background(ramp, phase(left), phase(path_right)));
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn gradient_quads(
    visual: &mut gpui::VisualTestContext,
    header: &'static str,
) -> Vec<(gpui::Bounds<gpui::ScaledPixels>, gpui::Background)> {
    let mut quads: Vec<_> = visual
        .rendered_filled_quads(header)
        .into_iter()
        .filter(|(_, fill)| format!("{fill:?}").starts_with("LinearGradient("))
        .collect();
    // Live and acknowledged backdrops can occupy different scene layers.
    quads.sort_by(|(a, _), (b, _)| {
        a.left()
            .partial_cmp(&b.left())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.top()
                    .partial_cmp(&b.top())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    quads
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::ContentWindow;

    #[test]
    fn visible_segments_keep_partial_phase_and_bound_dense_cycles() {
        let cycles = ContentWindow::new(3, 24, false)
            .unwrap()
            .repeat_cycles(10, 10)
            .unwrap();
        let paint = |visible, pixels| {
            segments(cycles, 100..121, visible, pixels, |elapsed| {
                Some(100 + elapsed)
            })
        };
        let whole = paint(100..121, 210.0).unwrap();
        assert_eq!(whole.0.len(), 3);
        assert_eq!((whole.0[0].first, whole.0[0].last), (0.3, 1.0));
        assert_eq!((whole.0[2].first, whole.0[2].last), (0.0, 0.4));
        let zoomed = paint(105..119, 210.0).unwrap();
        assert_eq!(zoomed.0.len(), 3);
        assert_eq!(zoomed.0[0].first, 0.8);
        assert_eq!(zoomed.0[0].right, 2.0 / 14.0);
        assert_eq!(zoomed.0[2].last, 0.2);

        let dense = ContentWindow::new(0, 10_000_000, false)
            .unwrap()
            .repeat_cycles(1, 1)
            .unwrap();
        assert!(segments(dense, 0..10_000_000, 0..10_000_000, 500.0, Some).is_none());
        let close = segments(dense, 0..10_000_000, 50_000..50_004, 100.0, Some).unwrap();
        assert_eq!(close.0.len(), 4);
    }
}
