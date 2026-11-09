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
        })
    }
    pub const fn occurrence_id(self) -> u64 {
        self.occurrence_id
    }
    /// Select another slice of the original ramp without re-quantizing it.
    pub fn with_offset_frames(
        self,
        offset_frames: u64,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        Self::new(
            self.occurrence_id,
            self.start_gain,
            self.end_gain,
            self.span_frames,
            offset_frames,
        )
    }
    /// Evaluate a relative sample offset. At/after the original exclusive end,
    /// hold the endpoint. Pair admission still requires the complete segment to
    /// lie inside the ramp; this endpoint policy is useful for control queries.
    pub fn value_at_offset(self, offset: u64) -> f64 {
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
    pub(crate) fn covers_frames(self, frames: usize) -> bool {
        u64::try_from(frames)
            .ok()
            .and_then(|frames| self.offset_frames.checked_add(frames))
            .is_some_and(|end| end <= self.span_frames)
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
