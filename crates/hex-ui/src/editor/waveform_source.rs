//! Map waveform display intervals to source frames and cached peak ranges.
use hex_project::Clip;
use hex_session::{CachedPeak, CachedWaveform};
use std::ops::Range;

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
    #[cfg(all(test, feature = "ui-unit-tier"))]
    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Map a visible timeline interval to its active source region, splitting at a loop wrap.
#[cfg(all(test, feature = "ui-unit-tier"))]
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
    // Transformed audio has a prepared root projection. An original asset peak
    // map cannot describe it, including not-yet-prepared drag/trim previews.
    if clip.audio_edit().is_some() {
        return SourceRanges::default();
    }
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

pub(super) fn push_source_range(
    ranges: &mut SourceRanges,
    start: u64,
    end: u64,
    available_frames: usize,
) {
    let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
        return;
    };
    if start < end
        && end <= available_frames
        && let Some(slot) = ranges.ranges.get_mut(ranges.len)
    {
        *slot = start..end;
        ranges.len += 1;
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
    aggregate_ranges_with_detail(waveform, ranges, 0)
}

/// Refine narrow resident ranges without exceeding the supplied per-column PCM budget.
pub(super) fn aggregate_ranges_with_detail(
    waveform: &CachedWaveform,
    ranges: &[Range<usize>],
    budget: usize,
) -> Option<[CachedPeak; 2]> {
    let mut merged = None;
    let per_range_budget = budget / ranges.len().max(1);
    let leaf_width = waveform
        .source_frames()
        .div_ceil(waveform.bins().len().max(1));
    for range in ranges {
        let span = range.end.saturating_sub(range.start);
        let peaks = if budget > 0 && span > 64 && span < leaf_width {
            waveform
                .bounded_exact_peaks_for_frames(range.start, range.end, per_range_budget)
                .or_else(|| aggregate_peaks(waveform, range.start, range.end))?
        } else {
            aggregate_peaks(waveform, range.start, range.end)?
        };
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

#[cfg(all(test, feature = "ui-unit-tier"))]
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
    fn narrow_loop_ranges_refine_within_budget_and_fall_back_without_pcm() {
        let mut samples = vec![[0.0, 0.0]; 4_096];
        samples[1_200] = [1.0, -1.0];
        let audio = hex_engine::Audio::new(48_000, samples).unwrap();
        let waveform = hex_media::waveform::summarize(&audio, 4).unwrap();
        let ranges = [1_024..1_100, 1_300..1_376];
        let exact = aggregate_ranges_with_detail(&waveform, &ranges, 152).unwrap();
        assert_eq!(exact[0].max, 0.0);
        let fallback = aggregate_ranges_with_detail(&waveform, &ranges, 0).unwrap();
        assert_eq!(fallback[0].max, 1.0);
        let peak_only = CachedWaveform::from_peaks(4_096, waveform.bins().to_vec()).unwrap();
        assert_eq!(
            aggregate_ranges_with_detail(&peak_only, &ranges, 152),
            Some(fallback)
        );
        assert!(aggregate_ranges_with_detail(&waveform, &[20..10], 152).is_none());
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
