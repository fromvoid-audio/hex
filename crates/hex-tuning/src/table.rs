//! Validated effective tuning, separate from editor and audition state.
use crate::Invalid;

/// One immutable single-channel MIDI frequency table. Finite positive frequencies
/// are authored data; each instrument admits its own sample-rate/Nyquist domain.
#[derive(Clone, Copy, Debug)]
pub struct TuningTable {
    frequencies: [f64; 128],
}
impl TuningTable {
    pub fn new(frequencies: [f64; 128]) -> Result<Self, Invalid> {
        if frequencies.iter().any(|hz| !hz.is_finite() || *hz <= 0.0) {
            return Err(Invalid("tuning frequencies must be finite and positive"));
        }
        Ok(Self { frequencies })
    }
    pub fn frequencies(&self) -> &[f64; 128] {
        &self.frequencies
    }
    pub fn frequency(&self, pitch: u8) -> Result<f64, Invalid> {
        self.frequencies
            .get(usize::from(pitch))
            .copied()
            .ok_or(Invalid("tuning key must be 0..127"))
    }
    pub fn is_neutral(&self) -> bool {
        self == &Self::default()
    }
}
impl Default for TuningTable {
    fn default() -> Self {
        Self {
            frequencies: crate::equal_temperament(),
        }
    }
}
impl PartialEq for TuningTable {
    fn eq(&self, other: &Self) -> bool {
        self.frequencies
            .iter()
            .zip(&other.frequencies)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    }
}
impl Eq for TuningTable {}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validated_exact_identity_excludes_audition_and_preserves_neutral() {
        let neutral = TuningTable::default();
        assert!(neutral.is_neutral());
        assert_eq!(neutral.frequencies(), &crate::equal_temperament());
        let mut values = *neutral.frequencies();
        values[69] = f64::from_bits(values[69].to_bits() + 1);
        let changed = TuningTable::new(values).unwrap();
        assert_ne!(neutral, changed);
        assert!(!changed.is_neutral());
        assert!(changed.frequency(128).is_err());
        for invalid in [f64::NAN, f64::INFINITY, 0.0, -0.0, -1.0] {
            values[127] = invalid;
            assert!(TuningTable::new(values).is_err());
        }
        assert_eq!(std::mem::size_of::<TuningTable>(), 1024);
    }
}
