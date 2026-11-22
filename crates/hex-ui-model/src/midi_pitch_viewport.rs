//! Finite view-only pitch coordinates for scene geometry and frame identity.
//! Fixed sub-semitone units give scene keys exact equality without float hashing.
use std::ops::Range;

const UNITS_PER_PITCH: u32 = 4096;
const MIDI_UNITS: u32 = 128 * UNITS_PER_PITCH;
const DEFAULT_ROW_HEIGHT: f64 = 16.0;
const MIN_ROW_HEIGHT: f64 = 10.0;
const MAX_ROW_HEIGHT: f64 = 48.0;
const MIN_VISIBLE_ROWS: f64 = 3.0;
const ZOOM_DOUBLING_PIXELS: f64 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PitchViewport {
    low: u32,
    span: u32,
}
impl PitchViewport {
    pub const DEFAULT: Self = Self {
        low: 48 * UNITS_PER_PITCH,
        span: 24 * UNITS_PER_PITCH,
    };

    /// First usable view of a clip, centered on the default pitch range.
    pub fn default_for_height(height: f32) -> Self {
        Self::DEFAULT
            .resize_around(height, f64::from(height) / DEFAULT_ROW_HEIGHT, 0.5)
            .unwrap_or(Self::DEFAULT)
    }

    /// Apply the manual bounds after the grid changes size. Note fit deliberately
    /// owns its wider view until manual zoom resumes.
    pub fn constrain_to_height(self, height: f32) -> Self {
        self.resize_around(height, self.span(), 0.5).unwrap_or(self)
    }

    /// Continuous zoom anchored in the note grid. Positive input increases
    /// the height of each row. Gesture callers pass displacement from capture.
    pub fn zoom_at(self, height: f32, fraction: f64, delta: f32) -> Option<Self> {
        if !delta.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return None;
        }
        if delta == 0.0 {
            return Some(self);
        }
        let log_scale = (-f64::from(delta) / ZOOM_DOUBLING_PIXELS).clamp(-16.0, 16.0);
        self.resize_around(height, self.span() * log_scale.exp2(), fraction)
    }

    fn resize_around(self, height: f32, requested: f64, fraction: f64) -> Option<Self> {
        if !height.is_finite() || height <= 0.0 {
            return None;
        }
        let height = f64::from(height);
        let min_span = (height / MAX_ROW_HEIGHT).clamp(MIN_VISIBLE_ROWS, 128.0);
        let max_span = (height / MIN_ROW_HEIGHT).clamp(min_span, 128.0);
        let units = f64::from(UNITS_PER_PITCH);
        let span = (requested.clamp(min_span, max_span) * units).round() as u32;
        let anchor = f64::from(self.low) + (1.0 - fraction) * f64::from(self.span);
        let low = (anchor - (1.0 - fraction) * f64::from(span))
            .round()
            .clamp(0.0, f64::from(MIDI_UNITS - span)) as u32;
        Some(Self { low, span })
    }

    pub fn from_rows(low: u8, rows: u8) -> Option<Self> {
        Self::new(f64::from(low), f64::from(rows))
    }
    /// Quantize endpoints to 1/4096 semitone; reject collapsed/outside ranges.
    pub fn new(low: f64, span: f64) -> Option<Self> {
        if !low.is_finite() || !span.is_finite() || low < 0.0 || span <= 0.0 || low + span > 128.0 {
            return None;
        }
        let high = ((low + span) * f64::from(UNITS_PER_PITCH)).round() as u32;
        let low = (low * f64::from(UNITS_PER_PITCH)).round() as u32;
        let span = high.checked_sub(low)?;
        (span > 0 && high <= MIDI_UNITS).then_some(Self { low, span })
    }
    pub fn low(self) -> f64 {
        f64::from(self.low) / f64::from(UNITS_PER_PITCH)
    }
    pub fn span(self) -> f64 {
        f64::from(self.span) / f64::from(UNITS_PER_PITCH)
    }
    pub fn high(self) -> f64 {
        self.low() + self.span()
    }
    /// Translate by whole semitones while retaining fractional endpoints.
    /// Return whether the MIDI extent clamped the requested move.
    pub fn scroll_rows(self, rows: i16) -> (Self, bool) {
        let target = i64::from(self.low) + i64::from(rows) * i64::from(UNITS_PER_PITCH);
        let low = target.clamp(0, i64::from(MIDI_UNITS - self.span));
        (
            Self {
                low: low as u32,
                ..self
            },
            low != target,
        )
    }
    /// Integer note cells intersecting the viewport; only for bounded iteration.
    pub fn coverage(self) -> Range<u8> {
        (self.low / UNITS_PER_PITCH) as u8..(self.low + self.span).div_ceil(UNITS_PER_PITCH) as u8
    }
    /// Lowest covered integer pitch, used by piano-roll UI tests.
    pub fn first_test_pitch(self) -> u8 {
        self.coverage().start
    }
    /// Number of covered integer pitch rows, used by piano-roll UI tests.
    pub fn test_row_count(self) -> u8 {
        self.coverage().len() as u8
    }
    pub fn contains(self, pitch: u8) -> bool {
        self.coverage().contains(&pitch)
    }
    /// Unclipped normalized note top. Edge notes may be partially visible.
    pub fn note_top(self, pitch: u8) -> Option<f32> {
        self.contains(pitch).then(|| {
            let top = i64::from(self.low) + i64::from(self.span)
                - (i64::from(pitch) + 1) * i64::from(UNITS_PER_PITCH);
            top as f32 / self.span as f32
        })
    }
    pub fn row_height_pixels(self, height: f32) -> Option<f32> {
        let pixels = height / self.span() as f32;
        (height.is_finite() && height > 0.0 && pixels.is_finite() && pixels > 0.0).then_some(pixels)
    }
    /// Visible normalized row bounds, including clipped edge cells.
    pub fn visible_row(self, pitch: u8) -> Option<(f32, f32)> {
        let top = self.note_top(pitch)?;
        Some((top.max(0.0), (top + 1.0 / self.span() as f32).min(1.0)))
    }
    pub fn pitch_at(self, fraction: f64) -> Option<u8> {
        if !(0.0..1.0).contains(&fraction) {
            return None;
        }
        let coverage = self.coverage();
        Some(
            self.floor_pitch(fraction)
                .clamp(f64::from(coverage.start), f64::from(coverage.end - 1)) as u8,
        )
    }
    /// Hit only the half-open visible canvas.
    pub fn pitch_at_y(self, y: f32, height: f32) -> Option<u8> {
        if !height.is_finite() || height <= 0.0 || !(0.0..height).contains(&y) {
            return None;
        }
        self.pitch_at(f64::from(y) / f64::from(height))
    }
    /// A held drag may continue outside the canvas up to the MIDI edges.
    pub fn drag_pitch_at(self, fraction: f64) -> u8 {
        self.pitch_at(fraction)
            .unwrap_or_else(|| self.floor_pitch(fraction).clamp(0.0, 127.0) as u8)
    }
    fn floor_pitch(self, fraction: f64) -> f64 {
        (self.high() - fraction * self.span()).ceil() - 1.0
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
                let (top, bottom) = view.visible_row(pitch).unwrap();
                let y = (top + bottom) * 120.0;
                assert_eq!(
                    view.pitch_at_y(y, 240.0),
                    Some(pitch),
                    "{view:?} pitch={pitch}"
                );
                assert_eq!(view.drag_pitch_at(f64::from(y) / 240.0), pitch);
            }
            assert_eq!(view.pitch_at(0.0), Some(view.coverage().end - 1));
            assert_eq!(view.pitch_at(1.0 - 1e-7), Some(view.coverage().start));
            assert_eq!(view.pitch_at(-0.01), None);
            assert_eq!(view.pitch_at(1.0), None);
            assert_eq!(view.pitch_at(f64::NAN), None);
            assert_eq!(view.pitch_at_y(240.0, 240.0), None);
            assert_eq!(view.pitch_at_y(f32::NAN, 240.0), None);
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
    fn whole_row_pan_preserves_fractional_span_until_midi_edges() {
        let view = PitchViewport::new(59.25, 12.5).unwrap();
        assert_eq!(
            view.scroll_rows(6),
            (PitchViewport::new(65.25, 12.5).unwrap(), false)
        );
        assert_eq!(
            view.scroll_rows(-128),
            (PitchViewport::new(0.0, 12.5).unwrap(), true)
        );
        assert_eq!(
            view.scroll_rows(128),
            (PitchViewport::new(115.5, 12.5).unwrap(), true)
        );
        assert_eq!(
            PitchViewport::DEFAULT.scroll_rows(-12).0,
            PitchViewport::from_rows(36, 24).unwrap()
        );
    }
    #[test]
    fn continuous_zoom_anchors_fractional_pitch_and_scales_with_grid_height() {
        for height in [56.0, 155.0, 420.0] {
            let view = PitchViewport::default_for_height(height);
            assert!((f64::from(height) / view.span() - 16.0).abs() < 0.01);
            for fraction in [0.0, 0.3, 0.5, 1.0] {
                let zoomed = view.zoom_at(height, fraction, 0.5).unwrap();
                assert!((0.0..1.0).contains(&(view.span() - zoomed.span())));
                let anchor = |pitch: PitchViewport| pitch.low() + (1.0 - fraction) * pitch.span();
                assert!((anchor(view) - anchor(zoomed)).abs() < 1.0 / 4096.0);
            }
            assert_eq!(view.zoom_at(height, 0.5, 0.0), Some(view));
            let closest = view.zoom_at(height, 0.5, f32::MAX).unwrap();
            let widest = view.zoom_at(height, 0.5, -f32::MAX).unwrap();
            assert!(closest.span() >= 3.0);
            assert!(f64::from(height) / closest.span() <= 48.01);
            assert!((f64::from(height) / widest.span() - 10.0).abs() < 0.01);
        }
        let full = PitchViewport::from_rows(0, 128).unwrap();
        assert_eq!(full.zoom_at(155.0, 0.5, 0.0), Some(full));
        assert_eq!(full.zoom_at(155.0, 0.5, f32::NAN), None);
        assert_eq!(full.zoom_at(155.0, -0.1, 2.0), None);
        assert_eq!(full.zoom_at(f32::INFINITY, 0.5, 2.0), None);
        let clamped = full.constrain_to_height(90.0);
        assert_eq!(clamped.span(), 9.0);
        assert_eq!(clamped.low() + clamped.span() / 2.0, 64.0);
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
