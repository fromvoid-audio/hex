//! Control-side peak summaries. Compute once after import, cache for visible UI.
use crate::ImportError;
use hex_engine::Audio;

pub const MAX_BINS: usize = 65_536;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}
/// A bin summarizes a nonempty, disjoint interval of the original PCM. The full
/// source is covered exactly once, even when its length is not divisible by bins.
#[derive(Debug, Clone)]
pub struct Waveform {
    pub source_frames: usize,
    pub bins: Vec<[Peak; 2]>,
    // Pairwise extrema above the leaf bins; built once on the control worker.
    levels: Vec<Vec<[Peak; 2]>>,
    samples: Option<Audio>,
    /// Present when summary frame `f` reads `samples[offset + (phase + f) % period]`.
    repeat: Option<Repeat>,
    // A bounded pyramid for one cycle prevents an extended clip from losing
    // source detail as its outer-summary bins grow.
    repeat_root: Option<Box<Waveform>>,
}

/// Loop geometry over retained samples, for clips that repeat one shared root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeat {
    pub offset: usize,
    pub period: usize,
    pub phase: usize,
}

impl Waveform {
    /// Exact retained PCM identity, for control-side summary reuse across revisions.
    /// A repeated summary never matches here; see `shares_repeat_with`.
    pub fn shares_audio_with(&self, audio: &Audio) -> bool {
        self.repeat.is_none()
            && self
                .samples
                .as_ref()
                .is_some_and(|samples| samples.shares_pcm_with(audio))
    }

    /// Exact retained PCM identity and loop geometry of a repeated summary.
    pub fn shares_repeat_with(&self, audio: &Audio, repeat: Repeat, frames: usize) -> bool {
        self.repeat == Some(repeat)
            && self.source_frames == frames
            && self
                .samples
                .as_ref()
                .is_some_and(|samples| samples.shares_pcm_with(audio))
    }

    /// Retained PCM index for a summary frame; `None` outside retained samples.
    fn sample(&self, frame: usize) -> Option<[f32; 2]> {
        let frames = self.samples.as_ref()?.frames();
        let index = match self.repeat {
            None => frame,
            Some(repeat) => repeat.offset + (repeat.phase + frame % repeat.period) % repeat.period,
        };
        frames.get(index).copied()
    }

    /// Exact extrema of retained PCM over a nonempty summary range.
    fn exact_range(&self, start: usize, end: usize) -> Option<[Peak; 2]> {
        let mut peaks = self.sample(start)?.map(|sample| Peak {
            min: sample,
            max: sample,
        });
        for frame in start + 1..end {
            merge(
                &mut peaks,
                self.sample(frame)?.map(|sample| Peak {
                    min: sample,
                    max: sample,
                }),
            );
        }
        Some(peaks)
    }

    /// Construct immutable cached peaks without resident PCM (e.g. fixtures).
    pub fn from_peaks(source_frames: usize, bins: Vec<[Peak; 2]>) -> Result<Self, ImportError> {
        if source_frames == 0
            || bins.is_empty()
            || bins.len() > MAX_BINS
            || bins.len() > source_frames
        {
            return Err(ImportError::LimitExceeded("invalid waveform extent"));
        }
        if bins
            .iter()
            .flatten()
            .any(|p| !p.min.is_finite() || !p.max.is_finite() || p.min > p.max)
        {
            return Err(ImportError::LimitExceeded("invalid waveform peaks"));
        }
        let mut levels = Vec::new();
        levels
            .try_reserve_exact(16)
            .map_err(|_| ImportError::AllocationFailed)?;
        let mut previous = bins.as_slice();
        while previous.len() > 1 {
            let mut next = Vec::new();
            next.try_reserve_exact(previous.len().div_ceil(2))
                .map_err(|_| ImportError::AllocationFailed)?;
            for pair in previous.chunks(2) {
                let mut peak = pair[0];
                if let Some(other) = pair.get(1) {
                    merge(&mut peak, *other);
                }
                next.push(peak);
            }
            levels.push(next);
            previous = levels.last().map(Vec::as_slice).unwrap_or(&[]);
        }
        Ok(Self {
            source_frames,
            bins,
            levels,
            samples: None,
            repeat: None,
            repeat_root: None,
        })
    }

    /// Leaf bins retained by this summary and its repeat-cycle cache.
    pub fn cached_leaf_count(&self) -> usize {
        self.bins.len().saturating_add(
            self.repeat_root
                .as_ref()
                .map_or(0, |root| root.cached_leaf_count()),
        )
    }

    /// Data bins retained by all resolutions, for control-side resident accounting.
    pub fn cached_bin_count(&self) -> usize {
        let count = self.levels.iter().fold(self.bins.len(), |count, level| {
            count.saturating_add(level.len())
        });
        count.saturating_add(
            self.repeat_root
                .as_ref()
                .map_or(0, |root| root.cached_bin_count()),
        )
    }

    /// Whether this summary represents repetitions of retained source PCM.
    pub fn is_repeated(&self) -> bool {
        self.repeat.is_some()
    }

    /// Exact resident-PCM extrema for a caller-bounded range. Returns `None`
    /// when PCM is not retained or the range exceeds the caller's work limit.
    fn exact_peaks_for_frames(
        &self,
        start: usize,
        end: usize,
        max_frames: usize,
    ) -> Option<[Peak; 2]> {
        let start = start.min(self.source_frames);
        let end = end.min(self.source_frames);
        if start >= end || end - start > max_frames {
            return None;
        }
        self.exact_range(start, end)
    }

    /// Exact extrema using cached whole bins and at most `max_pcm_frames`
    /// resident frames at the two partial boundaries. Returns `None` when the
    /// source has no retained PCM or the boundary work exceeds that limit.
    pub fn bounded_exact_peaks_for_frames(
        &self,
        start: usize,
        end: usize,
        max_pcm_frames: usize,
    ) -> Option<[Peak; 2]> {
        self.samples.as_ref()?;
        let start = start.min(self.source_frames);
        let end = end.min(self.source_frames);
        if start >= end || self.bins.is_empty() || self.bins.len() > self.source_frames {
            return None;
        }
        if let (Some(repeat), Some(root)) = (self.repeat, self.repeat_root.as_deref()) {
            let length = end - start;
            if length >= repeat.period {
                return root.cached_peaks_for_bins(0, root.bins.len());
            }
            let phase = start % repeat.period;
            let begin = if repeat.phase >= repeat.period - phase {
                repeat.phase - (repeat.period - phase)
            } else {
                repeat.phase + phase
            };
            let first_len = length.min(repeat.period - begin);
            if first_len == length {
                return root.bounded_exact_peaks_for_frames(begin, begin + length, max_pcm_frames);
            }
            let first_budget = max_pcm_frames / 2;
            let mut peaks =
                root.bounded_exact_peaks_for_frames(begin, begin + first_len, first_budget)?;
            let rest = root.bounded_exact_peaks_for_frames(
                0,
                length - first_len,
                max_pcm_frames - first_budget,
            )?;
            merge(&mut peaks, rest);
            return Some(peaks);
        }
        let count = self.bins.len();
        let width = self.source_frames / count;
        let extra = self.source_frames % count;
        let wide_frames = (width + 1).saturating_mul(extra);
        let bin_at = |frame: usize| {
            if frame < wide_frames {
                frame / (width + 1)
            } else {
                extra + (frame - wide_frames) / width
            }
        };
        let boundary = |bin: usize| bin * width + bin.min(extra);
        let first = bin_at(start);
        let last = bin_at(end - 1);
        if first == last {
            if start == boundary(first) && end == boundary(first + 1) {
                return self.bins.get(first).copied();
            }
            return self.exact_peaks_for_frames(start, end, max_pcm_frames);
        }
        let whole_start = if start == boundary(first) {
            start
        } else {
            boundary(first + 1)
        };
        let whole_end = if end == boundary(last + 1) {
            end
        } else {
            boundary(last)
        };
        let partial = (whole_start - start).checked_add(end - whole_end)?;
        if partial > max_pcm_frames {
            return None;
        }
        let mut result = None;
        if start < whole_start {
            include(
                &mut result,
                self.exact_peaks_for_frames(start, whole_start, max_pcm_frames)?,
            );
        }
        if whole_start < whole_end {
            let first_whole = first + usize::from(start < whole_start);
            let after_whole = last + usize::from(whole_end == end);
            include(
                &mut result,
                self.cached_peaks_for_bins(first_whole, after_whole)?,
            );
        }
        if whole_end < end {
            include(
                &mut result,
                self.exact_peaks_for_frames(whole_end, end, max_pcm_frames)?,
            );
        }
        result
    }

    /// Signed channel extrema for a half-open source range, with no allocation.
    /// Deep zoom reads at most 64 shared PCM frames; wider queries use cached
    /// pairwise extrema and conservatively include intersecting boundary bins.
    pub fn peaks_for_frames(&self, start: usize, end: usize) -> Option<[Peak; 2]> {
        let start = start.min(self.source_frames);
        let end = end.min(self.source_frames);
        if start >= end || self.bins.is_empty() || self.bins.len() > self.source_frames {
            return None;
        }
        if end - start <= 64 && self.samples.is_some() {
            return self.exact_range(start, end);
        }
        if let (Some(repeat), Some(root)) = (self.repeat, self.repeat_root.as_deref()) {
            let length = end - start;
            if length >= repeat.period {
                return root.peaks_for_frames(0, repeat.period);
            }
            let phase = start % repeat.period;
            let begin = if repeat.phase >= repeat.period - phase {
                repeat.phase - (repeat.period - phase)
            } else {
                repeat.phase + phase
            };
            let first_len = length.min(repeat.period - begin);
            let mut peaks = root.peaks_for_frames(begin, begin + first_len)?;
            if first_len < length {
                merge(&mut peaks, root.peaks_for_frames(0, length - first_len)?);
            }
            return Some(peaks);
        }
        let count = self.bins.len();
        let width = self.source_frames / count;
        let extra = self.source_frames % count;
        let wide_frames = (width + 1).saturating_mul(extra);
        let bin_at = |frame: usize| {
            if frame < wide_frames {
                frame / (width + 1)
            } else {
                extra + (frame - wide_frames) / width
            }
        };
        self.cached_peaks_for_bins(bin_at(start), bin_at(end - 1) + 1)
    }

    fn cached_peaks_for_bins(&self, mut left: usize, mut right: usize) -> Option<[Peak; 2]> {
        let mut level = 0;
        let mut result = None;
        while left < right {
            let nodes = if level == 0 {
                self.bins.as_slice()
            } else {
                self.levels.get(level - 1)?.as_slice()
            };
            if left % 2 == 1 {
                include(&mut result, *nodes.get(left)?);
                left += 1;
            }
            if right % 2 == 1 {
                right -= 1;
                include(&mut result, *nodes.get(right)?);
            }
            left /= 2;
            right /= 2;
            level += 1;
        }
        result
    }
}
fn merge(target: &mut [Peak; 2], other: [Peak; 2]) {
    for (target, other) in target.iter_mut().zip(other) {
        target.min = target.min.min(other.min);
        target.max = target.max.max(other.max);
    }
}
fn include(result: &mut Option<[Peak; 2]>, peaks: [Peak; 2]) {
    if let Some(current) = result {
        merge(current, peaks);
    } else {
        *result = Some(peaks);
    }
}

/// Summary for editor deep zoom: shares resident PCM for exact sub-bin queries.
pub fn summarize(audio: &Audio, requested_bins: usize) -> Result<Waveform, ImportError> {
    let mut waveform = summarize_peaks(audio, requested_bins)?;
    waveform.samples = Some(audio.clone());
    Ok(waveform)
}

/// Summary of `frames` frames looping `audio[offset..offset + period)` from
/// `phase`, without materializing the repeated PCM. Shares the root for deep zoom.
pub fn summarize_repeated(
    audio: &Audio,
    repeat: Repeat,
    frames: usize,
    requested_bins: usize,
) -> Result<Waveform, ImportError> {
    if requested_bins == 0 || requested_bins > MAX_BINS {
        return Err(ImportError::LimitExceeded("waveform bins must be 1..65536"));
    }
    if frames == 0
        || repeat.period == 0
        || repeat.phase >= repeat.period
        || repeat
            .offset
            .checked_add(repeat.period)
            .is_none_or(|end| end > audio.frames().len())
    {
        return Err(ImportError::LimitExceeded(
            "invalid repeated waveform extent",
        ));
    }
    let root = &audio.frames()[repeat.offset..repeat.offset + repeat.period];
    let count = requested_bins.min(frames);
    let mut bins = Vec::new();
    bins.try_reserve_exact(count)
        .map_err(|_| ImportError::AllocationFailed)?;
    let width = frames / count;
    let extra = frames % count;
    let mut index = repeat.phase;
    for bin in 0..count {
        let length = width + usize::from(bin < extra);
        let mut peaks = root[index].map(|sample| Peak {
            min: sample,
            max: sample,
        });
        for _ in 0..length {
            for (peak, sample) in peaks.iter_mut().zip(root[index]) {
                peak.min = peak.min.min(sample);
                peak.max = peak.max.max(sample);
            }
            index += 1;
            if index == repeat.period {
                index = 0;
            }
        }
        bins.push(peaks);
    }
    let mut waveform = Waveform::from_peaks(frames, bins)?;
    waveform.samples = Some(audio.clone());
    waveform.repeat = Some(repeat);
    let mut root_summary = summarize_slice(root, requested_bins)?;
    root_summary.samples = Some(audio.clone());
    root_summary.repeat = Some(Repeat { phase: 0, ..repeat });
    waveform.repeat_root = Some(Box::new(root_summary));
    Ok(waveform)
}

/// Peak-only browser metadata. Decoded PCM is borrowed during construction and
/// is not kept alive by the summary or its range-query hierarchy.
pub fn summarize_peaks(audio: &Audio, requested_bins: usize) -> Result<Waveform, ImportError> {
    if requested_bins == 0 || requested_bins > MAX_BINS {
        return Err(ImportError::LimitExceeded("waveform bins must be 1..65536"));
    }
    summarize_slice(audio.frames(), requested_bins)
}

fn summarize_slice(frames: &[[f32; 2]], requested_bins: usize) -> Result<Waveform, ImportError> {
    let count = requested_bins.min(frames.len());
    let mut bins = Vec::new();
    bins.try_reserve_exact(count)
        .map_err(|_| ImportError::AllocationFailed)?;
    // Quotient/remainder avoids multiplication overflow for long resident sources.
    let width = frames.len() / count;
    let extra = frames.len() % count;
    let mut start = 0;
    for bin in 0..count {
        let end = start + width + usize::from(bin < extra);
        let first = frames[start];
        let mut peaks = first.map(|sample| Peak {
            min: sample,
            max: sample,
        });
        for frame in &frames[start + 1..end] {
            for (peak, sample) in peaks.iter_mut().zip(frame) {
                peak.min = peak.min.min(*sample);
                peak.max = peak.max.max(*sample);
            }
        }
        bins.push(peaks);
        start = end;
    }
    Waveform::from_peaks(frames.len(), bins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_summary_matches_materialized_loop_without_copying_pcm() {
        let root: Vec<_> = (0..97)
            .map(|i| [((i * 37) % 53) as f32, -(((i * 29) % 41) as f32)])
            .collect();
        let audio = Audio::new(48_000, root.clone()).unwrap();
        let repeat = Repeat {
            offset: 11,
            period: 60,
            phase: 17,
        };
        let frames = 1_013;
        let tiled: Vec<_> = (0..frames)
            .map(|f| root[repeat.offset + (repeat.phase + f) % repeat.period])
            .collect();
        let expected = summarize(&Audio::new(48_000, tiled).unwrap(), 37).unwrap();
        let actual = summarize_repeated(&audio, repeat, frames, 37).unwrap();
        assert_eq!(actual.source_frames, frames);
        assert_eq!(actual.bins, expected.bins);
        for (start, end) in [(0, 5), (55, 70), (1_000, 1_013), (40, 900), (0, frames)] {
            assert_eq!(
                actual.peaks_for_frames(start, end),
                expected.peaks_for_frames(start, end),
                "{start}..{end}"
            );
            assert_eq!(
                actual.bounded_exact_peaks_for_frames(start, end, 64),
                expected.bounded_exact_peaks_for_frames(start, end, 64),
                "exact {start}..{end}"
            );
        }
        // Loop geometry is part of identity; plain root reuse never matches it.
        assert!(actual.shares_repeat_with(&audio, repeat, frames));
        assert!(!actual.shares_repeat_with(
            &audio,
            Repeat {
                phase: 18,
                ..repeat
            },
            frames
        ));
        assert!(!actual.shares_repeat_with(&audio, repeat, frames - 1));
        assert!(!actual.shares_audio_with(&audio));
        for invalid in [
            Repeat {
                period: 0,
                ..repeat
            },
            Repeat {
                phase: 60,
                ..repeat
            },
            Repeat {
                offset: 38,
                ..repeat
            },
        ] {
            assert!(summarize_repeated(&audio, invalid, frames, 37).is_err());
        }
        assert!(summarize_repeated(&audio, repeat, 0, 37).is_err());
    }

    #[test]
    fn long_repeated_summary_queries_the_cycle_with_phase_and_wrap() {
        let mut pcm = vec![[0.0, 0.0]; 16_020];
        pcm[10 + 2_010] = [1.0, -0.75];
        pcm[10 + 15_990] = [-0.25, 0.5];
        let audio = Audio::new(48_000, pcm).unwrap();
        let repeat = Repeat {
            offset: 10,
            period: 16_000,
            phase: 100,
        };
        let waveform = summarize_repeated(&audio, repeat, 8_000_000, 1_024).unwrap();
        assert!(waveform.is_repeated());
        let zero = [Peak { min: 0.0, max: 0.0 }; 2];
        for cycle in [0, 249, 498] {
            let start = cycle * repeat.period;
            assert_eq!(
                waveform.peaks_for_frames(start + 6_000, start + 10_097),
                Some(zero)
            );
            let peak = waveform
                .peaks_for_frames(start + 1_900, start + 2_060)
                .unwrap();
            assert_eq!(peak[0].max, 1.0);
            assert_eq!(peak[1].min, -0.75);
            let seam = waveform
                .peaks_for_frames(start + 15_800, start + 16_100)
                .unwrap();
            assert_eq!(seam[0].min, -0.25);
            assert_eq!(seam[1].max, 0.5);
        }
        assert!(
            waveform.cached_bin_count() > waveform.bins.len() * 2,
            "both pyramids count toward retained budget"
        );
    }

    #[test]
    fn bounded_exact_peaks_respect_residency_limits_and_ranges() {
        let mut frames = vec![[0.0, 0.0]; 1_000];
        frames[300] = [0.75, -0.5];
        let audio = Audio::new(48_000, frames).unwrap();
        let resident = summarize(&audio, 1).unwrap();
        let peak_only = summarize_peaks(&audio, 1).unwrap();
        let zero = [Peak { min: 0.0, max: 0.0 }; 2];
        assert_eq!(resident.exact_peaks_for_frames(200, 300, 100), Some(zero));
        assert_eq!(resident.exact_peaks_for_frames(200, 301, 100), None);
        assert_eq!(
            resident.exact_peaks_for_frames(300, 301, 1).unwrap()[0].max,
            0.75
        );
        assert_eq!(
            resident.exact_peaks_for_frames(900, usize::MAX, 100),
            Some(zero)
        );
        assert_eq!(resident.exact_peaks_for_frames(1_000, 1_001, 100), None);
        assert_eq!(peak_only.exact_peaks_for_frames(200, 300, 100), None);
    }

    #[test]
    fn bounded_hierarchy_matches_pcm_for_partial_and_whole_bins() {
        let frames: Vec<_> = (0..257)
            .map(|i| [((i * 37) % 97) as f32, -(((i * 29) % 89) as f32)])
            .collect();
        let audio = Audio::new(48_000, frames).unwrap();
        let resident = summarize(&audio, 17).unwrap();
        let peak_only = summarize_peaks(&audio, 17).unwrap();
        for start in 0..257 {
            for end in (start + 1)..=257 {
                assert_eq!(
                    resident.bounded_exact_peaks_for_frames(start, end, 64),
                    resident.exact_peaks_for_frames(start, end, 257),
                    "range {start}..{end}"
                );
            }
        }
        // The first bin contains 16 frames. Aligned whole bins need no PCM
        // boundary reads; a partial range must respect the supplied budget.
        assert!(resident.bounded_exact_peaks_for_frames(0, 16, 0).is_some());
        assert!(resident.bounded_exact_peaks_for_frames(1, 31, 1).is_none());
        assert!(
            peak_only
                .bounded_exact_peaks_for_frames(0, 16, 64)
                .is_none()
        );
        let mut malformed = resident.clone();
        malformed.bins.clear();
        assert!(
            malformed
                .bounded_exact_peaks_for_frames(0, 16, 64)
                .is_none()
        );
    }

    #[test]
    #[ignore = "manual optimized waveform query comparison"]
    fn event_detail_query_profile() {
        use std::{hint::black_box, time::Instant};
        let frames: Vec<_> = (0..4_194_000)
            .map(|i| {
                let v = ((i % 257) as f32 / 128.0) - 1.0;
                [v, -v]
            })
            .collect();
        let audio = Audio::new(48_000, frames).unwrap();
        let waveform = summarize(&audio, 1_024).unwrap();
        let run = |exact: bool| {
            let start = Instant::now();
            for _ in 0..50 {
                for column in 0..2_000 {
                    let left = column * 2_097;
                    let right = left + 2_097;
                    let peaks = if exact {
                        waveform.bounded_exact_peaks_for_frames(left, right, 2_097)
                    } else {
                        waveform.peaks_for_frames(left, right)
                    };
                    black_box(peaks);
                }
            }
            start.elapsed()
        };
        // Warm both paths; each pass queries 4,194,000 frames.
        black_box(run(false));
        black_box(run(true));
        println!(
            "summary={:?} exact={:?} passes=50 frames_per_pass=4194000 columns=2000",
            run(false),
            run(true)
        );
    }
    #[test]
    fn nondivisible_source_keeps_every_transient_and_stereo_extrema() {
        let audio = Audio::new(
            48_000,
            vec![
                [0.5, -0.5],
                [-0.8, 0.2],
                [0.0, 0.9],
                [2.0, -2.0],
                [-0.2, 0.1],
            ],
        )
        .unwrap();
        let waveform = summarize(&audio, 2).unwrap();
        assert_eq!(waveform.source_frames, 5);
        assert_eq!(
            waveform.bins,
            vec![
                [
                    Peak {
                        min: -0.8,
                        max: 0.5
                    },
                    Peak {
                        min: -0.5,
                        max: 0.9
                    }
                ],
                [
                    Peak {
                        min: -0.2,
                        max: 2.0
                    },
                    Peak {
                        min: -2.0,
                        max: 0.1
                    }
                ],
            ]
        );
    }
    #[test]
    fn detailed_browser_summary_keeps_impulses_and_releases_pcm() {
        let mut frames = vec![[0.125, -0.25]; 8195];
        frames[4097] = [-1.0, 0.875];
        frames[8194] = [0.75, -0.9];
        let audio = Audio::new(48_000, frames).unwrap();
        let summary = summarize_peaks(&audio, 4096).unwrap();
        assert_eq!(summary.bins.len(), 4096);
        assert!(
            summary.samples.is_none(),
            "browser cache must not retain decoded PCM"
        );
        assert!(!summary.shares_audio_with(&audio));
        assert_eq!(
            summary.peaks_for_frames(4097, 4098),
            Some([
                Peak {
                    min: -1.0,
                    max: 0.125
                },
                Peak {
                    min: -0.25,
                    max: 0.875
                }
            ])
        );
        assert_eq!(
            summary.peaks_for_frames(8193, 8195),
            Some([
                Peak {
                    min: 0.125,
                    max: 0.75
                },
                Peak {
                    min: -0.9,
                    max: -0.25
                }
            ])
        );
        drop(audio);
        assert_eq!(
            summary.peaks_for_frames(0, 2),
            Some([
                Peak {
                    min: 0.125,
                    max: 0.125
                },
                Peak {
                    min: -0.25,
                    max: -0.25
                }
            ])
        );
        let silence = Audio::new(8000, vec![[0.0; 2]; 3]).unwrap();
        let tiny = summarize_peaks(&silence, 4096).unwrap();
        assert_eq!(tiny.bins.len(), 3);
        assert_eq!(
            tiny.peaks_for_frames(0, 3),
            Some([Peak { min: 0.0, max: 0.0 }; 2])
        );
    }

    #[test]
    fn tiny_sources_and_limits_are_explicit() {
        let audio = Audio::new(48_000, vec![[0.25, -0.25]]).unwrap();
        assert_eq!(summarize(&audio, 1024).unwrap().bins.len(), 1);
        assert!(summarize(&audio, 0).is_err());
        assert!(summarize(&audio, MAX_BINS + 1).is_err());
    }
    #[test]
    fn cached_range_tree_matches_direct_signed_extrema_for_odd_leaf_counts() {
        let bins: Vec<_> = (0..97)
            .map(|i| {
                let x = ((i * 37) % 97) as f32 / 97.0;
                [
                    Peak {
                        min: -x,
                        max: x / 2.0,
                    },
                    Peak {
                        min: -x / 3.0,
                        max: x,
                    },
                ]
            })
            .collect();
        let waveform = Waveform::from_peaks(97, bins.clone()).expect("cache");
        for start in 0..97 {
            for end in start + 1..=97 {
                let mut expected = bins[start];
                for bin in &bins[start + 1..end] {
                    merge(&mut expected, *bin);
                }
                assert_eq!(waveform.peaks_for_frames(start, end), Some(expected));
            }
        }
        assert!(waveform.peaks_for_frames(97, usize::MAX).is_none());
        assert!(waveform.cached_bin_count() <= 2 * 97 + 16);
    }

    #[test]
    fn deep_zoom_reads_real_samples_without_bleeding_neighboring_transients() {
        let mut frames = vec![[0.0; 2]; 257];
        frames[10] = [-1.0, 0.75];
        let audio = Audio::new(48_000, frames).expect("audio");
        let waveform = summarize(&audio, 2).expect("summary");
        assert_eq!(
            waveform.peaks_for_frames(10, 11),
            Some([
                Peak {
                    min: -1.0,
                    max: -1.0
                },
                Peak {
                    min: 0.75,
                    max: 0.75
                }
            ])
        );
        assert_eq!(
            waveform.peaks_for_frames(11, 12),
            Some([Peak { min: 0.0, max: 0.0 }; 2])
        );
        let full = waveform.peaks_for_frames(0, 257).expect("overview");
        assert_eq!(full[0].min, -1.0);
        assert_eq!(full[1].max, 0.75);
    }

    #[test]
    fn peak_fixture_validation_refuses_invalid_numbers_and_extents() {
        assert!(Waveform::from_peaks(0, Vec::new()).is_err());
        assert!(
            Waveform::from_peaks(
                1,
                vec![
                    [Peak {
                        min: f32::NAN,
                        max: 1.0
                    }; 2]
                ]
            )
            .is_err()
        );
        assert!(
            Waveform::from_peaks(
                1,
                vec![
                    [Peak {
                        min: 1.0,
                        max: -1.0
                    }; 2]
                ]
            )
            .is_err()
        );
    }
}
