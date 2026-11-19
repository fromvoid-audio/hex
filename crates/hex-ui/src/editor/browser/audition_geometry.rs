//! Scalar hit mapping from rendered waveform positions to decoded PCM frames.

/// Normalize hit geometry without trusting stale or zero-width bounds.
pub(super) fn hit_fraction(x: f32, left: f32, width: f32) -> Option<f64> {
    if !x.is_finite() || !left.is_finite() || !width.is_finite() || width <= 0.0 {
        return None;
    }
    Some(((f64::from(x) - f64::from(left)) / f64::from(width)).clamp(0.0, 1.0))
}

/// Resolve against the actual decoded PCM, independently of cached display rate.
pub(super) fn frame_at_fraction(frames: usize, fraction: f64) -> usize {
    if !fraction.is_finite() || fraction <= 0.0 {
        return 0;
    }
    if fraction >= 1.0 {
        return frames;
    }
    ((frames as f64 * fraction).floor() as usize).min(frames)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{frame_at_fraction, hit_fraction};

    #[test]
    fn waveform_hit_positions_map_to_bounded_decoded_frames() {
        for (x, expected) in [(-5.0, 0), (10.0, 0), (60.0, 50), (110.0, 100), (200.0, 100)] {
            assert_eq!(
                frame_at_fraction(100, hit_fraction(x, 10.0, 100.0).unwrap()),
                expected
            );
        }
        assert_eq!(
            frame_at_fraction(7, hit_fraction(0.25, 0.0, 0.5).unwrap()),
            3
        );
        assert_eq!(frame_at_fraction(0, 1.0), 0);
        assert_eq!(frame_at_fraction(100, f64::NAN), 0);
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(hit_fraction(1.0, 0.0, width).is_none());
        }
        assert!(hit_fraction(f32::NAN, 0.0, 10.0).is_none());
    }
}
