use crate::theme::{self, Palette};
use gpui::{Context, Div, Window, canvas, div, prelude::*, px, rgb};
use hex_project::{Clip, ClipFade};
use hex_session::{CachedPeak, CachedWaveform};
use std::{ops::Range, rc::Rc, sync::Arc};

use super::waveform_geometry::{
    WaveformGeometryCache, WaveformGeometryRequest, WaveformMapping, cached_waveform_geometry,
    prepare_waveform_geometry,
};

const MAX_WAVEFORM_COLUMNS: usize = 4096;
const FADE_ENVELOPE_SAMPLES: usize = 32;
const FADE_ENVELOPE_BREAKPOINTS: usize = 4;

pub(super) fn waveform_view_at(
    waveform: &Arc<CachedWaveform>,
    clip: &Clip,
    timeline_origin: u64,
    visible_start: u64,
    visible_end: u64,
) -> Div {
    waveform_canvas(
        Arc::clone(waveform),
        WaveformMapping::Clip {
            clip: clip.clone(),
            timeline_origin,
        },
        visible_start,
        visible_end,
        None,
    )
}

pub(super) fn waveform_view_cached(
    waveform: &Arc<CachedWaveform>,
    clip: &Clip,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
) -> Div {
    waveform_canvas(
        Arc::clone(waveform),
        WaveformMapping::Clip {
            clip: clip.clone(),
            timeline_origin: clip.start(),
        },
        visible_start,
        visible_end,
        Some(cache),
    )
}

pub(super) fn waveform_source_segment_view(
    waveform: Arc<CachedWaveform>,
    timeline_origin: u64,
    visible_start: u64,
    visible_end: u64,
) -> Div {
    waveform_canvas(
        waveform,
        WaveformMapping::Source { timeline_origin },
        visible_start,
        visible_end,
        None,
    )
}

pub(super) fn waveform_source_segment_view_cached(
    waveform: Arc<CachedWaveform>,
    timeline_origin: u64,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
) -> Div {
    waveform_canvas(
        waveform,
        WaveformMapping::Source { timeline_origin },
        visible_start,
        visible_end,
        Some(cache),
    )
}

pub(super) fn waveform_summary_view(waveform: Arc<CachedWaveform>) -> Div {
    let end = u64::try_from(waveform.source_frames).unwrap_or(u64::MAX);
    waveform_source_segment_view(waveform, 0, 0, end)
}

fn waveform_canvas(
    waveform: Arc<CachedWaveform>,
    mapping: WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    cache: Option<Rc<std::cell::RefCell<WaveformGeometryCache>>>,
) -> Div {
    let mapping = mapping.clone();
    waveform_surface(canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            if visible_start >= visible_end || width <= 0.0 || height <= 0.0 {
                return;
            }
            let columns = waveform_column_count(width, window.scale_factor());
            let geometry = cache.as_ref().and_then(|cache| {
                let request = WaveformGeometryRequest {
                    mapping: &mapping,
                    visible_start,
                    visible_end,
                    bounds,
                    scale: window.scale_factor(),
                    columns,
                };
                cached_waveform_geometry(cache, &waveform, &request)
            });
            let geometry = geometry.or_else(|| {
                prepare_waveform_geometry(
                    &waveform,
                    &mapping,
                    visible_start,
                    visible_end,
                    bounds,
                    columns,
                )
                .map(Arc::new)
            });
            if let Some(geometry) = geometry {
                for channel in &geometry.channels {
                    window.paint_path(channel.centerline.clone(), rgb(Palette::BORDER));
                    window.paint_path(channel.waveform.clone(), rgb(0x000000));
                }
            }
        },
    ))
}

fn waveform_surface<T: 'static>(canvas: gpui::Canvas<T>) -> Div {
    div()
        .absolute()
        .inset_0()
        .children([canvas.absolute().inset_0()])
}

pub(super) fn waveform_column_count(width: f32, scale: f32) -> usize {
    if !width.is_finite() || width <= 0.0 || !scale.is_finite() || scale <= 0.0 {
        return 1;
    }
    ((width * scale).ceil() as usize).clamp(1, MAX_WAVEFORM_COLUMNS)
}

pub(super) fn peak_vertical_bounds(peak: CachedPeak, center: f32, amplitude: f32) -> (f32, f32) {
    let top = center - peak.max.clamp(-1.0, 1.0) * amplitude;
    let bottom = center - peak.min.clamp(-1.0, 1.0) * amplitude;
    (top.min(bottom), top.max(bottom))
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

pub(super) fn fraction_u64(value: u64, numerator: usize, denominator: usize) -> u64 {
    if denominator == 0 {
        return 0;
    }
    ((u128::from(value) * numerator as u128) / denominator as u128) as u64
}

/// A display column can touch at most two source intervals at a loop wrap.
/// Keep these intervals on the stack rather than allocating a vector per column.
#[derive(Default)]
pub(super) struct SourceRanges {
    ranges: [Range<usize>; 2],
    len: usize,
}
impl SourceRanges {
    pub(super) fn as_slice(&self) -> &[Range<usize>] {
        &self.ranges[..self.len]
    }
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Map a visible timeline interval to its active source region, splitting at a loop wrap.
#[cfg(test)]
fn timeline_source_ranges(
    clip: &Clip,
    visible: Range<u64>,
    available_frames: usize,
) -> SourceRanges {
    timeline_source_ranges_at(clip, clip.start(), visible, available_frames)
}

pub(super) fn timeline_source_ranges_at(
    clip: &Clip,
    timeline_origin: u64,
    visible: Range<u64>,
    available_frames: usize,
) -> SourceRanges {
    let Some(clip_end) = timeline_origin.checked_add(clip.duration()) else {
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
    let start = visible.start.max(timeline_origin);
    let end = visible.end.min(clip_end);
    if start >= end {
        return SourceRanges::default();
    }
    let relative_start = start - timeline_origin;
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

pub(super) fn aggregate_peaks(
    waveform: &CachedWaveform,
    start: usize,
    end: usize,
) -> Option<[CachedPeak; 2]> {
    waveform.peaks_for_frames(start, end)
}

pub(super) fn aggregate_ranges(
    waveform: &CachedWaveform,
    ranges: &[Range<usize>],
) -> Option<[CachedPeak; 2]> {
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

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn waveform_canvas_paints_full_clip_extent(cx: &mut gpui::TestAppContext) {
        use gpui::{IntoElement, point, size};
        use std::{cell::Cell, rc::Rc};

        let visual = cx.add_empty_window();
        for width in [8.0, 260.0] {
            let painted = Rc::new(Cell::new(None));
            let result = Rc::clone(&painted);
            visual.draw(
                point(px(0.0), px(0.0)),
                size(px(width), px(44.0)),
                |_, _| {
                    div()
                        .w(px(width))
                        .h(px(44.0))
                        .relative()
                        .children([waveform_surface(canvas(
                            |_, _, _| (),
                            move |bounds, _, _, _| {
                                result.set(Some(bounds));
                            },
                        ))])
                        .into_element()
                },
            );
            let bounds = painted.get().expect("waveform paint callback ran");
            assert_eq!(bounds.size, size(px(width), px(44.0)));
        }
    }

    #[test]
    fn waveform_density_tracks_device_pixels_and_stays_bounded() {
        assert_eq!(waveform_column_count(120.0, 1.0), 120);
        assert_eq!(waveform_column_count(120.0, 2.0), 240);
        assert_eq!(waveform_column_count(100_000.0, 2.0), MAX_WAVEFORM_COLUMNS);
        assert_eq!(waveform_column_count(f32::NAN, 2.0), 1);
    }

    #[test]
    fn waveform_peak_geometry_preserves_signed_channel_extrema() {
        let positive = peak_vertical_bounds(
            CachedPeak {
                min: 0.25,
                max: 0.75,
            },
            10.0,
            8.0,
        );
        assert!(positive.0 < positive.1 && positive.1 < 10.0);
        let negative = peak_vertical_bounds(
            CachedPeak {
                min: -0.75,
                max: -0.25,
            },
            10.0,
            8.0,
        );
        assert!(negative.0 > 10.0 && negative.1 > negative.0);
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
    fn cached_queries_preserve_signed_extrema_across_uneven_bins() {
        let waveform = CachedWaveform::from_peaks(
            11,
            vec![
                [peak(0.0), peak(0.0)],
                [peak(0.2), peak(0.2)],
                [peak(0.8), peak(0.8)],
            ],
        )
        .expect("valid cache");
        assert_eq!(aggregate_peaks(&waveform, 0, 4).unwrap()[0].max, 0.0);
        assert_eq!(aggregate_peaks(&waveform, 4, 7).unwrap()[0].max, 0.2);
        assert_eq!(aggregate_peaks(&waveform, 7, 11).unwrap()[0].max, 0.8);
        assert_eq!(waveform.cached_bin_count(), 6);
    }

    #[test]
    fn display_group_merges_all_bins_to_keep_transient_extrema() {
        let waveform = CachedWaveform::from_peaks(
            12,
            vec![
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
        )
        .expect("valid cache");
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

        let waveform = CachedWaveform::from_peaks(
            32,
            (0..32)
                .map(|frame| [peak(frame as f32 / 32.0), peak(frame as f32 / 32.0)])
                .collect(),
        )
        .expect("valid cache");
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
        let waveform = CachedWaveform::from_peaks(
            6,
            vec![
                [peak(0.1), peak(0.1)],
                [peak(0.9), peak(0.2)],
                [peak(0.3), peak(0.3)],
            ],
        )
        .expect("valid cache");
        let ranges = timeline_source_ranges(&clip(0, 8, 1, 4, true), 2..4, 6);
        let peaks = aggregate_ranges(&waveform, ranges.as_slice()).expect("wrapped visible loop");
        assert_eq!(peaks[0].max, 0.9);
        assert_eq!(peaks[1].max, 0.3);
    }
}
