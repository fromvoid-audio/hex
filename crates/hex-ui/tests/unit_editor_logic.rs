//! Editor logic unit tests without a window or service.
use hex_ui::output_meter;

#[path = "editor_logic_unit/editor.rs"]
mod editor;

#[allow(dead_code)] // This target exercises the source-local theme contracts.
#[path = "../src/theme.rs"]
mod theme;
