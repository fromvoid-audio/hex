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
#[path = "../../hex-ui/src/editor/musical_time.rs"]
#[allow(dead_code)] // The editor also consumes ruler, band and input-grid queries.
mod musical_time;
#[path = "../../hex-ui/src/output_meter.rs"]
mod output_meter;
#[path = "../../hex-ui/src/editor/pending_edit.rs"]
mod pending_edit;
#[path = "../../hex-ui/examples/layout_review/sandbox.rs"]
mod sandbox;
#[path = "../../hex-ui/src/section_appearance.rs"]
mod section_appearance;
#[path = "../../hex-ui/src/section_hit.rs"]
mod section_hit;
#[path = "../../hex-ui/src/section_input.rs"]
mod section_input;
#[path = "../../hex-ui/src/section_reorder.rs"]
#[allow(dead_code)] // Main also consumes the tempo preview.
mod section_reorder;
#[path = "../../hex-ui/src/section_resize.rs"]
#[allow(dead_code)] // Main also consumes tempo and marker-only variants.
mod section_resize;
#[path = "../../hex-ui/src/section_selection.rs"]
mod section_selection;
#[path = "../../hex-ui/src/section_surface.rs"]
mod section_surface;
#[path = "../../hex-ui/src/editor/seek.rs"]
#[allow(dead_code)] // The editor also uses transport seek acknowledgement.
mod seek;
#[path = "../../hex-ui/src/theme.rs"]
#[allow(dead_code)]
mod theme;
#[path = "../../hex-ui/src/editor/viewport.rs"]
#[allow(dead_code)] // The editor also uses zoom and scrolling operations.
mod viewport;

#[path = "../../hex-ui/examples/layout_review/launch.rs"]
mod launch;

fn main() {
    launch::run();
}
