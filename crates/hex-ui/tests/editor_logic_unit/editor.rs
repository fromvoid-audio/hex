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

#[path = "eq_response.rs"]
mod device_panel;
