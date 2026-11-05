//! # Purpose
//! Own control-side repeating-scale math, harmonic analysis, portable tuning presets and validated
//! immutable tuning values. Adapted from the user-owned Freqs scale/analysis implementation.
//!
//! # Dependencies
//! Uses only the standard library; no workspace dependencies.
//!
//! # Does not own
//! Does not perform audio, filesystem I/O, native FFI or process-global tuning publication. Hosts
//! own project history, instruments and MTS-ESP lifecycle.
pub mod analysis;
pub mod preset;
mod scale;
mod table;
pub use scale::{LatticePoint, MAX_DEGREES, MIN_GAP_CENTS, Scale, nearest_lattice};
pub use table::TuningTable;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Invalid {}

/// Reference-tone shape; independent of instrument oscillator settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    Triangle,
    Square,
    Saw,
}

/// Project-saved audition controls. Construction rejects nonfinite/out-of-range data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Audition {
    waveform: Waveform,
    level: f32,
    attack_ms: f32,
    release_ms: f32,
}
impl Default for Audition {
    fn default() -> Self {
        Self {
            waveform: Waveform::Triangle,
            level: 0.25,
            attack_ms: 10.0,
            release_ms: 900.0,
        }
    }
}
impl Audition {
    pub fn new(
        waveform: Waveform,
        level: f32,
        attack_ms: f32,
        release_ms: f32,
    ) -> Result<Self, Invalid> {
        if !level.is_finite()
            || !(0.0..=1.0).contains(&level)
            || !attack_ms.is_finite()
            || !(2.0..=500.0).contains(&attack_ms)
            || !release_ms.is_finite()
            || !(100.0..=3000.0).contains(&release_ms)
        {
            return Err(Invalid("invalid tuning audition settings"));
        }
        Ok(Self {
            waveform,
            level,
            attack_ms,
            release_ms,
        })
    }
    pub fn waveform(self) -> Waveform {
        self.waveform
    }
    pub fn level(self) -> f32 {
        self.level
    }
    pub fn attack_ms(self) -> f32 {
        self.attack_ms
    }
    pub fn release_ms(self) -> f32 {
        self.release_ms
    }
}

/// One canonical project value; MTS ownership and library preset names are not project data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectTuning {
    enabled: bool,
    scale: Scale,
    audition: Audition,
}
impl Default for ProjectTuning {
    fn default() -> Self {
        let scale = Scale::neutral();
        Self {
            enabled: false,
            scale,
            audition: Audition::default(),
        }
    }
}
impl ProjectTuning {
    pub fn new(enabled: bool, scale: Scale, audition: Audition) -> Result<Self, Invalid> {
        if !scale.valid() {
            return Err(Invalid("invalid tuning scale"));
        }
        Ok(Self {
            enabled,
            scale,
            audition,
        })
    }
    pub fn enabled(self) -> bool {
        self.enabled
    }
    /// Opt into or out of this already-validated scale without changing its data.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn scale(self) -> Scale {
        self.scale
    }
    pub fn audition(self) -> Audition {
        self.audition
    }
    /// Effective validated pitch table; audition controls do not affect its identity.
    pub fn table(self) -> Result<TuningTable, Invalid> {
        TuningTable::new(self.frequencies())
    }
    /// Complete single-channel MIDI table prepared on control, never a callback lookup.
    pub fn frequencies(self) -> [f64; 128] {
        if self.enabled {
            self.scale.frequencies()
        } else {
            equal_temperament()
        }
    }
}
/// Exact legacy A440 12-tone formula used when project tuning is disabled.
pub fn equal_temperament() -> [f64; 128] {
    std::array::from_fn(|key| 440.0 * 2.0_f64.powf((key as f64 - 69.0) / 12.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_is_legacy_a440_and_opt_in_maps_adjacent_degrees() {
        let neutral = ProjectTuning::default();
        for (key, hz) in neutral.frequencies().iter().enumerate() {
            assert_eq!(*hz, 440.0 * 2.0_f64.powf((key as f64 - 69.0) / 12.0));
        }
        let scale = Scale::default();
        let tuned = ProjectTuning::new(true, scale, Audition::default()).unwrap();
        let table = tuned.frequencies();
        assert_eq!(table[60], 220.0);
        assert_eq!(table[67], 440.0);
        assert!((table[61] / table[60] - 2.0_f64.powf(200.0 / 1200.0)).abs() < 1e-12);
        assert_eq!(
            ProjectTuning::new(false, scale, Audition::default())
                .unwrap()
                .frequencies(),
            neutral.frequencies()
        );
    }
    #[test]
    fn dense_period_compression_refuses_without_breaking_order() {
        let mut cents = [0.0; MAX_DEGREES];
        cents[1] = 1.0;
        cents[2] = 2.0;
        let mut scale = Scale::new(3, cents, 60, 220.0, 0, 2400.0).unwrap();
        let before = scale;
        scale.set_period(600.0);
        assert_eq!(scale, before);
        assert!(scale.valid());
        for (fifths, thirds) in [(i32::MIN, i32::MAX), (i32::MAX, i32::MIN)] {
            let (n, d) = LatticePoint::new(fifths, thirds).ratio();
            assert!(n >= d && n < 2 * d && d > 0);
        }
    }
    #[test]
    fn rejects_malformed_scale_and_audition_without_clamping() {
        let s = Scale::default();
        for count in [0, 2, 49, usize::MAX] {
            assert!(Scale::new(count, *s.cents(), 60, 220.0, 0, 1200.0).is_err());
        }
        let mut cents = *s.cents();
        cents[47] = f32::NAN;
        assert!(Scale::new(7, cents, 60, 220.0, 0, 1200.0).is_err());
        for hz in [f64::NAN, f64::INFINITY, 0.0, 9.0, 20001.0] {
            assert!(Scale::new(7, *s.cents(), 60, hz, 0, 1200.0).is_err());
        }
        assert!(Scale::new(7, *s.cents(), 128, 220.0, 0, 1200.0).is_err());
        assert!(Audition::new(Waveform::Saw, f32::NAN, 10.0, 900.0).is_err());
        assert!(Audition::new(Waveform::Square, 0.2, 0.0, 900.0).is_err());
    }
}
