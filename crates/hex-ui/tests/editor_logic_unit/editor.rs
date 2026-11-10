// Runtime consumers use more methods than this unit-only target.
#[allow(dead_code)]
#[path = "../../src/editor/session_route_view.rs"]
mod session_route_view;
#[allow(dead_code)]
#[path = "../../src/editor/user_errors.rs"]
mod user_errors;

#[path = "../../src/editor/mode.rs"]
mod mode;
pub(super) use mode::EditorMode;
#[path = "../../src/editor/session_navigation.rs"]
mod session_navigation;

#[path = "controller.rs"]
mod controller;

#[path = "audio_import_settings.rs"]
mod audio_import_settings;

#[allow(dead_code)]
#[path = "../../src/editor/refresh_state.rs"]
mod refresh_state;

#[allow(dead_code)]
#[path = "../../src/editor/fade_calculation.rs"]
mod fade_calculation;

#[allow(dead_code)]
#[path = "../../src/editor/console_track_page.rs"]
mod console_track_page;
#[path = "track_meters.rs"]
mod track_meters;

#[allow(dead_code)]
#[path = "../../src/editor/fade_handle_geometry.rs"]
mod fade_handle_geometry;

#[path = "computer_keyboard.rs"]
mod computer_keyboard;

#[allow(dead_code)]
#[path = "../../src/editor/waveform_source.rs"]
mod waveform_source;

#[path = "session_audio_panel.rs"]
mod session_audio_panel;

#[path = "session_grid.rs"]
mod session_grid;

#[path = "eq_response.rs"]
mod device_panel;

// Exercise the production column contract without compiling the full UI owner.
use crate::theme;
#[allow(dead_code)]
#[path = "../../src/editor/waveform_columns.rs"]
mod waveform_columns;
mod waveform {
    pub(super) use super::waveform_columns::waveform_column_count;
}
#[allow(dead_code)]
#[path = "../../src/editor/overview_waveform.rs"]
mod overview_waveform;

#[path = "drag.rs"]
mod drag;

// Compile the real peak projection and stretch root against the shared source mapper.
#[path = "waveform_loop.rs"]
mod waveform_loop;
#[path = "waveform_stretch.rs"]
mod waveform_stretch;

// Compile the production grid against the real viewport and column budget.
#[allow(dead_code)]
#[path = "../../src/editor/viewport.rs"]
mod viewport;
#[path = "waveform_geometry.rs"]
mod waveform_geometry;

#[path = "arrangement_focus.rs"]
mod arrangement_focus;
#[path = "arrangement_map.rs"]
mod arrangement_map;

// Compile the real test gate; unit tests never start a filesystem worker.
#[path = "browser.rs"]
mod browser;
#[allow(dead_code)]
#[path = "../../src/editor/test_environment.rs"]
mod test_environment;
