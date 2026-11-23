//! Editor logic unit tests without a window or service.
// Compile the production peak logic without the meter rendering tests.
#[path = "editor_logic_unit/output_meter.rs"]
mod output_meter;

#[path = "editor_logic_unit/editor.rs"]
mod editor;

#[allow(dead_code)] // This target exercises the source-local theme contracts.
#[path = "../src/theme.rs"]
mod theme;

#[allow(dead_code)]
#[path = "../src/test_wait.rs"]
mod test_wait;

#[path = "editor_logic_unit/log_viewer.rs"]
mod log_viewer;

#[path = "../src/vertical_scrollbar/geometry.rs"]
mod vertical_scrollbar_geometry;
