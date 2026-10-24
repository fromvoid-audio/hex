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
}

impl Waveform {
    /// Exact retained PCM identity, for control-side summary reuse across revisions.
    pub fn shares_audio_with(&self, audio: &Audio) -> bool {
        self.samples
            .as_ref()
            .is_some_and(|samples| samples.shares_pcm_with(audio))
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
        })
    }

    /// Data bins retained by all resolutions, for control-side resident accounting.
    pub fn cached_bin_count(&self) -> usize {
        self.levels.iter().fold(self.bins.len(), |count, level| {
            count.saturating_add(level.len())
        })
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
        if end - start <= 64 {
            if let Some(audio) = &self.samples {
                let frames = audio.frames().get(start..end)?;
                let first = frames.first()?;
                let mut peaks = first.map(|sample| Peak {
                    min: sample,
                    max: sample,
                });
                for frame in &frames[1..] {
                    merge(
                        &mut peaks,
                        frame.map(|sample| Peak {
                            min: sample,
                            max: sample,
                        }),
                    );
                }
                return Some(peaks);
            }
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
        let mut left = bin_at(start);
        let mut right = bin_at(end - 1) + 1;
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

/// Peak-only browser metadata. Decoded PCM is borrowed during construction and
/// is not kept alive by the summary or its range-query hierarchy.
pub fn summarize_peaks(audio: &Audio, requested_bins: usize) -> Result<Waveform, ImportError> {
    if requested_bins == 0 || requested_bins > MAX_BINS {
        return Err(ImportError::LimitExceeded("waveform bins must be 1..65536"));
    }
    let frames = audio.frames();
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
