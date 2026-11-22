//! Delegates to [`hex_ui_model::midi_pitch_viewport`]; pure math and unit tests live in `hex-ui-model`.
pub(super) use hex_ui_model::midi_pitch_viewport::PitchViewport;

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::PitchViewport;

    #[test]
    fn reexported_pitch_viewport_preserves_default_coverage() {
        assert_eq!(PitchViewport::DEFAULT.coverage(), 48..72);
    }
}
