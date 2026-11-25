//! Editor logic unit tests without a window or service.
#[allow(unused_imports)]
use hex_ui_kit::{output_meter, theme};

#[path = "editor_logic_unit/editor.rs"]
mod editor;

#[allow(dead_code)]
#[path = "../src/test_wait.rs"]
mod test_wait;

#[path = "editor_logic_unit/log_viewer.rs"]
mod log_viewer;

#[path = "../src/vertical_scrollbar"]
mod vertical_scrollbar {
    mod geometry;
}
