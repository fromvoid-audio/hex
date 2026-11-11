//! Device-pixel column budget shared by waveform projections.
use hex_widgets::MAX_WAVEFORM_COLUMNS;

pub(super) fn waveform_column_count(width: f32, scale: f32) -> usize {
    if !width.is_finite() || width <= 0.0 || !scale.is_finite() || scale <= 0.0 {
        return 1;
    }
    ((width * scale).ceil() as usize).clamp(1, MAX_WAVEFORM_COLUMNS)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn waveform_density_tracks_device_pixels_and_stays_bounded() {
        assert_eq!(waveform_column_count(120.0, 1.0), 120);
        assert_eq!(waveform_column_count(120.0, 2.0), 240);
        assert_eq!(waveform_column_count(100_000.0, 2.0), MAX_WAVEFORM_COLUMNS);
        assert_eq!(waveform_column_count(f32::NAN, 2.0), 1);
    }
}
