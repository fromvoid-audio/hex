//! Delegates timeline-anchored waveform sampling grid to [`hex_ui_model::waveform_grid`].
const _: () = assert!(
    hex_ui_model::waveform_grid::MAX_WAVEFORM_COLUMNS == hex_widgets::MAX_WAVEFORM_COLUMNS
        && hex_ui_model::waveform_grid::MAX_WAVEFORM_ENVELOPE_POINTS
            == hex_widgets::MAX_WAVEFORM_ENVELOPE_POINTS
);

pub(in crate::editor) use hex_ui_model::waveform_grid::WaveformGrid;

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use crate::editor::viewport::Viewport;

    #[test]
    fn reexported_waveform_grid_delegates_to_hex_ui_model() {
        let grid = WaveformGrid::arrangement(
            Viewport {
                start: 123,
                length: 12_000,
            },
            12.0,
            1000.0,
            2.0,
            48_000,
        )
        .unwrap();
        assert_eq!(grid.columns, 2000);
    }
}
