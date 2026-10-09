use crate::theme::{self, Palette};
use gpui::{Context, Div, Window, div, prelude::*, px, rgb};
use hex_project::{Clip, ClipFade};
use hex_session::{CachedPeak, CachedWaveform};
use std::{ops::Range, sync::Arc};

const WAVEFORM_BINS_PER_CLIP: usize = 48;
const FADE_ENVELOPE_SAMPLES: usize = 32;
const FADE_ENVELOPE_BREAKPOINTS: usize = 4;

pub(super) fn waveform_view(
    waveform: &Arc<CachedWaveform>,
    clip: &Clip,
    visible_start: u64,
    visible_end: u64,
) -> Div {
    let visible_frames = visible_end.saturating_sub(visible_start);
    let bar_count = usize::try_from(visible_frames.min(WAVEFORM_BINS_PER_CLIP as u64))
        .unwrap_or(WAVEFORM_BINS_PER_CLIP)
        .max(1);
    let mut bars = div()
        .absolute()
        .top(px(2.0))
        .bottom(px(2.0))
        .left(px(4.0))
        .right(px(2.0))
        .flex()
        .items_center()
        .gap(px(1.0));
    for bar in 0..bar_count {
        let timeline_start =
            visible_start.saturating_add(fraction_u64(visible_frames, bar, bar_count));
        let timeline_end =
            visible_start.saturating_add(fraction_u64(visible_frames, bar + 1, bar_count));
        let ranges =
            timeline_source_ranges(clip, timeline_start..timeline_end, waveform.source_frames);
        let amplitude = aggregate_ranges(waveform, ranges.as_slice())
            .map(|peaks| {
                peaks
                    .iter()
                    .map(|peak| peak.min.abs().max(peak.max.abs()))
                    .fold(0.0_f32, f32::max)
                    .clamp(0.0, 1.0)
            })
            .unwrap_or(0.0);
        let height = px((amplitude * 28.0).max(1.0));
        bars = bars.child(
            div()
                .flex_1()
                .h(px(30.0))
                .flex()
                .items_center()
                .child(div().w_full().h(height).bg(rgb(Palette::WAVEFORM))),
        );
    }
    div().absolute().inset_0().child(bars)
}

/// Return a viewport-clipped, bounded preview of the canonical fade gain law.
/// X and gain are normalized to [0, 1]; sample phases stay in clip timeline
/// frames, independent of looping source-media phase.
pub(super) fn fade_envelope_samples(
    fade: ClipFade,
    clip_start: u64,
    visible_start: u64,
    visible_end: u64,
) -> Vec<(f32, f32)> {
    if (fade.fade_in() == 0 && fade.fade_out() == 0)
        || visible_start < clip_start
        || visible_start >= visible_end
    {
        return Vec::new();
    }
    let visible_frames = visible_end - visible_start;
    let sample_count = usize::try_from(visible_frames.min(FADE_ENVELOPE_SAMPLES as u64))
        .unwrap_or(FADE_ENVELOPE_SAMPLES)
        .max(1);
    let Some(local_start) = visible_start.checked_sub(clip_start) else {
        return Vec::new();
    };
    let local_span = visible_frames.saturating_sub(1);
    let Some(local_end) = local_start.checked_add(local_span) else {
        return Vec::new();
    };
    let mut frames = Vec::with_capacity(sample_count + FADE_ENVELOPE_BREAKPOINTS);
    for index in 0..sample_count {
        frames.push(local_start.saturating_add(fraction_u64(
            local_span,
            index,
            sample_count.saturating_sub(1),
        )));
    }
    let mut add_domain_position = |position: u64| {
        if let Some(frame) = position.checked_sub(fade.offset())
            && frame >= local_start
            && frame <= local_end
        {
            frames.push(frame);
        }
    };
    if fade.fade_in() > 1 {
        add_domain_position(fade.fade_in() - 1);
    }
    if fade.fade_out() > 0 {
        let fade_out_start = fade.span() - fade.fade_out();
        if fade_out_start > 0 {
            add_domain_position(fade_out_start - 1);
        }
        add_domain_position(fade_out_start);
    }
    add_domain_position(fade.span() - 1);
    frames.sort_unstable();
    frames.dedup();
    let mut points = Vec::with_capacity(frames.len());
    for frame in frames {
        let x = if local_span == 0 {
            0.0
        } else {
            (frame - local_start) as f32 / local_span as f32
        };
        points.push((x, fade.gain_at(frame).clamp(0.0, 1.0)));
    }
    if points.iter().all(|(_, gain)| *gain >= 1.0) {
        return Vec::new();
    }
    points
}

fn fraction_u64(value: u64, numerator: usize, denominator: usize) -> u64 {
    if denominator == 0 {
        return 0;
    }
    ((u128::from(value) * numerator as u128) / denominator as u128) as u64
}

/// A display column can touch at most two source intervals at a loop wrap.
/// Keep these intervals on the stack rather than allocating a vector per column.
#[derive(Default)]
struct SourceRanges {
    ranges: [Range<usize>; 2],
    len: usize,
}
impl SourceRanges {
    fn as_slice(&self) -> &[Range<usize>] {
        &self.ranges[..self.len]
    }
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Map a visible timeline interval to its active source region, splitting at a loop wrap.
fn timeline_source_ranges(
    clip: &Clip,
    visible: Range<u64>,
    available_frames: usize,
) -> SourceRanges {
    let Some(clip_end) = clip.start().checked_add(clip.duration()) else {
        return SourceRanges::default();
    };
    let Some(source_end) = clip.source_offset().checked_add(clip.source_length()) else {
        return SourceRanges::default();
    };
    if clip.source_length() == 0
        || source_end > available_frames as u64
        || visible.start >= visible.end
    {
        return SourceRanges::default();
    }
    let start = visible.start.max(clip.start());
    let end = visible.end.min(clip_end);
    if start >= end {
        return SourceRanges::default();
    }
    let relative_start = start - clip.start();
    let span = end - start;
    let mut ranges = SourceRanges::default();
    if !clip.looping() {
        let active_end = (relative_start.saturating_add(span)).min(clip.source_length());
        let active_start = relative_start.min(clip.source_length());
        if active_start < active_end {
            push_source_range(
                &mut ranges,
                clip.source_offset() + active_start,
                clip.source_offset() + active_end,
                available_frames,
            );
        }
        return ranges;
    }

    if span >= clip.source_length() {
        push_source_range(
            &mut ranges,
            clip.source_offset(),
            source_end,
            available_frames,
        );
        return ranges;
    }
    let phase_start = ((u128::from(clip.source_phase() % clip.source_length())
        + u128::from(relative_start % clip.source_length()))
        % u128::from(clip.source_length())) as u64;
    let phase_end = u128::from(phase_start) + u128::from(span);
    if phase_end <= u128::from(clip.source_length()) {
        push_source_range(
            &mut ranges,
            clip.source_offset() + phase_start,
            clip.source_offset() + phase_end as u64,
            available_frames,
        );
    } else {
        push_source_range(
            &mut ranges,
            clip.source_offset() + phase_start,
            source_end,
            available_frames,
        );
        push_source_range(
            &mut ranges,
            clip.source_offset(),
            clip.source_offset() + (phase_end - u128::from(clip.source_length())) as u64,
            available_frames,
        );
    }
    ranges
}

fn push_source_range(ranges: &mut SourceRanges, start: u64, end: u64, available_frames: usize) {
    let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
        return;
    };
    if start < end && end <= available_frames {
        if let Some(slot) = ranges.ranges.get_mut(ranges.len) {
            *slot = start..end;
            ranges.len += 1;
        }
    }
}

/// Find summary bins intersecting a half-open source-frame interval.
fn summary_bin_range(
    waveform: &CachedWaveform,
    start: usize,
    end: usize,
) -> std::ops::Range<usize> {
    let start = start.min(waveform.source_frames);
    let end = end.min(waveform.source_frames).max(start);
    if start == end || waveform.bins.is_empty() || waveform.source_frames == 0 {
        return 0..0;
    }
    let count = waveform.bins.len();
    let width = waveform.source_frames / count;
    let extra = waveform.source_frames % count;
    let wide_frames = (width + 1).saturating_mul(extra);
    let bin_at = |frame: usize| {
        if frame < wide_frames {
            frame / (width + 1)
        } else {
            extra + (frame - wide_frames) / width
        }
    };
    bin_at(start).min(count)..(bin_at(end - 1) + 1).min(count)
}

/// Merge every summary bin touched by one display column, preserving extrema.
fn aggregate_peaks(waveform: &CachedWaveform, start: usize, end: usize) -> Option<[CachedPeak; 2]> {
    let range = summary_bin_range(waveform, start, end);
    let first = waveform.bins.get(range.start)?;
    let mut merged = *first;
    for bin in waveform.bins.get(range.start + 1..range.end)? {
        for channel in 0..2 {
            merged[channel].min = merged[channel].min.min(bin[channel].min);
            merged[channel].max = merged[channel].max.max(bin[channel].max);
        }
    }
    Some(merged)
}

fn aggregate_ranges(waveform: &CachedWaveform, ranges: &[Range<usize>]) -> Option<[CachedPeak; 2]> {
    let mut merged = None;
    for range in ranges {
        let peaks = aggregate_peaks(waveform, range.start, range.end)?;
        if let Some(current) = merged.as_mut() {
            let current: &mut [CachedPeak; 2] = current;
            for channel in 0..2 {
                current[channel].min = current[channel].min.min(peaks[channel].min);
                current[channel].max = current[channel].max.max(peaks[channel].max);
            }
        } else {
            merged = Some(peaks);
        }
    }
    merged
}

pub(super) fn button(
    label: &'static str,
    tooltip: &'static str,
    id: (&'static str, u64),
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .w(px(19.0))
        .h(px(21.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_xs()
        .bg(rgb(Palette::RAISED))
        .text_color(rgb(Palette::MUTED))
        .tooltip(move |_, cx| {
            cx.new(|_| ClipTooltip {
                name: tooltip.to_string(),
            })
            .into()
        })
        .child(theme::text(label))
}

pub(super) struct ClipTooltip {
    pub(super) name: String,
}

impl Render for ClipTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .border_1()
            .border_color(rgb(Palette::BORDER))
            .bg(rgb(Palette::RAISED))
            .font_family(theme::FONT_FAMILY)
            .text_xs()
            .text_color(rgb(Palette::TEXT))
            .child(theme::text(&self.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    fn peak(value: f32) -> CachedPeak {
        CachedPeak {
            min: value,
            max: value,
        }
    }

    fn clip(start: u64, duration: u64, offset: u64, length: u64, looping: bool) -> Clip {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "track".into(),
                path: "audio.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .expect("media import")
        else {
            panic!("unexpected import result");
        };
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start,
                duration,
                source_offset: offset,
                source_length: length,
                looping,
            })
            .expect("place clip")
        else {
            panic!("unexpected clip result");
        };
        editor.project().clip(id).expect("clip").clone()
    }

    fn trimmed_loop_after_left_trim() -> Clip {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "track".into(),
                path: "loop.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .expect("media import")
        else {
            panic!("unexpected import result");
        };
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 100,
                duration: 20,
                source_offset: 10,
                source_length: 4,
                looping: true,
            })
            .expect("place loop")
        else {
            panic!("unexpected clip result");
        };
        editor
            .execute(Command::TrimClipEdge {
                clip: id,
                edge: hex_project::TrimEdge::Left,
                frame: 101,
            })
            .expect("trim loop");
        editor.project().clip(id).expect("trimmed loop").clone()
    }

    #[test]
    fn source_ranges_map_to_uneven_summary_bins() {
        let waveform = CachedWaveform {
            source_frames: 11,
            bins: vec![
                [peak(0.0), peak(0.0)],
                [peak(0.2), peak(0.2)],
                [peak(0.8), peak(0.8)],
            ],
        };
        assert_eq!(summary_bin_range(&waveform, 0, 4), 0..1);
        assert_eq!(summary_bin_range(&waveform, 4, 8), 1..2);
        assert_eq!(summary_bin_range(&waveform, 8, 11), 2..3);
        assert_eq!(summary_bin_range(&waveform, 3, 9), 0..3);
    }

    #[test]
    fn display_group_merges_all_bins_to_keep_transient_extrema() {
        let waveform = CachedWaveform {
            source_frames: 12,
            bins: vec![
                [peak(0.0), peak(0.0)],
                [
                    CachedPeak {
                        min: -1.0,
                        max: 0.9,
                    },
                    peak(0.7),
                ],
                [peak(0.0), peak(0.0)],
            ],
        };
        let peaks = aggregate_peaks(&waveform, 0, 12).expect("nonempty group");
        assert_eq!(peaks[0].min, -1.0);
        assert_eq!(peaks[0].max, 0.9);
        assert_eq!(peaks[1].max, 0.7);
    }

    #[test]
    fn cropped_one_shot_maps_only_its_visible_active_source_frames() {
        assert_eq!(
            timeline_source_ranges(&clip(100, 20, 10, 8, false), 105..118, 32).as_slice(),
            std::slice::from_ref(&(15..18))
        );
        assert!(timeline_source_ranges(&clip(100, 20, 10, 8, false), 108..116, 32).is_empty());
    }

    #[test]
    fn cropped_loop_maps_phase_and_splits_at_wrap() {
        assert_eq!(
            timeline_source_ranges(&clip(100, 20, 10, 4, true), 102..105, 32).as_slice(),
            &[12..14, 10..11]
        );
        assert_eq!(
            timeline_source_ranges(&clip(100, 20, 10, 4, true), 101..106, 32).as_slice(),
            std::slice::from_ref(&(10..14))
        );
    }

    #[test]
    fn cropped_loop_waveform_uses_trimmed_source_phase_for_transients() {
        let clip = trimmed_loop_after_left_trim();
        assert_eq!(clip.start(), 101);
        assert_eq!(clip.source_phase(), 1);
        let ranges = timeline_source_ranges(&clip, 103..106, 32);
        assert_eq!(ranges.as_slice(), &[13..14, 10..12]);

        let waveform = CachedWaveform {
            source_frames: 32,
            bins: (0..32)
                .map(|frame| [peak(frame as f32 / 32.0), peak(frame as f32 / 32.0)])
                .collect(),
        };
        let peaks = aggregate_ranges(&waveform, ranges.as_slice()).expect("visible loop peaks");
        assert_eq!(peaks[0].max, 13.0 / 32.0);
        assert_eq!(peaks[0].min, 10.0 / 32.0);
    }

    #[test]
    fn fade_envelope_samples_are_clipped_and_use_the_canonical_inherited_phase() {
        let fade = hex_project::ClipFade::new(1_000, 400, 600, 500).expect("inherited fade");
        let samples = fade_envelope_samples(fade, 200, 240, 260);
        assert_eq!(samples.len(), 20);
        assert!(samples.len() <= FADE_ENVELOPE_SAMPLES + FADE_ENVELOPE_BREAKPOINTS);
        assert_eq!(samples.first().unwrap().0, 0.0);
        assert_eq!(samples.last().unwrap().0, 1.0);
        assert_eq!(samples.first().unwrap().1, fade.gain_at(40));
        assert_eq!(samples.last().unwrap().1, fade.gain_at(59));
        assert!(fade_envelope_samples(fade, 200, 190, 220).is_empty());
        assert!(fade_envelope_samples(fade, 200, 240, 240).is_empty());
        let neutral = ClipFade::neutral(100).expect("neutral fade");
        assert!(fade_envelope_samples(neutral, 200, 200, 220).is_empty());
        let outside_fade = ClipFade::new(100, 40, 10, 10).expect("off-segment fade");
        assert!(fade_envelope_samples(outside_fade, 200, 220, 240).is_empty());
    }

    #[test]
    fn short_fade_breakpoint_stays_local_on_a_long_visible_clip() {
        let fade = ClipFade::new(1_000_000, 0, 241, 0).expect("short fade");
        let samples = fade_envelope_samples(fade, 0, 0, 1_000_000);
        assert!(samples.len() <= FADE_ENVELOPE_SAMPLES + FADE_ENVELOPE_BREAKPOINTS);
        assert_eq!(samples.first().unwrap().1, 0.0);
        let end = samples
            .iter()
            .find(|(x, _)| (*x - 240.0 / 999_999.0).abs() < f32::EPSILON)
            .expect("exact fade-in end breakpoint");
        assert_eq!(end.1, 1.0);
    }

    #[test]
    fn inherited_fade_breakpoints_use_domain_positions_before_offset_translation() {
        let fade = ClipFade::new(1_000, 200, 250, 400).expect("inherited fade");
        let samples = fade_envelope_samples(fade, 1_000, 1_000, 1_800);
        let at_local_frame = |frame: u64| {
            samples
                .iter()
                .find(|(x, _)| (*x - frame as f32 / 799.0).abs() < f32::EPSILON)
                .copied()
                .expect("exact inherited breakpoint")
        };
        assert_eq!(at_local_frame(49).1, 1.0); // fade-in endpoint
        assert_eq!(at_local_frame(399).1, 1.0); // before fade-out
        assert_eq!(at_local_frame(400).1, 1.0); // fade-out start
        assert_eq!(at_local_frame(799).1, 0.0); // original span endpoint

        let fade_in_crop = fade_envelope_samples(fade, 1_000, 1_040, 1_060);
        assert!(
            fade_in_crop
                .iter()
                .any(|(x, gain)| { (*x - 9.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0 })
        );
        let fade_out_crop = fade_envelope_samples(fade, 1_000, 1_390, 1_410);
        assert!(
            fade_out_crop
                .iter()
                .any(|(x, gain)| (*x - 9.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0)
        );
        assert!(
            fade_out_crop
                .iter()
                .any(|(x, gain)| (*x - 10.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0)
        );
    }

    #[test]
    fn loop_waveform_preserves_peaks_across_the_phase_wrap() {
        let waveform = CachedWaveform {
            source_frames: 6,
            bins: vec![
                [peak(0.1), peak(0.1)],
                [peak(0.9), peak(0.2)],
                [peak(0.3), peak(0.3)],
            ],
        };
        let ranges = timeline_source_ranges(&clip(0, 8, 1, 4, true), 2..4, 6);
        let peaks = aggregate_ranges(&waveform, ranges.as_slice()).expect("wrapped visible loop");
        assert_eq!(peaks[0].max, 0.9);
        assert_eq!(peaks[1].max, 0.3);
    }
}
