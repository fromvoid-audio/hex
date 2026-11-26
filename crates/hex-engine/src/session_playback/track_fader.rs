//! Audio-owned gain transition for a track's post-device fader.
use crate::Frame;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TrackFader {
    current: Frame,
    target: Frame,
    remaining: u32,
    removing: bool,
    mute_current: f32,
    mute_target: f32,
    mute_remaining: u32,
    muting: bool,
    mute_touched: bool,
}

impl TrackFader {
    pub(super) const fn new(gain: Frame) -> Self {
        Self {
            current: gain,
            target: gain,
            remaining: 0,
            removing: false,
            mute_current: 1.0,
            mute_target: 1.0,
            mute_remaining: 0,
            muting: false,
            mute_touched: false,
        }
    }

    pub(super) fn retarget(&mut self, target: Frame, sample_rate: u32) {
        if self.target != target {
            self.target = target;
            self.remaining = Self::fade_frames(sample_rate);
        }
    }

    pub(super) fn fade_frames(sample_rate: u32) -> u32 {
        sample_rate.div_ceil(200).max(1)
    }

    pub(super) fn active(self) -> bool {
        self.removing || self.remaining != 0 || self.muting || self.mute_remaining != 0
    }

    pub(super) fn is_removing(self) -> bool {
        self.removing
    }

    pub(super) fn mute_active(self) -> bool {
        self.muting || self.mute_remaining != 0
    }

    pub(super) fn reset_mute_touched(&mut self) {
        self.mute_touched = false;
    }

    pub(super) fn mute_touched(self) -> bool {
        self.mute_touched
    }

    /// Ramp this track's mute multiplier smoothly to zero while the old graph
    /// retains exact source, effect, and stream-page ownership (#7600/#7601/#7602).
    pub(super) fn begin_mute(&mut self, sample_rate: u32) {
        self.muting = true;
        if self.mute_target != 0.0 {
            self.mute_target = 0.0;
            self.mute_remaining = Self::fade_frames(sample_rate);
        }
    }

    pub(super) fn mute_ready(self) -> bool {
        self.muting && self.mute_remaining == 0
    }

    /// Reverse a cancelled pre-commit mute fade smoothly back to unity.
    pub(super) fn cancel_mute(&mut self, sample_rate: u32) {
        if self.muting {
            self.muting = false;
            self.mute_target = 1.0;
            self.mute_remaining = Self::fade_frames(sample_rate);
        }
    }

    /// Ramp a newly unmuted track smoothly from silence to unity.
    pub(super) fn begin_unmute(&mut self, sample_rate: u32) {
        self.muting = false;
        self.mute_current = 0.0;
        self.mute_target = 1.0;
        self.mute_remaining = Self::fade_frames(sample_rate);
    }

    /// Reset mute transition state once a muted successor is installed.
    pub(super) fn clear_mute(&mut self) {
        self.muting = false;
        self.mute_current = 1.0;
        self.mute_target = 1.0;
        self.mute_remaining = 0;
        self.mute_touched = false;
    }

    /// Advance a mute ramp on a silent/skipped track so pre-commit mute fades
    /// always settle deterministically in `fade_frames(sample_rate)` frames.
    pub(super) fn advance_mute_silent(&mut self, frames: usize) {
        if self.mute_remaining == 0 {
            return;
        }
        let steps = u32::try_from(frames).unwrap_or(u32::MAX);
        if steps >= self.mute_remaining {
            self.mute_current = self.mute_target;
            self.mute_remaining = 0;
        } else {
            let fraction = steps as f32 / self.mute_remaining as f32;
            self.mute_current += (self.mute_target - self.mute_current) * fraction;
            self.mute_remaining -= steps;
        }
        self.mute_touched = true;
    }

    /// Keep using this fader at silence while the old graph owns the track.
    pub(super) fn begin_removal(&mut self, sample_rate: u32) {
        self.removing = true;
        self.retarget([0.0; 2], sample_rate);
    }

    pub(super) fn removal_ready(self) -> bool {
        self.removing && self.remaining == 0
    }

    pub(super) fn cancel_removal(&mut self, gain: Frame, sample_rate: u32) {
        if self.removing {
            self.removing = false;
            self.retarget(gain, sample_rate);
        }
    }

    pub(super) fn current(self) -> Frame {
        [
            self.current[0] * self.mute_current,
            self.current[1] * self.mute_current,
        ]
    }

    pub(super) fn follow(&mut self, gain: Frame) {
        self.current = gain;
        self.target = gain;
        self.remaining = 0;
        self.removing = false;
    }

    pub(super) fn next_mute_scale(&mut self) -> f32 {
        self.mute_touched = true;
        if self.mute_remaining != 0 {
            self.mute_current +=
                (self.mute_target - self.mute_current) / self.mute_remaining as f32;
            self.mute_remaining -= 1;
            if self.mute_remaining == 0 {
                self.mute_current = self.mute_target;
            }
        }
        self.mute_current
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
        let scale = self.next_mute_scale();
        [self.current[0] * scale, self.current[1] * scale]
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
