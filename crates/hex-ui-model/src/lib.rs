//! # Purpose
//! Own GPUI-free editor geometry, musical time and adaptive grid math, MIDI
//! piano-roll scene/selection/drag/stroke models, Arrangement clip indexing and
//! miniature map projection, Session navigation, Console paging, song-section
//! reorder/resize/selection/appearance/geometry models, viewport coordinate
//! transforms, waveform sampling grids, output meter peak conversion, clip
//! fade/trim/drag handle math, and pure display-handoff state machines so pure
//! UI logic verifies in under one second without compiling or linking GPUI or
//! audio/storage crates.
//!
//! # Dependencies
//! Uses `std` and `hex-project` for canonical project, clip, MIDI, section,
//! tempo, fade, and trim domain types.
//!
//! # Does not own
//! Does not own GPUI rendering, window input capture, themes, assets, audio
//! playback, session workers, persistence, or project mutation history.

pub mod adaptive_grid_density;
pub mod arrangement_clip_index;
pub mod arrangement_focus_interval;
pub mod arrangement_map;
pub mod audio_admission;
pub mod clip_boundary_geometry;
pub mod clone_source_window;
pub mod console_track_page;
pub mod cpu_meter;
pub mod disk_overload;
pub mod drag_geometry;
pub mod elapsed_time;
pub mod eq_panel;
pub mod fade_calculation;
pub mod fade_handle_geometry;
pub mod group_overlap;
pub mod midi_drag;
pub mod midi_group_drag;
pub mod midi_pitch_viewport;
pub mod midi_scene;
pub mod midi_selection;
pub mod midi_stroke;
pub mod midi_thumbnail;
pub mod midi_velocity;
pub mod midi_viewport_grid;
pub mod mode;
pub mod musical_time;
pub mod navigation_selection;
pub mod output_meter_logic;
pub mod pending_edit;
pub mod play_intent;
pub mod recovery_state;
pub mod refresh_state;
pub mod section_appearance;
pub mod section_focus_geometry;
pub mod section_geometry;
pub mod section_reorder;
pub mod section_resize;
pub mod section_selection;
pub mod seek;
pub mod session_navigation;
pub mod session_route_view;
pub mod signed_frame_viewport;
pub mod tempo_input_value;
pub mod track_meter_scale;
pub mod track_proposals;
pub mod trim_geometry;
pub mod user_errors;
pub mod viewport;
pub mod waveform_grid;
