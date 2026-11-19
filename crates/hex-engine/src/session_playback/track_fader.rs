//! Audio-owned gain transition for a track's post-device fader.
use crate::Frame;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TrackFader {
    current: Frame,
    target: Frame,
    remaining: u32,
}

impl TrackFader {
    pub(super) const fn new(gain: Frame) -> Self {
        Self {
            current: gain,
            target: gain,
            remaining: 0,
        }
    }

    pub(super) fn retarget(&mut self, target: Frame, sample_rate: u32) {
        if self.target != target {
            self.target = target;
            self.remaining = sample_rate.div_ceil(200).max(1);
        }
    }

    pub(super) fn active(self) -> bool {
        self.remaining != 0
    }

    pub(super) fn current(self) -> Frame {
        self.current
    }

    pub(super) fn follow(&mut self, gain: Frame) {
        *self = Self::new(gain);
    }

    pub(super) fn next(&mut self) -> Frame {
        if self.remaining != 0 {
            for channel in 0..2 {
                self.current[channel] +=
                    (self.target[channel] - self.current[channel]) / self.remaining as f32;
            }
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            }
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retarget_uses_current_gain_and_repeated_targets_keep_progress() {
        let mut fader = TrackFader::new([1.0, 0.5]);
        fader.retarget([0.0, 0.0], 1000);
        assert_eq!(fader.next(), [0.8, 0.4]);
        fader.retarget([0.0, 0.0], 1000);
        assert_eq!(fader.next(), [0.6, 0.3]);
        fader.retarget([1.0, 0.5], 1000);
        let first = fader.next();
        assert!((first[0] - 0.68).abs() < 1e-6);
        for _ in 0..4 {
            fader.next();
        }
        assert_eq!(fader.next(), [1.0, 0.5]);
        assert!(!fader.active());
    }

    #[test]
    fn duration_scales_with_sample_rate_and_reaches_exact_zero() {
        for rate in [44_100, 48_000, 96_000] {
            let mut fader = TrackFader::new([1.0; 2]);
            fader.retarget([0.0; 2], rate);
            for _ in 1..rate.div_ceil(200) {
                assert!(fader.next()[0] > 0.0);
            }
            assert_eq!(fader.next(), [0.0; 2]);
            assert!(!fader.active());
        }
    }
}
