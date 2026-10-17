//! Validated amplitude/decibel units; no device-specific range or floor policy.
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidGain;
impl fmt::Display for InvalidGain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("gain is nonfinite, negative or not representable")
    }
}
impl std::error::Error for InvalidGain {}

/// Finite amplitude decibels (20 log10), distinct from power decibels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decibels(f64);
impl Decibels {
    pub fn new(value: f64) -> Result<Self, InvalidGain> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(InvalidGain)
        }
    }
    pub const fn value(self) -> f64 {
        self.0
    }
    /// Original 10^(dB/20) arithmetic. Overflow refuses; finite underflow is
    /// explicit zero amplitude. Device-specific floors/ranges belong to callers.
    pub fn linear_amplitude(self) -> Result<LinearAmplitude, InvalidGain> {
        LinearAmplitude::new(10.0_f64.powf(self.0 / 20.0))
    }
}

/// Finite nonnegative linear amplitude, with zero representing silence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearAmplitude(f64);
impl LinearAmplitude {
    pub fn new(value: f64) -> Result<Self, InvalidGain> {
        if value.is_finite() && value >= 0.0 {
            Ok(Self(value))
        } else {
            Err(InvalidGain)
        }
    }
    pub const fn value(self) -> f64 {
        self.0
    }
    /// Silence has no finite dB value. The caller chooses display/noise floors;
    /// no arbitrary epsilon is inserted into a musical law here.
    pub fn decibels(self) -> Option<Decibels> {
        if self.0 == 0.0 {
            None
        } else {
            Some(Decibels(20.0 * self.0.log10()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amplitude_anchors_silence_and_numeric_boundaries() -> Result<(), InvalidGain> {
        for (db, expected) in [(0.0, 1.0), (20.0, 10.0), (-20.0, 0.1), (40.0, 100.0)] {
            let amplitude = Decibels::new(db)?.linear_amplitude()?;
            assert!((amplitude.value() - expected).abs() < 1.0e-12);
            assert!((amplitude.decibels().ok_or(InvalidGain)?.value() - db).abs() < 1.0e-12);
        }
        assert_eq!(LinearAmplitude::new(0.0)?.decibels(), None);
        assert_eq!(Decibels::new(-f64::MAX)?.linear_amplitude()?.value(), 0.0);
        assert!(Decibels::new(f64::MAX)?.linear_amplitude().is_err());
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Decibels::new(value).is_err());
            assert!(LinearAmplitude::new(value).is_err());
        }
        assert!(LinearAmplitude::new(-1.0).is_err());
        Ok(())
    }
}
