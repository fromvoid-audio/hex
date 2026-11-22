//! Pure clip boundary chamfer geometry independent of GPUI layout/paint.

/// Bounded 45-degree corner chamfer extent for clip headers and outlines.
pub fn corner_extent(width: f32, height: f32) -> f32 {
    if !width.is_finite() || !height.is_finite() {
        return 0.0;
    }
    5.0_f32.min(width.max(0.0) / 2.0).min(height.max(0.0) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_extent_is_bounded_and_rejects_non_finite_inputs() {
        assert_eq!(corner_extent(100.0, 44.0), 5.0);
        assert_eq!(corner_extent(6.0, 44.0), 3.0);
        assert_eq!(corner_extent(100.0, 4.0), 2.0);
        assert_eq!(corner_extent(0.0, 44.0), 0.0);
        assert_eq!(corner_extent(-1.0, 44.0), 0.0);
        assert_eq!(corner_extent(f32::NAN, 44.0), 0.0);
        assert_eq!(corner_extent(100.0, f32::INFINITY), 0.0);
    }
}
