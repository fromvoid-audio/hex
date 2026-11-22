//! Delegates to [`hex_ui_model::signed_frame_viewport`]; pure math and unit tests live in `hex-ui-model`.
pub(super) use hex_ui_model::signed_frame_viewport::SignedFrameViewport;

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::super::viewport::Viewport;
    use super::SignedFrameViewport;

    #[test]
    fn reexported_signed_viewport_matches_unsigned_projection() {
        let view = SignedFrameViewport {
            start: -50,
            length: 100,
        };
        assert_eq!(
            view.visible_unsigned(),
            Some(Viewport {
                start: 0,
                length: 50
            })
        );
    }
}
