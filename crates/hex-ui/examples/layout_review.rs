//! Native, disposable layout catalog. No audio, project or preferences I/O.
// The gallery samples shared compositions; the live windowed controller also
// uses helpers which this disposable fixture does not mount.
#[path = "../src/context_menu.rs"]
mod context_menu;
#[path = "../src/device_composition.rs"]
#[allow(dead_code)]
mod device_composition;
#[path = "layout_review/gallery.rs"]
mod gallery;
#[path = "../src/output_meter.rs"]
mod output_meter;
#[path = "../src/theme.rs"]
#[allow(dead_code)]
mod theme;

#[path = "../src/editor/pending_edit.rs"]
mod pending_edit;
#[path = "../src/section_appearance.rs"]
mod section_appearance;
#[path = "../src/section_hit.rs"]
mod section_hit;
#[path = "../src/section_input.rs"]
mod section_input;
#[path = "../src/section_reorder.rs"]
#[allow(dead_code)] // Main also consumes the tempo preview.
mod section_reorder;
#[path = "../src/section_resize.rs"]
#[allow(dead_code)] // Main also consumes tempo and marker-only variants.
mod section_resize;
#[path = "../src/section_selection.rs"]
mod section_selection;
#[path = "../src/section_surface.rs"]
mod section_surface;

#[path = "layout_review/sandbox.rs"]
mod sandbox;

#[path = "../src/assets.rs"]
#[allow(dead_code)]
mod assets;
#[path = "layout_review/launch.rs"]
mod launch;

fn main() {
    launch::run();
}
