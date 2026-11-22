//! # Purpose
//! Own GPUI-free editor geometry, viewport coordinate transforms, waveform
//! sampling grids, clip fade/trim/drag handle math, and pure display-handoff
//! state machines so UI math verifies in under one second without linking GPUI
//! or audio/storage crates.
//!
//! # Dependencies
//! Uses `std` and `hex-project` for canonical clip/fade/trim domain types.
//!
//! # Does not own
//! Does not own GPUI rendering, window input capture, themes, audio playback,
//! session workers, persistence, or project mutation history.

pub mod adaptive_grid_density;
pub mod clip_boundary_geometry;
pub mod drag_geometry;
pub mod fade_handle_geometry;
pub mod midi_pitch_viewport;
pub mod pending_edit;
pub mod seek;
pub mod signed_frame_viewport;
pub mod trim_geometry;
pub mod viewport;
pub mod waveform_grid;
