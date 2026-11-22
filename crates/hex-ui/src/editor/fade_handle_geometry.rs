//! Delegates to [`hex_ui_model::fade_handle_geometry`]; pure geometry and unit tests live in `hex-ui-model`.
#[allow(unused_imports)] // Re-exported for fade_handles and unit-tier tests.
pub(super) use hex_ui_model::fade_handle_geometry::{
    Handle, curve_handle_gain, edited_fade, knots,
};

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::ClipFade;

    #[test]
    fn reexported_fade_handle_geometry_delegates_to_hex_ui_model() {
        let fade = ClipFade::new(100, 0, 40, 30).unwrap();
        assert_eq!(knots(fade).len(), 6);
        assert!(edited_fade(fade, Handle::InTop, 10.0, 0).is_some());
    }
}
