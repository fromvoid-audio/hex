//! # Purpose
//! Run the disposable native widget and device-layout catalog using the real
//! production `hex-ui-kit` and `hex-ui-model` owners.
//!
//! # Dependencies
//! Uses GPUI, `hex-ui-kit`, `hex-ui-model`, `hex-widgets`, `hex-preferences`,
//! `hex-project`, and `hex-devices` for prepared device EQ queries.
//!
//! # Does not own
//! Does not open projects, initialize audio, write preferences, or own
//! application state.
#[allow(unused_imports)]
use hex_ui_kit::{
    assets, context_menu, device_composition, output_meter, section_hit, section_input,
    section_surface, theme,
};
#[allow(unused_imports)]
use hex_ui_model::{
    musical_time, pending_edit, section_appearance, section_reorder, section_resize,
    section_selection, seek, signed_frame_viewport, viewport,
};

mod gallery;
mod launch;
mod sandbox;

fn main() {
    launch::run();
}
