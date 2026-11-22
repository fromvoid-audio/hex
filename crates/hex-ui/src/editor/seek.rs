//! Delegates to [`hex_ui_model::seek`]; pure math and unit tests live in `hex-ui-model`.
#[allow(unused_imports)] // hex-sandbox uses only pointer_to_frame; hex-ui uses both.
pub(super) use hex_ui_model::seek::{pointer_to_frame, seek_is_pending};

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::super::viewport::Viewport;
    use super::{pointer_to_frame, seek_is_pending};

    #[test]
    fn reexported_seek_geometry_maps_ruler_and_gate() {
        let viewport = Viewport {
            start: 100,
            length: 11,
        };
        assert_eq!(pointer_to_frame(20.0, 10.0, 20.0, viewport), Some(106));
        assert!(seek_is_pending(Some(7), 6, false));
    }
}
