//! Section geometry unit-tier smoke re-export; authoritative unit tests live in
//! `hex-ui-model` and `hex-ui-kit`.
#[allow(unused_imports)]
use hex_ui_kit::{section_hit, section_input, section_surface};
#[allow(unused_imports)]
use hex_ui_model::{
    section_appearance, section_focus_geometry, section_geometry, section_reorder, section_resize,
    section_selection,
};

#[test]
fn section_model_and_kit_crates_are_wired() {
    const {
        assert!(section_geometry::CHAMFER > 0.0);
        assert!(section_hit::INNER_EDGE_WIDTH > 0.0);
    }
}
