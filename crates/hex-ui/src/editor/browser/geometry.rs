//! Scalar browser width limits and window-relative drag ghost geometry.

pub(super) const MIN_WIDTH: f32 = 220.0;
pub(in crate::editor) const MIN_WORKSPACE_WIDTH: f32 = 560.0;

pub(super) fn clamped_panel_width(requested: f32, window_width: f32, panel_gutter: f32) -> f32 {
    requested.clamp(
        MIN_WIDTH,
        (window_width
            - MIN_WORKSPACE_WIDTH
            - 4.0
            // Both the browser and workspace panels have framed surrounds.
            - 4.0 * hex_widgets::PANEL_FRAME_INSET
            - panel_gutter)
            .max(MIN_WIDTH),
    )
}

pub(super) fn browser_audio_drag_ghost_position(pointer: (f32, f32)) -> (f32, f32) {
    (pointer.0 + 12.0, pointer.1 + 12.0)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{MIN_WIDTH, browser_audio_drag_ghost_position, clamped_panel_width};

    #[test]
    fn browser_drag_ghost_position_uses_window_relative_pointer_coordinates() {
        // Window::bounds().origin is global screen space; it is intentionally not
        // accepted by this helper because GPUI pointer events are window-relative.
        assert_eq!(
            browser_audio_drag_ghost_position((317.0, 129.0)),
            (329.0, 141.0)
        );
    }

    #[test]
    fn browser_width_preserves_minimum_workspace_and_panel() {
        assert_eq!(clamped_panel_width(280.0, 1_240.0, 3.0), 280.0);
        assert_eq!(clamped_panel_width(500.0, 880.0, 3.0), 297.0);
        assert_eq!(clamped_panel_width(100.0, 880.0, 3.0), MIN_WIDTH);
    }
}
