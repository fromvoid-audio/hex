//! Product-wide text rules. Identifiers and source data keep their original casing.
use gpui::{App, SharedString};
use std::borrow::Cow;

/// Accepted Frame iteration 3 palette shared by the compact workspace panels.
pub struct Palette;

impl Palette {
    pub const BACKGROUND: u32 = 0x2a2828;
    pub const SURFACE: u32 = 0x2f2d2d;
    pub const RAISED: u32 = 0x242222;
    pub const CLIP: u32 = 0x454c49;
    pub const CLIP_FILL: u32 = 0xcf7362;
    pub const CLIP_SELECTED: u32 = 0xf18c73;
    pub const CLIP_HOVER: u32 = 0xdb8570;
    pub const CLIP_TEXT: u32 = 0x1b1412;
    pub const HOVER: u32 = 0x363333;
    pub const BORDER: u32 = 0x191717;
    pub const GRID: u32 = 0x363333;
    pub const TEXT: u32 = 0xc3c5c0;
    pub const MUTED: u32 = 0x9ea59f;
    pub const CORAL: u32 = 0xf06449;
    pub const CORAL_DARK: u32 = 0x35302f;
    pub const WAVEFORM: u32 = 0xc0c4bb;
    pub const WAVE_BG: u32 = 0x242222;
    pub const CYAN: u32 = 0x70e8e3;
}

// Transparent macOS titlebars overlay native window controls on the first row.
// Keep this chrome reservation separate from compact panel padding.
pub const TITLEBAR_LEADING_INSET: f32 = if cfg!(target_os = "macos") { 80.0 } else { 8.0 };

pub const FONT_FAMILY: &str = "Ioskeley Mono";
const REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/ioskeley-mono/ioskeley-mono-regular.ttf");
const MEDIUM: &[u8] =
    include_bytes!("../../../assets/fonts/ioskeley-mono/ioskeley-mono-medium.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/ioskeley-mono/ioskeley-mono-bold.ttf");
const ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/ioskeley-mono/ioskeley-mono-italic.ttf");

pub fn load_fonts(cx: &App) -> Result<(), String> {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(REGULAR),
            Cow::Borrowed(MEDIUM),
            Cow::Borrowed(BOLD),
            Cow::Borrowed(ITALIC),
        ])
        .map_err(|error| format!("could not load bundled ioskeley mono: {error}"))?;
    if !cx
        .text_system()
        .all_font_names()
        .iter()
        .any(|name| name == FONT_FAMILY)
    {
        return Err("bundled ioskeley mono was not registered".into());
    }
    Ok(())
}

/// Display normalization only. Never use this for paths, project IDs, or saved names.
pub fn text(value: impl AsRef<str>) -> SharedString {
    value.as_ref().to_lowercase().into()
}
