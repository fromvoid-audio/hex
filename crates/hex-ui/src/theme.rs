//! Product-wide display text and typography; source data keeps original casing.
//!
//! App-owned labels, status, units, tooltips and accessibility text pass through text. Do not
//! case-fold paths, identifiers, file contents or saved user names. Root views inherit bundled
//! Ioskeley Mono; load_fonts must register and verify it before UI startup, with no silent
//! system-font fallback. OS dialogs/menu chrome retain OS fonts. Transparent titlebar rows reserve
//! TITLEBAR_LEADING_INSET independently of compact panel padding so app controls do not paint
//! under native window chrome.
use gpui::{App, SharedString};

#[path = "theme/device.rs"]
mod device;
// Native review examples include this module and use only a subset of its API.
#[allow(unused_imports)]
pub(crate) use device::{
    DeviceActionRole, DeviceStyleRole, device_analyzer_style, device_frame_style,
    device_header_action_style, device_identity_style, device_segment_style, device_style,
    oscillator_identity_color,
};
use std::borrow::Cow;
use std::cell::Cell;

/// Editable palette roles used throughout the native UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteRole {
    Background,
    Surface,
    Raised,
    Clip,
    ClipFill,
    ClipText,
    SecondaryText,
    Hover,
    Border,
    Grid,
    Text,
    Muted,
    Primary,
    PrimaryDark,
    Waveform,
    WaveBackground,
    Secondary,
}

#[derive(Clone, Copy)]
struct ThemeTokens {
    colors: hex_preferences::ThemePreferences,
    elements: [ElementColors; 10],
    clip: u32,
    clip_fill: u32,
    clip_text: u32,
    secondary_text: u32,
    surface: u32,
    raised: u32,
    hover: u32,
    border: u32,
    muted: u32,
    primary_dark: u32,
    waveform: u32,
}

impl ThemeTokens {
    fn from_preferences(colors: hex_preferences::ThemePreferences) -> Self {
        let background = colors.background.packed();
        let text = colors.text.packed();
        let primary = colors.primary.packed();
        Self {
            colors,
            elements: hex_project::ElementColor::ALL
                .map(|identity| resolve_element_colors(identity, lighten(background, 5))),
            clip: mix(background, text, 0.15),
            clip_fill: mix(primary, text, 0.15),
            clip_text: contrast_text(mix(primary, text, 0.15), text),
            secondary_text: contrast_text(colors.secondary.packed(), text),
            surface: lighten(background, 5),
            raised: darken(background, 6),
            hover: lighten(background, 12),
            border: darken(background, 17),
            muted: mix(background, text, 0.68),
            primary_dark: mix(background, primary, 0.08),
            waveform: contrast_text(mix(primary, text, 0.15), text),
        }
    }

    fn color(self, role: PaletteRole) -> u32 {
        match role {
            PaletteRole::Background => self.colors.background.packed(),
            PaletteRole::Surface => self.surface,
            PaletteRole::Raised | PaletteRole::WaveBackground => self.raised,
            PaletteRole::Clip => self.clip,
            PaletteRole::ClipFill => self.clip_fill,
            PaletteRole::ClipText => self.clip_text,
            PaletteRole::SecondaryText => self.secondary_text,
            PaletteRole::Hover | PaletteRole::Grid => self.hover,
            PaletteRole::Border => self.border,
            PaletteRole::Text => self.colors.text.packed(),
            PaletteRole::Muted => self.muted,
            PaletteRole::Primary => self.colors.primary.packed(),
            PaletteRole::PrimaryDark => self.primary_dark,
            PaletteRole::Waveform => self.waveform,
            PaletteRole::Secondary => self.colors.secondary.packed(),
        }
    }
}

thread_local! {
    /// UI-thread-only immutable token cache. Audio and project workers never
    /// read theme state; updates replace the complete derived snapshot.
    static ACTIVE_THEME: Cell<ThemeTokens> = Cell::new(ThemeTokens::from_preferences(
        hex_preferences::ThemePreferences::default(),
    ));
}

pub fn color(role: PaletteRole) -> u32 {
    ACTIVE_THEME.with(|theme| theme.get().color(role))
}

pub fn preferences() -> hex_preferences::ThemePreferences {
    ACTIVE_THEME.with(|theme| theme.get().colors)
}

pub fn apply_preferences(colors: hex_preferences::ThemePreferences) {
    ACTIVE_THEME.with(|theme| theme.set(ThemeTokens::from_preferences(colors)));
}

fn mix(first: u32, second: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let left = ((first >> shift) & 0xff) as f32;
        let right = ((second >> shift) & 0xff) as f32;
        (left + (right - left) * amount).round().clamp(0.0, 255.0) as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn lighten(color: u32, amount: u32) -> u32 {
    let channel = |shift: u32| (((color >> shift) & 0xff) + amount).min(0xff);
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn darken(color: u32, amount: u32) -> u32 {
    let channel = |shift: u32| ((color >> shift) & 0xff).saturating_sub(amount);
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn contrast_text(background: u32, preferred: u32) -> u32 {
    if contrast_ratio(background, preferred) >= 4.5 {
        return preferred;
    }
    let black = 0x000000;
    let white = 0xffffff;
    if contrast_ratio(background, black) >= contrast_ratio(background, white) {
        black
    } else {
        white
    }
}

fn contrast_ratio(first: u32, second: u32) -> f32 {
    let luminance = |color: u32| {
        let linear = |shift: u32| {
            let channel = ((color >> shift) & 0xff) as f32 / 255.0;
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0)
    };
    let (light, dark) = if luminance(first) >= luminance(second) {
        (luminance(first), luminance(second))
    } else {
        (luminance(second), luminance(first))
    };
    (light + 0.05) / (dark + 0.05)
}

pub fn colors_are_legible(colors: hex_preferences::ThemePreferences) -> bool {
    contrast_ratio(colors.background.packed(), colors.text.packed()) >= 4.5
        && contrast_ratio(colors.background.packed(), colors.primary.packed()) >= 3.0
        && contrast_ratio(colors.background.packed(), colors.secondary.packed()) >= 3.0
}

/// Choose a readable derived foreground without changing any saved color role.
pub fn readable_text(background: u32, preferred: u32) -> u32 {
    contrast_text(background, preferred)
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

/// Reusable element palette for tracks, collections and other semantic markers.
/// Fixed, restrained hues are mixed with the active neutral surfaces.
#[derive(Clone, Copy)]
pub struct ElementColors {
    pub muted: u32,
    pub accent: u32,
    pub muted_text: u32,
    pub accent_text: u32,
}
pub fn element_colors(identity: hex_project::ElementColor) -> ElementColors {
    ACTIVE_THEME.with(|theme| theme.get().elements[identity as usize])
}
fn resolve_element_colors(identity: hex_project::ElementColor, surface: u32) -> ElementColors {
    use hex_project::ElementColor;
    let hue = match identity {
        ElementColor::Gray => 0xa4aaa8,
        ElementColor::Red => 0xcf7773,
        ElementColor::Orange => 0xd58d69,
        ElementColor::Amber => 0xc5aa70,
        ElementColor::Olive => 0xa3ad79,
        ElementColor::Green => 0x7eae94,
        ElementColor::Cyan => 0x77b6bb,
        ElementColor::Blue => 0x809fca,
        ElementColor::Violet => 0xa195c3,
        ElementColor::Rose => 0xc18fa6,
    };
    let muted = mix(surface, hue, 0.82);
    let accent = mix(surface, hue, 0.98);
    ElementColors {
        muted,
        accent,
        muted_text: contrast_text(muted, 0x000000),
        accent_text: contrast_text(accent, 0x000000),
    }
}
pub fn clip_fill(
    color: hex_project::ElementColor,
    flavor: hex_project::ClipFlavor,
    selected: bool,
) -> u32 {
    let colors = element_colors(color);
    let base = match flavor {
        hex_project::ClipFlavor::Muted => colors.muted,
        hex_project::ClipFlavor::Accent => colors.accent,
    };
    if selected { lighten(base, 22) } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RestoreTheme(hex_preferences::ThemePreferences);

    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            apply_preferences(self.0);
        }
    }

    #[test]
    fn default_palette_keeps_the_four_shipped_roles_and_readable_derived_tokens() {
        let _restore = RestoreTheme(preferences());
        apply_preferences(hex_preferences::ThemePreferences::default());
        let defaults = hex_preferences::ThemePreferences::default();
        assert_eq!(color(PaletteRole::Background), defaults.background.packed());
        assert_eq!(color(PaletteRole::Text), defaults.text.packed());
        assert_eq!(color(PaletteRole::Primary), defaults.primary.packed());
        assert_eq!(color(PaletteRole::Secondary), defaults.secondary.packed());
        assert!(colors_are_legible(defaults));
        assert!(contrast_ratio(color(PaletteRole::ClipFill), color(PaletteRole::ClipText)) >= 4.5);
        assert!(
            contrast_ratio(
                color(PaletteRole::Secondary),
                color(PaletteRole::SecondaryText)
            ) >= 4.5
        );
    }

    #[test]
    fn illegible_draft_is_rejected_without_replacing_any_saved_color_role() {
        let _restore = RestoreTheme(preferences());
        let selected = hex_preferences::ThemePreferences {
            background: hex_preferences::RgbColor::new(30, 30, 30),
            text: hex_preferences::RgbColor::new(31, 31, 31),
            primary: hex_preferences::RgbColor::new(200, 80, 60),
            secondary: hex_preferences::RgbColor::new(40, 180, 190),
        };
        assert!(!colors_are_legible(selected));
        let before = preferences();
        assert_ne!(selected, before);
        apply_preferences(before);
        assert_eq!(preferences(), before);
        assert_eq!(color(PaletteRole::Primary), before.primary.packed());
    }
}

#[cfg(test)]
mod element_palette_tests {
    use super::*;
    #[test]
    fn both_flavors_keep_ink_legible_and_identities_distinct() {
        let mut hues = std::collections::BTreeSet::new();
        for identity in hex_project::ElementColor::ALL {
            let colors = element_colors(identity);
            assert!(hues.insert(colors.accent));
            assert_ne!(colors.muted, colors.accent);
            assert!(contrast_ratio(colors.muted, colors.muted_text) >= 4.5);
            assert!(contrast_ratio(colors.accent, colors.accent_text) >= 4.5);
            // Arrangement waveforms and note thumbnails use neutral black ink.
            assert!(contrast_ratio(colors.muted, 0x000000) >= 4.5);
            assert!(
                contrast_ratio(resolve_element_colors(identity, 0x000000).muted, 0x000000) >= 4.5
            );
        }
        assert_eq!(hues.len(), 10);
    }
}
