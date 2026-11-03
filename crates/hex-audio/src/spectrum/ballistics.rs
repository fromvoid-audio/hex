//! Display-only per-point ballistics and peak hold.
//!
//! Every update uses the actual elapsed time, so the same input history yields
//! the same displayed levels at the same instants whatever the analysis or
//! refresh cadence. State is fixed-size and updates never allocate.

use super::{FLOOR_DBFS, SPECTRUM_BANDS, SpectrumFrame};
use std::time::Duration;

/// Time constants for [`SpectrumBallistics`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BallisticsSettings {
    /// Exponential rise time constant, in seconds.
    pub attack_seconds: f32,
    /// Exponential fall time constant toward a lower level, in seconds.
    pub release_seconds: f32,
    /// Time a new peak stays fixed before it starts to fall, in seconds.
    pub peak_hold_seconds: f32,
    /// Linear peak fall rate after the hold, in dB per second.
    pub peak_decay_db_per_second: f32,
}

impl BallisticsSettings {
    pub const DEFAULT: Self = Self {
        attack_seconds: 0.010,
        release_seconds: 0.250,
        peak_hold_seconds: 1.0,
        peak_decay_db_per_second: 6.0,
    };
}

impl Default for BallisticsSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Smoothed level and peak-hold state for one analyzed signal.
#[derive(Clone, Debug)]
pub struct SpectrumBallistics {
    settings: BallisticsSettings,
    primed: bool,
    level: [f32; SPECTRUM_BANDS],
    peak: [f32; SPECTRUM_BANDS],
    hold_left: [f32; SPECTRUM_BANDS],
}

impl Default for SpectrumBallistics {
    fn default() -> Self {
        Self::new(BallisticsSettings::DEFAULT)
    }
}

impl SpectrumBallistics {
    /// Non-finite or negative settings fall back to the defaults field by field.
    pub fn new(settings: BallisticsSettings) -> Self {
        let valid = |value: f32, fallback: f32| {
            if value.is_finite() && value >= 0.0 {
                value
            } else {
                fallback
            }
        };
        let defaults = BallisticsSettings::DEFAULT;
        Self {
            settings: BallisticsSettings {
                attack_seconds: valid(settings.attack_seconds, defaults.attack_seconds),
                release_seconds: valid(settings.release_seconds, defaults.release_seconds),
                peak_hold_seconds: valid(settings.peak_hold_seconds, defaults.peak_hold_seconds),
                peak_decay_db_per_second: valid(
                    settings.peak_decay_db_per_second,
                    defaults.peak_decay_db_per_second,
                ),
            },
            primed: false,
            level: [FLOOR_DBFS; SPECTRUM_BANDS],
            peak: [FLOOR_DBFS; SPECTRUM_BANDS],
            hold_left: [0.0; SPECTRUM_BANDS],
        }
    }

    /// Forget history; the next update shows its input directly.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    /// Replace `frame.bins_dbfs` with the smoothed level and `frame.peak_dbfs`
    /// with the held peak, `elapsed` after the previous update.
    pub fn apply(&mut self, frame: &mut SpectrumFrame, elapsed: Duration) {
        if !self.primed {
            self.level = frame.bins_dbfs;
            self.peak = frame.bins_dbfs;
            self.hold_left = [self.settings.peak_hold_seconds; SPECTRUM_BANDS];
            self.primed = true;
            frame.peak_dbfs = self.peak;
            return;
        }
        let dt = elapsed.as_secs_f32();
        let coefficient = |seconds: f32| {
            if seconds <= 0.0 {
                1.0
            } else {
                1.0 - (-dt / seconds).exp()
            }
        };
        let attack = coefficient(self.settings.attack_seconds);
        let release = coefficient(self.settings.release_seconds);
        for point in 0..SPECTRUM_BANDS {
            let target = frame.bins_dbfs[point];
            let level = &mut self.level[point];
            let step = if target > *level { attack } else { release };
            *level = (*level + (target - *level) * step).max(FLOOR_DBFS);

            let peak = &mut self.peak[point];
            let hold = &mut self.hold_left[point];
            if *level >= *peak {
                *peak = *level;
                *hold = self.settings.peak_hold_seconds;
            } else {
                let falling = (dt - *hold).max(0.0);
                *hold = (*hold - dt).max(0.0);
                *peak = (*peak - falling * self.settings.peak_decay_db_per_second)
                    .max(*level)
                    .max(FLOOR_DBFS);
            }
        }
        frame.bins_dbfs = self.level;
        frame.peak_dbfs = self.peak;
    }
}
