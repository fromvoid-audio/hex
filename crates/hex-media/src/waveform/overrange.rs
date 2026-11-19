//! Bounded exact over-range decisions from retained waveform PCM.
use super::Waveform;
use std::ops::Range;

/// Channels with samples above positive or below negative full scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overrange {
    pub positive: [bool; 2],
    pub negative: [bool; 2],
}

impl Waveform {
    /// Evaluate the per-frame fade and clip gain in that order and report samples
    /// beyond full scale. The caller owns timeline mapping and must supply unity
    /// for any gain already applied to the retained PCM. Multiplication stays
    /// sequential, matching the playback path at the full-scale boundary.
    ///
    /// Returns `None` for missing PCM, invalid/empty ranges, insufficient budget,
    /// unsupported inconsistent storage, or absent/invalid gain. This is not a
    /// no-clipping result. The frame budget covers logical frames, including
    /// sparse silence and repeated frames, and is checked before the first gain
    /// query. Work is linear in the admitted range (plus sparse-map lookup); this
    /// method allocates nothing, excluding the caller-supplied gain function.
    /// This control-side operation must not run on the audio callback.
    pub fn bounded_overrange(
        &self,
        range: Range<usize>,
        max_frames: usize,
        mut gain_at: impl FnMut(usize) -> Option<(f32, f32)>,
    ) -> Option<Overrange> {
        let length = range.end.checked_sub(range.start)?;
        if length == 0 || length > max_frames || range.end > self.source_frames {
            return None;
        }
        let samples = self.samples.as_ref()?.frames();
        if self.repeat.is_some() && self.sparse.is_some() {
            return None;
        }
        let mut result = Overrange::default();
        let mut invalid = false;
        let mut visit = |index, frame: Option<[f32; 2]>| {
            if invalid {
                return;
            }
            let Some((frame, (fade, gain))) = frame.zip(gain_at(index)) else {
                invalid = true;
                return;
            };
            if !fade.is_finite()
                || fade < 0.0
                || !gain.is_finite()
                || gain < 0.0
                || frame.iter().any(|sample| !sample.is_finite())
            {
                invalid = true;
                return;
            }
            for (channel, sample) in frame.into_iter().enumerate() {
                let value = (sample * fade) * gain;
                if !value.is_finite() {
                    invalid = true;
                    return;
                }
                result.positive[channel] |= value > 1.0;
                result.negative[channel] |= value < -1.0;
            }
        };
        if let Some(repeat) = self.repeat {
            if repeat.period == 0 || repeat.phase >= repeat.period {
                return None;
            }
            let offset = range.start % repeat.period;
            let mut phase = if repeat.phase >= repeat.period - offset {
                repeat.phase - (repeat.period - offset)
            } else {
                repeat.phase + offset
            };
            for index in range {
                let sample = repeat
                    .offset
                    .checked_add(phase)
                    .and_then(|i| samples.get(i))
                    .copied();
                visit(index, sample);
                phase += 1;
                if phase == repeat.period {
                    phase = 0;
                }
            }
        } else if let Some(map) = &self.sparse {
            let mut cursor = range.start;
            map.visit(range.clone(), |logical, packed| {
                while cursor < logical.start {
                    visit(cursor, Some([0.0; 2]));
                    cursor += 1;
                }
                for (index, packed_index) in logical.clone().zip(packed) {
                    visit(index, samples.get(packed_index).copied());
                }
                cursor = logical.end;
            });
            while cursor < range.end {
                visit(cursor, Some([0.0; 2]));
                cursor += 1;
            }
        } else {
            for index in range {
                visit(index, samples.get(index).copied());
            }
        }
        (!invalid).then_some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::{Repeat, summarize, summarize_peaks, summarize_repeated};
    use hex_engine::Audio;
    use hex_project::ClipFade;
    use std::cell::Cell;

    #[test]
    fn exact_gain_preserves_sample_time_and_channel_polarity() {
        let mut frames = vec![[0.0; 2]; 100];
        frames[0] = [3.0, -3.0];
        frames[75] = [2.0, -2.0];
        frames[99] = [-1.0, 1.0];
        let audio = Audio::new(48_000, frames.clone()).unwrap();
        let waveform = summarize(&audio, 1).unwrap();
        let fade = ClipFade::new(100, 0, 100, 0).unwrap();
        let expected = |range: Range<usize>| {
            let mut result = Overrange::default();
            for i in range {
                for (channel, sample) in frames[i].iter().copied().enumerate() {
                    let value = sample * fade.gain_at(i as u64);
                    result.positive[channel] |= value > 1.0;
                    result.negative[channel] |= value < -1.0;
                }
            }
            result
        };
        for range in [0..1, 0..50, 0..100, 70..90, 99..100] {
            assert_eq!(
                waveform
                    .bounded_overrange(range.clone(), 100, |i| Some((fade.gain_at(i as u64), 1.0))),
                Some(expected(range))
            );
        }
        assert_eq!(
            waveform.bounded_overrange(0..1, 1, |i| Some((fade.gain_at(i as u64), 1.0))),
            Some(Overrange::default())
        );
        let exact = summarize(&Audio::new(48_000, vec![[1.0, -1.0]]).unwrap(), 1).unwrap();
        assert_eq!(
            exact.bounded_overrange(0..1, 1, |_| Some((1.0, 1.0))),
            Some(Overrange::default())
        );
    }

    #[test]
    fn repeated_pcm_uses_logical_coordinates_across_wraps() {
        let audio = Audio::new(48_000, vec![[0.0; 2], [2.0, -2.0], [-3.0, 3.0], [0.5; 2]]).unwrap();
        let repeat = Repeat {
            offset: 1,
            period: 3,
            phase: 2,
        };
        let waveform = summarize_repeated(&audio, repeat, 12, 4).unwrap();
        let visited = std::cell::RefCell::new(Vec::new());
        let result = waveform
            .bounded_overrange(1..6, 5, |i| {
                visited.borrow_mut().push(i);
                Some((if i == 2 { 0.0 } else { 1.0 }, 1.0))
            })
            .unwrap();
        let mut expected = Overrange::default();
        for i in 1..6 {
            let frame = audio.frames()[1 + (2 + i) % 3];
            let gain = if i == 2 { 0.0 } else { 1.0 };
            for (channel, sample) in frame.iter().copied().enumerate() {
                expected.positive[channel] |= sample * gain > 1.0;
                expected.negative[channel] |= sample * gain < -1.0;
            }
        }
        assert_eq!(result, expected);
        assert_eq!(*visited.borrow(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn threshold_uses_the_engines_sequential_multiplication() {
        let sample = 1.790_487_4_f32;
        let fade = 0.196_145_68_f32;
        let clip_gain = hex_project::ClipGain::new(9_089).unwrap().linear();
        let sequential = (sample * fade) * clip_gain;
        let combined = sample * (fade * clip_gain);
        assert!(
            sequential > 1.0 && combined <= 1.0,
            "fixture distinguishes operation order"
        );
        let audio = Audio::new(48_000, vec![[sample, -sample]]).unwrap();
        let waveform = summarize(&audio, 1).unwrap();
        let over = waveform
            .bounded_overrange(0..1, 1, |_| Some((fade, clip_gain)))
            .unwrap();
        assert_eq!(over.positive, [true, false]);
        assert_eq!(over.negative, [false, true]);
    }

    #[test]
    fn unavailable_and_over_budget_are_never_silence_or_partial_work() {
        let audio = Audio::new(48_000, vec![[2.0, -2.0]; 8]).unwrap();
        let cached = summarize(&audio, 4).unwrap();
        let peak_only = summarize_peaks(&audio, 4).unwrap();
        let calls = Cell::new(0);
        let query = |i| {
            calls.set(calls.get() + 1);
            Some((i as f32, 1.0))
        };
        assert_eq!(peak_only.bounded_overrange(0..8, 8, query), None);
        assert_eq!(cached.bounded_overrange(0..8, 7, query), None);
        assert_eq!(cached.bounded_overrange(3..3, 8, query), None);
        assert_eq!(
            cached.bounded_overrange(Range { start: 5, end: 1 }, 8, query),
            None
        );
        assert_eq!(cached.bounded_overrange(0..9, 9, query), None);
        assert_eq!(calls.get(), 0);
        for invalid in [f32::NAN, f32::INFINITY, -1.0] {
            assert_eq!(
                cached.bounded_overrange(0..2, 2, |_| Some((invalid, 1.0))),
                None
            );
        }
        assert_eq!(cached.bounded_overrange(0..2, 2, |_| None), None);
    }
}
