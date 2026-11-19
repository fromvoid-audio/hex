//! Shared analyzer display scale and published-frame curve conversion for
//! every spectrum view; response-curve gain axes remain independent.
use hex_audio::spectrum::{SpectrumFrame, band_center_hz};

pub(super) const MIN_DBFS: f32 = -96.0;

pub(in crate::editor) fn level(dbfs: f32) -> f32 {
    ((dbfs - MIN_DBFS) / -MIN_DBFS).clamp(0.0, 1.0)
}

/// Normalized level and held-peak curves for one published frame.
pub(in crate::editor) struct SpectrumCurves {
    pub level: Vec<[f32; 2]>,
    pub peak: Vec<[f32; 2]>,
}

/// Convert a frame to normalized curves. `x` maps a point's center frequency,
/// in Hz, to the owner's horizontal axis; results are clamped to 0..=1.
pub(in crate::editor) fn curves(frame: &SpectrumFrame, x: impl Fn(f32) -> f32) -> SpectrumCurves {
    let curve = |values: &[f32]| {
        values
            .iter()
            .enumerate()
            .filter_map(|(index, dbfs)| {
                let position = x(band_center_hz(frame.sample_rate_hz, index)?);
                position
                    .is_finite()
                    .then(|| [position.clamp(0.0, 1.0), level(*dbfs)])
            })
            .collect()
    };
    SpectrumCurves {
        level: curve(&frame.bins_dbfs),
        peak: curve(&frame.peak_dbfs),
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    #[test]
    fn quiet_signals_remain_visible_on_a_fixed_calibrated_range() {
        assert_eq!(level(-120.0), 0.0);
        assert_eq!(level(-96.0), 0.0);
        assert_eq!(level(-90.0), 0.0625);
        assert_eq!(level(-48.0), 0.5);
        assert_eq!(level(0.0), 1.0);
        assert_eq!(level(6.0), 1.0);
    }

    #[test]
    fn frames_convert_to_monotonic_level_and_peak_curves() {
        let mut frame = SpectrumFrame::silent(48_000);
        frame.bins_dbfs[10] = -48.0;
        frame.peak_dbfs[10] = 0.0;
        let curves = curves(&frame, |hz| (hz / 20.0).ln() / (20_000.0f32 / 20.0).ln());
        assert_eq!(curves.level.len(), hex_audio::spectrum::SPECTRUM_BANDS);
        assert_eq!(curves.peak.len(), curves.level.len());
        assert!(curves.level.windows(2).all(|pair| pair[0][0] <= pair[1][0]));
        assert_eq!(curves.level[10][1], 0.5);
        assert_eq!(curves.peak[10][1], 1.0);
        assert_eq!(curves.level[11][1], 0.0);
        assert!(
            curves
                .level
                .iter()
                .chain(&curves.peak)
                .all(|[x, _]| (0.0..=1.0).contains(x))
        );
    }
}
