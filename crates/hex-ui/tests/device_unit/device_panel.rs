// Runtime consumers use more methods than this unit-only target.
#[path = "eq_panel.rs"]
mod eq_panel;
#[allow(dead_code)]
#[path = "../../src/editor/device_panel/numeric_display.rs"]
mod numeric_display;
#[allow(dead_code)]
#[path = "../../src/editor/device_panel/spectrum_display.rs"]
mod spectrum_display;
