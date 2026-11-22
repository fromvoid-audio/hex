//! Delegates to [`hex_ui_model::drag_geometry`]; pure geometry and unit tests live in `hex-ui-model`.
pub(super) use hex_ui_model::drag_geometry::lane_contains_pointer;
#[allow(unused_imports)] // Re-exported for editor drag and unit-tier consumers.
pub(in crate::editor) use hex_ui_model::drag_geometry::{
    ClipSnapEdge, LaneBounds, clip_snap_edge, drop_start, grab_offset, grab_offset_from_pointer,
};

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{LaneBounds, drop_start, grab_offset, lane_contains_pointer};

    #[test]
    fn reexported_drag_geometry_delegates_to_hex_ui_model() {
        assert_eq!(grab_offset(25.0, 50.0, 120, 220, 40), Some(130));
        let lane = LaneBounds {
            left: 10.0,
            width: 100.0,
            top: 0.0,
            height: 44.0,
        };
        assert_eq!(drop_start(60.0, lane, 0, 200, 15), Some(85));
        assert!(lane_contains_pointer(20.0, 10.0, lane));
    }
}
