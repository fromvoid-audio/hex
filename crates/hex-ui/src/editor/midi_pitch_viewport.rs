//! Finite view-only pitch coordinates for scene geometry and frame identity.
//! Fixed sub-semitone units give scene keys exact equality without float hashing.
use std::ops::Range;

const UNITS_PER_PITCH: u32 = 4096;
const MIDI_UNITS: u32 = 128 * UNITS_PER_PITCH;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct PitchViewport {
    low: u32,
    span: u32,
}
impl PitchViewport {
    pub(super) fn from_rows(low: u8, rows: u8) -> Option<Self> {
        Self::new(f64::from(low), f64::from(rows))
    }
    /// Quantize endpoints to 1/4096 semitone; reject collapsed/outside ranges.
    pub(super) fn new(low: f64, span: f64) -> Option<Self> {
        if !low.is_finite() || !span.is_finite() || low < 0.0 || span <= 0.0 || low + span > 128.0 {
            return None;
        }
        let high = ((low + span) * f64::from(UNITS_PER_PITCH)).round() as u32;
        let low = (low * f64::from(UNITS_PER_PITCH)).round() as u32;
        let span = high.checked_sub(low)?;
        (span > 0 && high <= MIDI_UNITS).then_some(Self { low, span })
    }
    pub(super) fn low(self) -> f64 {
        f64::from(self.low) / f64::from(UNITS_PER_PITCH)
    }
    pub(super) fn span(self) -> f64 {
        f64::from(self.span) / f64::from(UNITS_PER_PITCH)
    }
    pub(super) fn high(self) -> f64 {
        self.low() + self.span()
    }
    /// Integer note cells intersecting the viewport; only for bounded iteration.
    pub(super) fn coverage(self) -> Range<u8> {
        (self.low / UNITS_PER_PITCH) as u8..(self.low + self.span).div_ceil(UNITS_PER_PITCH) as u8
    }
    pub(super) fn contains(self, pitch: u8) -> bool {
        self.coverage().contains(&pitch)
    }
    /// Unclipped normalized note top. Edge notes may be partially visible.
    pub(super) fn note_top(self, pitch: u8) -> Option<f32> {
        self.contains(pitch)
            .then(|| ((self.high() - f64::from(pitch) - 1.0) / self.span()) as f32)
    }
    pub(super) fn row_height_pixels(self, height: f32) -> Option<f32> {
        let pixels = height / self.span() as f32;
        (height.is_finite() && height > 0.0 && pixels.is_finite() && pixels > 0.0).then_some(pixels)
    }
    pub(super) fn pitch_at(self, fraction: f64) -> Option<u8> {
        if !(0.0..1.0).contains(&fraction) {
            return None;
        }
        let coverage = self.coverage();
        // A fraction just below one can subtract back to an integer low by
        // floating-point rounding; in-viewport hits stay in visible coverage.
        Some(
            ((self.high() - fraction * self.span()).ceil() - 1.0)
                .clamp(f64::from(coverage.start), f64::from(coverage.end - 1)) as u8,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewport_rejects_invalid_ranges_and_retains_exact_identity() {
        for (low, span) in [
            (f64::NAN, 12.0),
            (0.0, f64::INFINITY),
            (-1.0, 12.0),
            (0.0, 0.0),
            (127.0, 2.0),
            (128.0, 1.0),
            (0.0, 1e-10),
        ] {
            assert!(PitchViewport::new(low, span).is_none(), "{low}/{span}");
        }
        assert!(PitchViewport::from_rows(0, 128).is_some());
        assert!(PitchViewport::from_rows(127, 1).is_some());
        assert!(PitchViewport::from_rows(0, 0).is_none());
        let wide = PitchViewport::from_rows(0, 128).unwrap();
        for height in [f32::NAN, f32::INFINITY, 0.0, -1.0, f32::from_bits(1)] {
            assert_eq!(wide.row_height_pixels(height), None);
        }
        assert_eq!(
            PitchViewport::new(0.0, 1.0 / 4096.0)
                .unwrap()
                .row_height_pixels(f32::MAX),
            None
        );
        assert_ne!(
            PitchViewport::new(59.25, 12.5),
            PitchViewport::new(59.25 + 1.0 / 4096.0, 12.5)
        );
    }
    #[test]
    fn partial_rows_and_midi_edges_share_forward_inverse_geometry() {
        for (low, span) in [
            (0.0, 12.5),
            (59.25, 12.5),
            (115.5, 12.5),
            (127.75, 0.25),
            (0.25, 127.75),
        ] {
            let view = PitchViewport::new(low, span).unwrap();
            for pitch in view.coverage() {
                let top = f64::from(view.note_top(pitch).unwrap());
                let bottom = top + f64::from(view.row_height_pixels(1.0).unwrap());
                let middle = (top.max(0.0) + bottom.min(1.0)) / 2.0;
                assert_eq!(view.pitch_at(middle), Some(pitch), "{view:?} pitch={pitch}");
            }
            assert_eq!(view.pitch_at(0.0), Some(view.coverage().end - 1));
            assert_eq!(view.pitch_at(1.0 - 1e-7), Some(view.coverage().start));
            assert_eq!(view.pitch_at(-0.01), None);
            assert_eq!(view.pitch_at(1.0), None);
            assert_eq!(view.pitch_at(f64::NAN), None);
        }
    }
    #[test]
    fn bottom_fraction_rounding_stays_inside_visible_coverage() {
        let near_one = f64::from_bits(1.0_f64.to_bits() - 1);
        for (low, rows) in [(48, 24), (127, 1), (0, 128)] {
            let view = PitchViewport::from_rows(low, rows).unwrap();
            assert_eq!(view.pitch_at(near_one), Some(low));
            assert_eq!(view.pitch_at(1.0), None);
        }
    }
    #[test]
    fn integer_inputs_preserve_row_and_hit_mapping() {
        for low in 0..128_u8 {
            for rows in 1..=128 - low {
                let view = PitchViewport::from_rows(low, rows).unwrap();
                assert_eq!(view.coverage(), low..low + rows);
                assert_eq!(view.row_height_pixels(240.0), Some(240.0 / f32::from(rows)));
                for row in 0..rows {
                    let pitch = low + rows - 1 - row;
                    assert_eq!(
                        view.note_top(pitch).unwrap(),
                        f32::from(row) / f32::from(rows)
                    );
                    assert_eq!(
                        view.pitch_at((f64::from(row) + 0.5) / f64::from(rows)),
                        Some(pitch)
                    );
                }
            }
        }
    }
}
