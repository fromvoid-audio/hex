//! Borrowed per-occurrence output gain, independent of velocity and voice history.
//! A slice retains each original ramp's coordinates so arbitrary block partitions
//! evaluate products without rounding new intermediate endpoints.
use crate::NativeInstrumentRuntimeError;

pub const INSTRUMENT_GAIN_UNITY: f64 = 1.0;
/// Maximum for each factor: covers a 0..4 source-beat curve multiplied by
/// the validated -60..+24 dB clip scalar (less than 16).
pub const MAX_INSTRUMENT_OCCURRENCE_GAIN: f64 = 64.0;
/// Fixed callback work and storage limit for one occurrence's gain product.
pub const MAX_INSTRUMENT_GAIN_FACTORS: usize = 8;
/// Upper bound of eight valid factors, also used for an outgoing held product.
pub const MAX_INSTRUMENT_GAIN_PRODUCT: f64 = 281_474_976_710_656.0;

/// A frame-linear factor with its original endpoints and current slice offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeInstrumentGainFactor {
    start_gain: f64,
    end_gain: f64,
    span_frames: u64,
    offset_frames: u64,
}
impl NativeInstrumentGainFactor {
    pub fn new(
        start_gain: f64,
        end_gain: f64,
        span_frames: u64,
        offset_frames: u64,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        if !start_gain.is_finite()
            || !end_gain.is_finite()
            || !(0.0..=MAX_INSTRUMENT_OCCURRENCE_GAIN).contains(&start_gain)
            || !(0.0..=MAX_INSTRUMENT_OCCURRENCE_GAIN).contains(&end_gain)
            || span_frames == 0
            || offset_frames > span_frames
        {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        Ok(Self {
            start_gain,
            end_gain,
            span_frames,
            offset_frames,
        })
    }
    fn value_at_offset(&self, offset: u64) -> f64 {
        let position = self
            .offset_frames
            .saturating_add(offset)
            .min(self.span_frames);
        if position == self.span_frames {
            return self.end_gain;
        }
        if position == 0 || self.start_gain == self.end_gain {
            return self.start_gain;
        }
        let fraction = position as f64 / self.span_frames as f64;
        self.start_gain + (self.end_gain - self.start_gain) * fraction
    }
    fn covers_frames(&self, frames: usize) -> bool {
        u64::try_from(frames)
            .ok()
            .and_then(|frames| self.offset_frames.checked_add(frames))
            .is_some_and(|end| end <= self.span_frames)
    }
}

/// A bounded product of frame-linear factors on one stable occurrence.
/// Missing occurrences retain unity. Factors own no state or resources and do
/// not alter velocity, cursor, gate or age. Each factor remains within the
/// validated 0..64 range, so the eight-factor product is finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeInstrumentOccurrenceGain {
    occurrence_id: u64,
    factors: [NativeInstrumentGainFactor; MAX_INSTRUMENT_GAIN_FACTORS],
    len: u8,
}
impl NativeInstrumentOccurrenceGain {
    pub fn new(
        occurrence_id: u64,
        start_gain: f64,
        end_gain: f64,
        span_frames: u64,
        offset_frames: u64,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        let factor =
            NativeInstrumentGainFactor::new(start_gain, end_gain, span_frames, offset_frames)?;
        Self::product(occurrence_id, &[factor])
    }
    pub fn product(
        occurrence_id: u64,
        factors: &[NativeInstrumentGainFactor],
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        if occurrence_id == 0 || factors.is_empty() || factors.len() > MAX_INSTRUMENT_GAIN_FACTORS {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        let unity = NativeInstrumentGainFactor {
            start_gain: 1.0,
            end_gain: 1.0,
            span_frames: 1,
            offset_frames: 0,
        };
        let mut stored = [unity; MAX_INSTRUMENT_GAIN_FACTORS];
        stored[..factors.len()].copy_from_slice(factors);
        Ok(Self {
            occurrence_id,
            factors: stored,
            len: factors.len() as u8,
        })
    }
    /// Hold an already evaluated product after its source has retired. This
    /// borrows no source factors and cannot resume a retired gain law.
    pub fn held(
        occurrence_id: u64,
        gain: f64,
        span_frames: u64,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        if occurrence_id == 0
            || !gain.is_finite()
            || !(0.0..=MAX_INSTRUMENT_GAIN_PRODUCT).contains(&gain)
            || span_frames == 0
        {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        let unity = NativeInstrumentGainFactor {
            start_gain: 1.0,
            end_gain: 1.0,
            span_frames: 1,
            offset_frames: 0,
        };
        let mut factors = [unity; MAX_INSTRUMENT_GAIN_FACTORS];
        factors[0] = NativeInstrumentGainFactor {
            start_gain: gain,
            end_gain: gain,
            span_frames,
            offset_frames: 0,
        };
        Ok(Self {
            occurrence_id,
            factors,
            len: 1,
        })
    }
    pub const fn occurrence_id(&self) -> u64 {
        self.occurrence_id
    }
    /// Select a slice relative to the first factor, shifting every other factor
    /// by the same amount without re-quantizing its endpoints.
    pub fn with_offset_frames(
        mut self,
        offset_frames: u64,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        let original = self.factors[0].offset_frames;
        for factor in &mut self.factors[..usize::from(self.len)] {
            let shifted = if offset_frames >= original {
                factor.offset_frames.checked_add(offset_frames - original)
            } else {
                factor.offset_frames.checked_sub(original - offset_frames)
            }
            .filter(|offset| *offset <= factor.span_frames)
            .ok_or(NativeInstrumentRuntimeError::InvalidOccurrenceGain)?;
            factor.offset_frames = shifted;
        }
        Ok(self)
    }
    /// Evaluate each original factor at this relative sample and multiply.
    /// At or after an exclusive end the corresponding endpoint is held.
    pub fn value_at_offset(&self, offset: u64) -> f64 {
        self.factors[..usize::from(self.len)]
            .iter()
            .fold(1.0, |gain, factor| gain * factor.value_at_offset(offset))
    }
    pub(crate) fn covers_frames(&self, frames: usize) -> bool {
        self.factors[..usize::from(self.len)]
            .iter()
            .all(|factor| factor.covers_frames(frames))
    }
}

pub(crate) fn validate_occurrence_gains(
    gains: &[NativeInstrumentOccurrenceGain],
    frames: usize,
    maximum: usize,
) -> Result<(), NativeInstrumentRuntimeError> {
    if gains.len() > maximum {
        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
    }
    let mut previous = 0;
    for gain in gains {
        if gain.occurrence_id() <= previous || !gain.covers_frames(frames) {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        previous = gain.occurrence_id();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn products_keep_each_original_ramp_across_partitions() {
        let first = NativeInstrumentGainFactor::new(0.0, 1.0, 8, 0).unwrap();
        let second = NativeInstrumentGainFactor::new(1.0, 0.0, 12, 2).unwrap();
        let product = NativeInstrumentOccurrenceGain::product(7, &[first, second]).unwrap();
        assert!(product.covers_frames(8));
        for offset in 0..8 {
            let expected = (offset as f64 / 8.0) * (1.0 - (offset + 2) as f64 / 12.0);
            assert!((product.value_at_offset(offset) - expected).abs() < 1e-15);
        }
        let split = product.with_offset_frames(3).unwrap();
        assert!(split.covers_frames(5));
        for offset in 0..5 {
            assert_eq!(
                split.value_at_offset(offset),
                product.value_at_offset(offset + 3)
            );
        }
        assert!(!split.covers_frames(6));
        assert!(NativeInstrumentOccurrenceGain::product(7, &[]).is_err());
        assert!(NativeInstrumentOccurrenceGain::product(0, &[first]).is_err());
        assert!(
            NativeInstrumentOccurrenceGain::product(7, &[first; MAX_INSTRUMENT_GAIN_FACTORS + 1])
                .is_err()
        );
        let max = NativeInstrumentGainFactor::new(64.0, 64.0, 8, 0).unwrap();
        let bounded =
            NativeInstrumentOccurrenceGain::product(7, &[max; MAX_INSTRUMENT_GAIN_FACTORS])
                .unwrap();
        assert_eq!(bounded.value_at_offset(0), 64.0_f64.powi(8));
        let held = NativeInstrumentOccurrenceGain::held(7, bounded.value_at_offset(0), 8).unwrap();
        assert_eq!(held.value_at_offset(7), bounded.value_at_offset(7));
        assert!(
            NativeInstrumentOccurrenceGain::held(7, MAX_INSTRUMENT_GAIN_PRODUCT * 2.0, 8).is_err()
        );
        let later = NativeInstrumentGainFactor::new(0.0, 1.0, 8, 4).unwrap();
        let near_start = NativeInstrumentGainFactor::new(0.0, 1.0, 8, 1).unwrap();
        let shifted = NativeInstrumentOccurrenceGain::product(7, &[later, near_start]).unwrap();
        assert!(shifted.with_offset_frames(2).is_err());
    }
    #[test]
    fn invalid_ramp_fields_and_slice_overflow_are_refused() {
        for args in [
            (0, 0.0, 0.0, 1, 0),
            (1, MAX_INSTRUMENT_OCCURRENCE_GAIN + 1.0, 0.0, 1, 0),
            (1, 0.0, MAX_INSTRUMENT_OCCURRENCE_GAIN + 1.0, 1, 0),
            (1, -0.01, 1.0, 1, 0),
            (1, 0.0, -0.01, 1, 0),
            (1, f64::NAN, 1.0, 1, 0),
            (1, 0.0, f64::INFINITY, 1, 0),
            (1, f64::NEG_INFINITY, 1.0, 1, 0),
            (1, 0.0, 0.0, 0, 0),
            (1, 0.0, 0.0, 1, 2),
        ] {
            assert_eq!(
                NativeInstrumentOccurrenceGain::new(args.0, args.1, args.2, args.3, args.4),
                Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain)
            );
        }
        let ramp =
            NativeInstrumentOccurrenceGain::new(1, 0.0, INSTRUMENT_GAIN_UNITY, u64::MAX, u64::MAX)
                .unwrap();
        assert!(!ramp.covers_frames(1));
        assert_eq!(ramp.value_at_offset(u64::MAX), 1.0);
        assert!(ramp.with_offset_frames(0).unwrap().covers_frames(7));
        let tiny = NativeInstrumentOccurrenceGain::new(1, 4.0, f64::MIN_POSITIVE, 1, 0).unwrap();
        assert_eq!(tiny.value_at_offset(0), 4.0);
        assert_eq!(tiny.value_at_offset(1), f64::MIN_POSITIVE);
    }
}
