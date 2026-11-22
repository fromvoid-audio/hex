//! Delegates device-pixel column budget to [`hex_ui_model::waveform_grid`].
const _: () =
    assert!(hex_ui_model::waveform_grid::MAX_WAVEFORM_COLUMNS == hex_widgets::MAX_WAVEFORM_COLUMNS);

pub(super) use hex_ui_model::waveform_grid::waveform_column_count;

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn reexported_waveform_column_count_matches_widget_budget() {
        assert_eq!(
            waveform_column_count(100_000.0, 2.0),
            hex_widgets::MAX_WAVEFORM_COLUMNS
        );
    }
}
