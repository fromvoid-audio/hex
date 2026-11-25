//! Shared track, Return, and Main meter decay, peak-hold, and latched-clip state.
use super::scale;
use crate::output_meter::{MeterReading, PEAK_HOLD_DURATION};
use std::time::Instant;

pub(super) fn decay_factor(updated: &mut Option<Instant>, now: Instant) -> f32 {
    let elapsed = updated.map_or(0.0, |previous| {
        now.saturating_duration_since(previous).as_secs_f32()
    });
    *updated = Some(now);
    10.0_f32.powf(-12.0 * elapsed / 20.0)
}

pub(super) fn decay_level(level: f32, decay: f32) -> f32 {
    let next = level * decay;
    if next < scale::FLOOR_AMPLITUDE {
        0.0
    } else {
        next
    }
}

fn sanitize_peak(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// Unified per-channel smooth decay, peak-hold, and callback-latched clip state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct StereoMeterState {
    level: [f32; 2],
    peak_hold: [f32; 2],
    hold_until: [Option<Instant>; 2],
    clipped: [bool; 2],
}

impl StereoMeterState {
    #[cfg(test)]
    pub(super) fn from_peak(peaks: [f32; 2], now: Instant) -> Self {
        let mut state = Self::default();
        state.observe(peaks, [false; 2], 1.0, now);
        state
    }

    #[cfg(test)]
    pub(super) fn level(&self) -> [f32; 2] {
        self.level
    }

    pub(super) fn reading(&self) -> MeterReading {
        MeterReading {
            level: self.level,
            peak_hold: self.peak_hold,
            clipped: self.clipped,
        }
    }

    /// Observe one stereo peak sample plus callback-latched between-poll clip flags.
    /// Returns `true` when any visible field (`level`, `peak_hold`, `clipped`) changed.
    pub(super) fn observe(
        &mut self,
        peaks: [f32; 2],
        latched_clip: [bool; 2],
        decay: f32,
        now: Instant,
    ) -> bool {
        let before = self.reading();
        for ch in 0..2 {
            let sample = sanitize_peak(peaks[ch]);
            let next_level = sample.max(decay_level(self.level[ch], decay));
            self.level[ch] = next_level;

            let next_hold = if sample > 0.0 && sample >= self.peak_hold[ch] {
                self.hold_until[ch] = Some(now + PEAK_HOLD_DURATION);
                sample
            } else if let Some(until) = self.hold_until[ch] {
                if now < until {
                    self.peak_hold[ch].max(next_level)
                } else {
                    self.hold_until[ch] = None;
                    decay_level(self.peak_hold[ch], decay).max(next_level)
                }
            } else {
                decay_level(self.peak_hold[ch], decay).max(next_level)
            };
            self.peak_hold[ch] = if next_hold < scale::FLOOR_AMPLITUDE {
                self.hold_until[ch] = None;
                0.0
            } else {
                next_hold
            };
            self.clipped[ch] = latched_clip[ch] || sample > 1.0;
        }
        before != self.reading()
    }

    /// Advance decay and peak-hold expiry during a missing observation handoff.
    pub(super) fn decay_only(&mut self, decay: f32, now: Instant) -> bool {
        self.observe([0.0; 2], [false; 2], decay, now)
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn decay_keeps_the_new_finite_floor_without_changing_its_rate() {
        assert_eq!(decay_level(0.001, 0.5), 0.0005);
        assert_eq!(
            decay_level(scale::FLOOR_AMPLITUDE, 1.0),
            scale::FLOOR_AMPLITUDE
        );
        assert_eq!(decay_level(scale::FLOOR_AMPLITUDE, 0.5), 0.0);
    }

    #[test]
    fn peak_hold_retains_peak_for_window_then_decays_and_preserves_latched_clip() {
        let now = Instant::now();
        let mut state = StereoMeterState::default();
        let mut updated = None;
        let d0 = decay_factor(&mut updated, now);
        assert!(state.observe([0.2, 0.4], [true, false], d0, now));
        assert_eq!(
            state.reading(),
            MeterReading {
                level: [0.2, 0.4],
                peak_hold: [0.2, 0.4],
                clipped: [true, false],
            }
        );

        // During hold window: level decays, peak_hold stays pinned, clip clears on quiet tick.
        let t1 = now + Duration::from_millis(500);
        let d1 = decay_factor(&mut updated, t1);
        assert!(state.observe([0.0, 0.0], [false, false], d1, t1));
        assert!(state.level()[0] < 0.2);
        assert_eq!(state.reading().peak_hold, [0.2, 0.4]);
        assert_eq!(state.reading().clipped, [false, false]);

        // After hold window expires: peak_hold decays smoothly toward zero.
        let t2 = now + PEAK_HOLD_DURATION + Duration::from_millis(500);
        let d2 = decay_factor(&mut updated, t2);
        assert!(state.observe([0.0, 0.0], [false, false], d2, t2));
        assert!(state.reading().peak_hold[0] < 0.2);
        assert!(state.reading().peak_hold[1] < 0.4);

        // Long silence reaches floor zero for both level and peak_hold.
        let t3 = t2 + Duration::from_secs(10);
        let d3 = decay_factor(&mut updated, t3);
        assert!(state.observe([0.0, 0.0], [false, false], d3, t3));
        assert_eq!(state.reading(), MeterReading::default());
    }
}
