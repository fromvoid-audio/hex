//! Borrowed per-occurrence output gain, independent of velocity and voice history.
//! A slice retains the original ramp coordinates so arbitrary block partitions
//! evaluate the same samples without rounding new intermediate endpoints.
use crate::NativeInstrumentRuntimeError;

pub const INSTRUMENT_GAIN_UNITY: f64 = 1.0;
/// Covers the product of a 0..4 source-beat curve and the validated
/// -60..+24 dB clip scalar (less than 16), with a fixed finite boundary.
pub const MAX_INSTRUMENT_OCCURRENCE_GAIN: f64 = 64.0;

/// One frame-linear ramp on a stable runtime occurrence. Endpoints are unsigned
/// linear gain; `span_frames` ends at the exclusive ramp boundary. Prepared
/// tempo/cue boundaries may have fractional gains; never re-quantize them.
/// `offset_frames` selects the beginning of the borrowed processing segment.
/// Missing occurrences in a gain batch retain unity. This value owns no state
/// and never changes a voice's velocity, cursor, gate or age.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeInstrumentOccurrenceGain {
    occurrence_id: u64,
    start_gain: f64,
    end_gain: f64,
    span_frames: u64,
    offset_frames: u64,
    attenuation: Option<AttenuationRamp>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AttenuationRamp {
    start: f64,
    end: f64,
    span: u64,
    offset: u64,
}
impl NativeInstrumentOccurrenceGain {
    pub fn new(
        occurrence_id: u64,
        start_gain: f64,
        end_gain: f64,
        span_frames: u64,
        offset_frames: u64,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        if occurrence_id == 0
            || !start_gain.is_finite()
            || !end_gain.is_finite()
            || !(0.0..=MAX_INSTRUMENT_OCCURRENCE_GAIN).contains(&start_gain)
            || !(0.0..=MAX_INSTRUMENT_OCCURRENCE_GAIN).contains(&end_gain)
            || span_frames == 0
            || offset_frames > span_frames
        {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        Ok(Self {
            occurrence_id,
            start_gain,
            end_gain,
            span_frames,
            offset_frames,
            attenuation: None,
        })
    }
    /// Multiply this gain by an independently timed 0..1 linear attenuation.
    /// Both ramps retain their original coordinates and must cover each slice.
    pub fn with_attenuation(
        mut self,
        start: f64,
        end: f64,
        span: u64,
        offset: u64,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        if self.attenuation.is_some()
            || !start.is_finite()
            || !end.is_finite()
            || !(0.0..=1.0).contains(&start)
            || !(0.0..=1.0).contains(&end)
            || span == 0
            || offset > span
        {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        self.attenuation = Some(AttenuationRamp {
            start,
            end,
            span,
            offset,
        });
        Ok(self)
    }
    pub const fn occurrence_id(self) -> u64 {
        self.occurrence_id
    }
    /// Select another slice of the original ramp without re-quantizing it.
    pub fn with_offset_frames(
        self,
        offset_frames: u64,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        let mut shifted = Self::new(
            self.occurrence_id,
            self.start_gain,
            self.end_gain,
            self.span_frames,
            offset_frames,
        )?;
        if let Some(mut attenuation) = self.attenuation {
            attenuation.offset = if offset_frames >= self.offset_frames {
                attenuation
                    .offset
                    .checked_add(offset_frames - self.offset_frames)
            } else {
                attenuation
                    .offset
                    .checked_sub(self.offset_frames - offset_frames)
            }
            .filter(|offset| *offset <= attenuation.span)
            .ok_or(NativeInstrumentRuntimeError::InvalidOccurrenceGain)?;
            shifted.attenuation = Some(attenuation);
        }
        Ok(shifted)
    }
    /// Evaluate a relative sample offset. At/after the original exclusive end,
    /// hold the endpoint. Pair admission still requires the complete segment to
    /// lie inside the ramp; this endpoint policy is useful for control queries.
    pub fn value_at_offset(self, offset: u64) -> f64 {
        let position = self
            .offset_frames
            .saturating_add(offset)
            .min(self.span_frames);
        let base = if position == self.span_frames {
            self.end_gain
        } else if position == 0 || self.start_gain == self.end_gain {
            self.start_gain
        } else {
            let fraction = position as f64 / self.span_frames as f64;
            self.start_gain + (self.end_gain - self.start_gain) * fraction
        };
        match self.attenuation {
            Some(ramp) => {
                let position = ramp.offset.saturating_add(offset).min(ramp.span);
                let factor = if position == ramp.span {
                    ramp.end
                } else if position == 0 || ramp.start == ramp.end {
                    ramp.start
                } else {
                    ramp.start + (ramp.end - ramp.start) * (position as f64 / ramp.span as f64)
                };
                base * factor
            }
            None => base,
        }
    }
    pub(crate) fn covers_frames(self, frames: usize) -> bool {
        u64::try_from(frames).ok().is_some_and(|frames| {
            self.offset_frames
                .checked_add(frames)
                .is_some_and(|end| end <= self.span_frames)
                && self.attenuation.is_none_or(|ramp| {
                    ramp.offset
                        .checked_add(frames)
                        .is_some_and(|end| end <= ramp.span)
                })
        })
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
    fn independently_timed_attenuation_preserves_product_across_slices() {
        let gain = NativeInstrumentOccurrenceGain::new(9, 0.5, 2.0, 12, 3)
            .unwrap()
            .with_attenuation(1.0, 0.0, 8, 1)
            .unwrap();
        for index in 0..7 {
            let clip = 0.5 + 1.5 * ((3 + index) as f64 / 12.0);
            let mute = 1.0 - (1 + index) as f64 / 8.0;
            let whole = gain.value_at_offset(index);
            let slice = gain.with_offset_frames(3 + index).unwrap();
            assert_eq!(whole, slice.value_at_offset(0));
            assert!((whole - clip * mute).abs() < 1e-12);
            assert!(slice.covers_frames((7 - index) as usize));
        }
        assert_eq!(gain.value_at_offset(7), 0.0);
        assert!(!gain.covers_frames(8));
        assert!(gain.with_offset_frames(2).unwrap().covers_frames(8));
        assert!(gain.with_offset_frames(1).is_err());
        assert!(gain.with_offset_frames(11).is_err());
    }

    #[test]
    fn attenuation_rejects_invalid_coordinates_and_values() {
        let gain = NativeInstrumentOccurrenceGain::new(1, 64.0, 64.0, u64::MAX, 0).unwrap();
        for (start, end, span, offset) in [
            (-0.01, 1.0, 1, 0),
            (1.0, 1.01, 1, 0),
            (f64::NAN, 0.0, 1, 0),
            (1.0, f64::INFINITY, 1, 0),
            (1.0, 0.0, 0, 0),
            (1.0, 0.0, 1, 2),
        ] {
            assert_eq!(
                gain.with_attenuation(start, end, span, offset),
                Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain)
            );
        }
        let silent = gain.with_attenuation(0.0, 0.0, u64::MAX, u64::MAX).unwrap();
        assert_eq!(silent.value_at_offset(u64::MAX), 0.0);
        assert!(!silent.covers_frames(1));
        assert!(silent.with_offset_frames(1).is_err());
        assert!(silent.with_attenuation(1.0, 1.0, 1, 0).is_err());
        let unity = gain.with_attenuation(1.0, 1.0, 3, 0).unwrap();
        assert_eq!(unity.value_at_offset(2), 64.0);
        assert!(unity.with_offset_frames(u64::MAX).is_err());
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
