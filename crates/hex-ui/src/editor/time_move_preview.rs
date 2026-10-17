//! Passive time-chunk fragment rendering using cached waveform and note summaries.
use super::tracks::clip_grid_layer;
use super::viewport::Viewport;
use crate::theme;
use gpui::{Div, div, prelude::*, px, relative, rgb};
use hex_project::Clip;

pub(super) fn move_exclusions(
    display: Option<&super::time_selection::TimeRangeMoveDisplay<'_>>,
    track: hex_project::TrackId,
    track_index: usize,
) -> [Option<(u64, u64)>; 2] {
    let Some(display) = display else {
        return [None, None];
    };
    let source = (!display.copy_modifier && display.source.tracks().contains(&track))
        .then_some((display.source.start(), display.source.end()));
    let destination = (track_index >= display.destination_first_index
        && track_index <= display.destination_last_index)
        .then_some((display.destination_start, display.destination_end));
    [source, destination]
}

pub(super) fn clip_intersects_exclusions(clip: &Clip, exclusions: [Option<(u64, u64)>; 2]) -> bool {
    let Some(end) = clip.start().checked_add(clip.duration()) else {
        return false;
    };
    exclusions
        .into_iter()
        .flatten()
        .any(|(start, stop)| start < end && stop > clip.start())
}

pub(super) fn clip_fragments_outside(
    clip: &Clip,
    exclusions: [Option<(u64, u64)>; 2],
    tempo: &hex_project::TempoMap,
) -> [Option<Clip>; 3] {
    let Some(clip_end) = clip.start().checked_add(clip.duration()) else {
        return [None, None, None];
    };
    let intervals = time_fragments_outside(clip.start(), clip_end, exclusions);
    let mut result = [None, None, None];
    for (index, interval) in intervals.into_iter().enumerate() {
        if let Some((start, end)) = interval {
            result[index] =
                hex_project::preview_clip_fragment_with_tempo(clip, start, end, start, tempo).ok();
        }
    }
    result
}

pub(super) fn time_fragments_outside(
    start: u64,
    end: u64,
    exclusions: [Option<(u64, u64)>; 2],
) -> [Option<(u64, u64)>; 3] {
    let mut intervals = [None, None];
    let mut interval_count = 0;
    for (exclude_start, exclude_end) in exclusions.into_iter().flatten() {
        let exclude_start = exclude_start.max(start);
        let exclude_end = exclude_end.min(end);
        if exclude_start < exclude_end && interval_count < intervals.len() {
            intervals[interval_count] = Some((exclude_start, exclude_end));
            interval_count += 1;
        }
    }
    if interval_count == 2
        && intervals[0].is_some_and(|left| intervals[1].is_some_and(|right| right.0 < left.0))
    {
        intervals.swap(0, 1);
    }
    let mut result = [None, None, None];
    let mut count = 0;
    let mut cursor = start;
    for (from, to) in intervals.into_iter().flatten() {
        if to <= cursor {
            continue;
        }
        if from > cursor && count < result.len() {
            result[count] = Some((cursor, from));
            count += 1;
        }
        cursor = cursor.max(to);
    }
    if cursor < end && count < result.len() {
        result[count] = Some((cursor, end));
    }
    result
}

pub(super) fn clip_intersection(clip: &Clip, interval: (u64, u64)) -> Option<(u64, u64)> {
    let end = clip.start().checked_add(clip.duration())?;
    let start = clip.start().max(interval.0);
    let stop = end.min(interval.1);
    (start < stop).then_some((start, stop))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn passive_audio_fragment(
    fill: u32,
    clip: &Clip,
    waveform: Option<&std::sync::Arc<hex_session::CachedWaveform>>,
    viewport: Viewport,
    viewport_end: u64,
    grid: &[super::musical_time::GridLine],
    cache: std::rc::Rc<std::cell::RefCell<super::waveform_geometry::WaveformGeometryCache>>,
    selector: String,
) -> Option<Div> {
    let end = clip.start().checked_add(clip.duration())?;
    let start = clip.start().max(viewport.start);
    let stop = end.min(viewport_end);
    if start >= stop || viewport.length == 0 {
        return None;
    }
    let left = (start - viewport.start) as f64 / viewport.length as f64;
    let width = (stop - start) as f64 / viewport.length as f64;
    let mut content = div()
        .debug_selector(move || selector.clone())
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(left as f32))
        .w(relative(width as f32))
        .min_w(px(3.0))
        .overflow_hidden()
        .border_1()
        .border_color(rgb(0x000000))
        .bg(rgb(fill))
        .children([clip_grid_layer(grid, start, stop)]);
    if let Some(waveform) = waveform {
        content = content.child(super::waveform::waveform_view_cached(
            waveform, clip, start, stop, cache,
        ));
    } else {
        content = content.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .px_1()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text("missing media")),
        );
    }
    Some(content)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn passive_midi_fragment(
    fill: u32,
    clip_id: hex_project::ClipId,
    samples: &[hex_engine::MidiPreviewSample],
    source_interval: (u64, u64),
    timeline_interval: (u64, u64),
    viewport: Viewport,
    viewport_end: u64,
    grid: &[super::musical_time::GridLine],
    selector: String,
    shape_budget: usize,
) -> (Option<Div>, usize) {
    let visible_start = timeline_interval.0.max(viewport.start);
    let visible_end = timeline_interval.1.min(viewport_end);
    if visible_start >= visible_end || viewport.length == 0 {
        return (None, 0);
    }
    let left = (visible_start - viewport.start) as f64 / viewport.length as f64;
    let width = (visible_end - visible_start) as f64 / viewport.length as f64;
    let mut content = div()
        .debug_selector(move || format!("{selector}-{}", clip_id.value()))
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(left as f32))
        .w(relative(width as f32))
        .min_w(px(4.0))
        .overflow_hidden()
        .bg(rgb(fill))
        .border_1()
        .border_color(rgb(0x000000))
        .children([clip_grid_layer(grid, visible_start, visible_end)]);
    let mut shown = 0;
    for sample in samples
        .iter()
        .take(hex_engine::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP)
        .take(shape_budget)
    {
        let source_start = sample.frame_start.max(source_interval.0);
        let source_end = sample.frame_gate_end.min(source_interval.1);
        if source_start >= source_end {
            continue;
        }
        let Some(note_start) = timeline_interval
            .0
            .checked_add(source_start - source_interval.0)
        else {
            continue;
        };
        let Some(note_end) = timeline_interval
            .0
            .checked_add(source_end - source_interval.0)
        else {
            continue;
        };
        let Some((x, note_width, pitch)) = super::midi_thumbnail::note_shape(
            note_start,
            note_end,
            sample.pitch,
            visible_start,
            visible_end,
        ) else {
            continue;
        };
        let top = f32::from(127_u8.saturating_sub(pitch)) / 128.0;
        content = content.child(
            div()
                .absolute()
                .left(relative(x))
                .top(relative(top))
                .w(relative(note_width))
                .h(px(2.0))
                .bg(rgb(0x000000)),
        );
        shown += 1;
    }
    (Some(content), shown)
}

#[cfg(test)]
mod time_move_interval_tests {
    use super::time_fragments_outside;

    #[test]
    fn overlapping_source_and_destination_exclusions_leave_only_the_union_tail() {
        assert_eq!(
            time_fragments_outside(0, 96_000, [Some((0, 48_000)), Some((24_000, 72_000))]),
            [Some((72_000, 96_000)), None, None]
        );
    }

    #[test]
    fn destination_replacement_preserves_both_outside_tails() {
        assert_eq!(
            time_fragments_outside(0, 96_000, [None, Some((24_000, 72_000))]),
            [Some((0, 24_000)), Some((72_000, 96_000)), None]
        );
    }
}
