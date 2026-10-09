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
}

pub fn summarize(audio: &Audio, requested_bins: usize) -> Result<Waveform, ImportError> {
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
    Ok(Waveform {
        source_frames: frames.len(),
        bins,
    })
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
    fn tiny_sources_and_limits_are_explicit() {
        let audio = Audio::new(48_000, vec![[0.25, -0.25]]).unwrap();
        assert_eq!(summarize(&audio, 1024).unwrap().bins.len(), 1);
        assert!(summarize(&audio, 0).is_err());
        assert!(summarize(&audio, MAX_BINS + 1).is_err());
    }
}
