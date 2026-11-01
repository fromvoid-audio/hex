//! # Purpose
//! Run the disposable native widget and device-layout catalog using the real UI owners.
//!
//! # Dependencies
//! Uses GPUI and shared widgets, preferences, project colors and prepared device EQ queries.
//!
//! # Does not own
//! Does not open projects, initialize audio, write preferences or own application state.
// The sandbox mounts the production-owned source directly; no duplicate UI implementation.
#[path = "../../hex-ui/src/assets.rs"]
#[allow(dead_code)]
mod assets;
#[path = "../../hex-ui/src/context_menu.rs"]
mod context_menu;
#[path = "../../hex-ui/src/device_composition.rs"]
#[allow(dead_code)]
mod device_composition;
#[path = "../../hex-ui/examples/layout_review/gallery.rs"]
mod gallery;
#[path = "../../hex-ui/src/output_meter.rs"]
mod output_meter;
#[path = "../../hex-ui/examples/layout_review/sandbox.rs"]
mod sandbox;
#[path = "../../hex-ui/src/theme.rs"]
#[allow(dead_code)]
mod theme;

#[path = "../../hex-ui/examples/layout_review/launch.rs"]
mod launch;

fn main() {
    launch::run();
}
