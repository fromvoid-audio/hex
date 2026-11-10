//! Timeline-anchored waveform sampling grid.
use hex_widgets::MAX_WAVEFORM_ENVELOPE_POINTS;

/// A sampling lattice shared by all clips in a timeline viewport. Clip edges
/// select columns from it; they never change its density or horizontal origin.
#[derive(Clone, Copy, Debug)]
pub(in crate::editor) struct WaveformGrid {
    pub(in crate::editor) start: u64,
    pub(in crate::editor) end: u64,
    pub(in crate::editor) left: f32,
    pub(in crate::editor) width: f32,
    pub(in crate::editor) columns: usize,
    /// Coarse bins hold their extrema across the whole interval, so masking a
    /// tiny clip cannot hide a peak whose bin center falls outside the clip.
    pub(in crate::editor) binned: bool,
}

impl WaveformGrid {
    /// Static arranger detail, measured in seconds per physical pixel. Coarse
    /// views aggregate extrema over wider bins; detailed editors keep their
    /// supplied grids. This never reads capture, playback, idle time or frame
    /// timing, so releasing a gesture cannot change waveform quality.
    pub(in crate::editor) fn arrangement(
        viewport: super::super::viewport::Viewport,
        left: f32,
        width: f32,
        scale: f32,
        sample_rate: u32,
    ) -> Option<Self> {
        let end = viewport.end()?;
        if viewport.length == 0
            || sample_rate == 0
            || !left.is_finite()
            || !width.is_finite()
            || width <= 0.0
            || !(left + width).is_finite()
            || !scale.is_finite()
            || scale <= 0.0
        {
            return None;
        }
        let seconds_per_pixel =
            viewport.length as f64 / f64::from(sample_rate) / (f64::from(width) * f64::from(scale));
        let spacing = if seconds_per_pixel >= 0.05 {
            8
        } else if seconds_per_pixel >= 0.005 {
            4
        } else if seconds_per_pixel >= 0.001 {
            2
        } else {
            1
        };
        let columns =
            super::super::waveform_columns::waveform_column_count(width, scale).div_ceil(spacing);
        // Two envelope points per coarse bin, including three padding bins,
        // must fit the existing shared painter's point budget.
        let columns = if spacing > 1 {
            columns.min(MAX_WAVEFORM_ENVELOPE_POINTS / 2 - 3)
        } else {
            columns
        };
        let mut grid = Self {
            start: viewport.start,
            end,
            left,
            width,
            columns,
            binned: spacing > 1,
        };
        if spacing == 1 {
            return Some(grid);
        }
        // An integer frame step anchors bins to the timeline, rather than the
        // moving viewport. Pad with neighboring bins for edge interpolation.
        // At the u64 boundary, keep the same coarse density and full coverage;
        // a span that cannot be padded uses the valid unpadded grid above.
        let step = viewport.length.div_ceil(columns as u64);
        let padded_columns = columns.checked_add(3)?;
        if let Some(span) = step.checked_mul(padded_columns as u64) {
            let aligned = (viewport.start / step).checked_mul(step)?;
            let start = aligned.saturating_sub(step).min(u64::MAX - span);
            let padded_end = start.checked_add(span)?;
            if start <= viewport.start && padded_end >= end {
                let padded_left = f64::from(left)
                    - (viewport.start - start) as f64 / viewport.length as f64 * f64::from(width);
                let padded_width = span as f64 / viewport.length as f64 * f64::from(width);
                let (padded_left, padded_width) = (padded_left as f32, padded_width as f32);
                if padded_left.is_finite() && padded_width.is_finite() && padded_width > 0.0 {
                    grid = Self {
                        start,
                        end: padded_end,
                        left: padded_left,
                        width: padded_width,
                        columns: padded_columns,
                        binned: true,
                    };
                }
            }
        }
        Some(grid)
    }

    pub(super) fn key(self) -> [u64; 6] {
        [
            self.start,
            self.end,
            u64::from(self.left.to_bits()),
            u64::from(self.width.to_bits()),
            self.columns as u64,
            self.binned as u64,
        ]
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_widgets::MAX_WAVEFORM_COLUMNS;

    #[test]
    fn arranger_detail_preserves_close_zoom_and_anchors_coarse_bins() {
        use crate::editor::viewport::Viewport;
        for sample_rate in [8_000_u32, 44_100, 48_000, 192_000] {
            let close = Viewport {
                start: 123,
                length: u64::from(sample_rate) / 4,
            };
            let grid = WaveformGrid::arrangement(close, 12.0, 1000.0, 2.0, sample_rate).unwrap();
            assert_eq!((grid.start, grid.end), (close.start, close.end().unwrap()));
            assert_eq!((grid.left, grid.width, grid.columns), (12.0, 1000.0, 2000));
            let length = u64::from(sample_rate) * 120;
            let mut step = None;
            for start in [0, 17_123, length / 2] {
                let view = Viewport { start, length };
                let grid = WaveformGrid::arrangement(view, 12.0, 1000.0, 2.0, sample_rate).unwrap();
                assert!(grid.start <= start && grid.end >= view.end().unwrap());
                // The dense overview needs at most one column per eight
                // physical pixels, while retaining neighbor bins at the edges.
                assert!(grid.columns as f32 / (grid.width * 2.0) <= 0.126);
                let current_step = (grid.end - grid.start) / grid.columns as u64;
                assert_eq!(grid.start % current_step, 0);
                assert_eq!(grid.end - grid.start, current_step * grid.columns as u64);
                if let Some(previous) = step {
                    assert_eq!(previous, current_step, "pan changed the sampling density");
                }
                step = Some(current_step);
            }
        }
    }

    #[test]
    fn arranger_grid_refuses_invalid_numbers_and_covers_u64_edges() {
        use crate::editor::viewport::Viewport;
        let view = Viewport {
            start: 0,
            length: 1_000_000,
        };
        for (left, width, scale, rate) in [
            (0.0, 0.0, 2.0, 44_100),
            (0.0, f32::NAN, 2.0, 44_100),
            (f32::INFINITY, 1000.0, 2.0, 44_100),
            (f32::MAX, f32::MAX, 2.0, 44_100),
            (0.0, 1000.0, -1.0, 44_100),
            (0.0, 1000.0, 2.0, 0),
        ] {
            assert!(WaveformGrid::arrangement(view, left, width, scale, rate).is_none());
        }
        for view in [
            Viewport {
                start: u64::MAX - 1_000_000,
                length: 1_000_000,
            },
            Viewport {
                start: 0,
                length: u64::MAX,
            },
        ] {
            let grid = WaveformGrid::arrangement(view, 12.0, 1000.0, 2.0, 44_100).unwrap();
            assert!(grid.start <= view.start && grid.end >= view.end().unwrap());
            assert!((1..=MAX_WAVEFORM_COLUMNS).contains(&grid.columns));
            assert!(grid.left.is_finite() && grid.width.is_finite() && grid.width > 0.0);
        }
        assert!(
            WaveformGrid::arrangement(
                Viewport {
                    start: u64::MAX,
                    length: 1
                },
                0.0,
                1000.0,
                2.0,
                44_100
            )
            .is_none()
        );
    }
}
