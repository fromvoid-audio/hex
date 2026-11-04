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
    DeviceActionRole, DeviceStyleRole, device_analyzer_style, device_enable_colors,
    device_frame_style, device_header_action_style, device_identity_style, device_segment_style,
    device_style, oscillator_identity_color,
};
use std::borrow::Cow;
use std::cell::Cell;

/// Common widget palette for device panels, workspace fields and preferences.
pub(crate) fn control_style() -> hex_widgets::ControlStyle {
    device_style(DeviceStyleRole::Control)
}

/// Shared product colors for all sample-level meters; widgets own no palette.
pub(crate) fn meter_colors() -> hex_widgets::MeterColors {
    hex_widgets::MeterColors {
        signal: color(PaletteRole::Secondary),
        near_peak: color(PaletteRole::Primary),
        clipping: 0xe34b4b,
    }
}

/// Common workspace action colors, resolved from the native widget palette.
pub(crate) fn action_style() -> hex_widgets::ActionStyle {
    let style = control_style();
    hex_widgets::ActionStyle {
        background: style.raised,
        text: readable_text(style.raised, style.text),
        hover_background: style.hover,
        hover_text: Some(style.hover_text),
    }
}

/// Brighter momentary press fill with foreground resolved against that fill.
#[allow(dead_code)] // Consumed by the standalone native sandbox example.
pub(crate) fn action_press_style(style: hex_widgets::ActionStyle) -> hex_widgets::ActionPressStyle {
    let background = mix(style.background, 0xffffff, 0.22);
    hex_widgets::ActionPressStyle {
        background,
        text: readable_text(background, style.text),
    }
}

/// High-contrast time-position triangles over the ruler and Arrangement.
pub(crate) fn time_position_marker_color() -> u32 {
    readable_text(color(PaletteRole::Surface), 0xffffff)
}

/// Neutral four-unit bands stay visible independently of the saved palette.
pub(crate) fn time_grid_band_color(alternate: bool) -> u32 {
    mix(
        color(PaletteRole::Background),
        color(PaletteRole::Text),
        if alternate { 0.07 } else { 0.015 },
    )
}

/// Stronger bar ticks retain a quieter intermediate-beat hierarchy.
pub(crate) fn ruler_tick_color(bar_start: bool, meter_marker: bool) -> u32 {
    if meter_marker {
        color(PaletteRole::Primary)
    } else {
        mix(
            color(PaletteRole::Background),
            color(PaletteRole::Text),
            if bar_start { 0.55 } else { 0.3 },
        )
    }
}

/// Near-white time-range fill shared by the arranger and clip editors, tinted
/// slightly toward the palette text so it stays within the saved theme.
pub(crate) fn time_selection_fill() -> u32 {
    mix(color(PaletteRole::Text), 0xffffff, 0.8)
}

/// Shared translucency for arranger and clip-editor time ranges. The fill
/// paints above clips, so this stays low enough for waveforms and clip names
/// to remain readable through the bright band.
pub(crate) const TIME_SELECTION_OPACITY: f32 = 0.42;

/// Move-destination ranges stay quieter than the committed selection.
pub(crate) const TIME_SELECTION_PREVIEW_OPACITY: f32 = 0.2;

/// Explicit track membership stays quieter than time/clip selection overlays.
pub(crate) const TRACK_SELECTION_OPACITY: f32 = 0.06;

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
    /// Unselected clip boundary/outline stroke.
    ClipOutline,
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
    muted_clip_fills: [u32; 10],
    clip: u32,
    clip_fill: u32,
    clip_text: u32,
    secondary_text: u32,
    surface: u32,
    raised: u32,
    hover: u32,
    border: u32,
    clip_outline: u32,
    muted: u32,
    primary_dark: u32,
    waveform: u32,
}

impl ThemeTokens {
    fn from_preferences(colors: hex_preferences::ThemePreferences) -> Self {
        let background = colors.background.packed();
        let text = colors.text.packed();
        let primary = colors.primary.packed();
        let elements = hex_project::ElementColor::ALL
            .map(|identity| resolve_element_colors(identity, lighten(background, 5)));
        Self {
            colors,
            muted_clip_fills: elements.map(|colors| muted_clip_fill(colors.accent)),
            elements,
            clip: mix(background, text, 0.15),
            clip_fill: mix(primary, text, 0.15),
            clip_text: contrast_text(mix(primary, text, 0.15), text),
            secondary_text: contrast_text(colors.secondary.packed(), text),
            surface: lighten(background, 5),
            raised: darken(background, 6),
            hover: lighten(background, 12),
            border: darken(background, 17),
            // Keep outlines dark against clip fills while following custom backgrounds.
            clip_outline: mix(0x000000, darken(background, 42), 0.25),
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
            PaletteRole::ClipOutline => self.clip_outline,
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

/// Test-only injection of the clip-outline role. The returned guard restores
/// the derived tokens of the active preferences when dropped.
#[cfg(test)]
#[allow(dead_code)]
#[must_use]
pub(crate) fn inject_test_clip_outline(outline: u32) -> TestClipOutline {
    ACTIVE_THEME.with(|theme| {
        let mut tokens = theme.get();
        tokens.clip_outline = outline;
        theme.set(tokens);
    });
    TestClipOutline
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) struct TestClipOutline;

#[cfg(test)]
impl Drop for TestClipOutline {
    fn drop(&mut self) {
        apply_preferences(preferences());
    }
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

/// Display normalization only. Lowercases app text but keeps spans marked with
/// `hex_session::service::text::verbatim` (paths, saved names) exactly as written,
/// then removes the marks. Pass bare paths, project IDs or saved names unmarked
/// only through `verbatim`, never as plain text.
pub fn text(value: impl AsRef<str>) -> SharedString {
    verbatim_text::plain(&verbatim_text::lowercase_outside_verbatim(value.as_ref())).into()
}

// hex-sandbox compiles this file by path without depending on hex-session or
// hex-storage, so compile hex-storage's dependency-free text module the same way.
// Producers in other crates use `verbatim`; this copy only consumes marks.
#[allow(dead_code)]
#[path = "../../hex-storage/src/text.rs"]
mod verbatim_text;

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
/// Darken the track hue by removing neutral light, increasing saturation without
/// shifting its hue. Resolve once per theme; retain 4.5:1 contrast for the existing
/// black waveform/note ink and Console foregrounds, including historical gray.
fn muted_clip_fill(accent: u32) -> u32 {
    (0..=40)
        .rev()
        .map(|amount| darken(accent, amount))
        .find(|fill| contrast_ratio(*fill, 0x000000) >= 4.5)
        .unwrap_or(accent)
}

/// Shared audio/MIDI body, Console cell, drag preview and title fill. The persisted
/// `Muted` flavor is darker and more saturated; Accent keeps the track hue.
/// Track strips, sections and browser marker palettes retain their own surfaces.
pub fn clip_fill(
    color: hex_project::ElementColor,
    flavor: hex_project::ClipFlavor,
    selected: bool,
) -> u32 {
    let base = ACTIVE_THEME.with(|theme| {
        let theme = theme.get();
        match flavor {
            hex_project::ClipFlavor::Muted => theme.muted_clip_fills[color as usize],
            hex_project::ClipFlavor::Accent => theme.elements[color as usize].accent,
        }
    });
    if selected { lighten(base, 22) } else { base }
}

/// Activation is independent of the persisted color flavor. Keep inactive clips
/// fully neutral regardless of track hue, flavor or customized theme colors.
/// Mid-gray retains readable black labels; selection remains visibly lighter.
pub fn clip_activity_fill(
    color: hex_project::ElementColor,
    flavor: hex_project::ClipFlavor,
    selected: bool,
    active: bool,
) -> u32 {
    if active {
        clip_fill(color, flavor, selected)
    } else if selected {
        0x9a9a9a
    } else {
        0x888888
    }
}

/// Restrained Main section surfaces; selection changes the whole fill and is also
/// marked with a check in the lane. Hover remains distinct in both states.
pub(crate) fn section_colors(
    identity: hex_project::ElementColor,
    selected: bool,
    hovered: bool,
) -> (u32, u32, u32) {
    let surface = color(PaletteRole::Surface);
    let hue = element_colors(identity).muted;
    let fill = mix(
        surface,
        hue,
        match (selected, hovered) {
            (false, false) => 0.30,
            (false, true) => 0.44,
            (true, false) => 0.70,
            (true, true) => 0.82,
        },
    );
    let text = contrast_text(fill, color(PaletteRole::Text));
    let border = if selected {
        text
    } else if hovered {
        mix(hue, text, 0.4)
    } else {
        mix(surface, hue, 0.5)
    };
    (fill, text, border)
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
    fn text_lowercases_app_text_but_keeps_marked_paths_exact() {
        let message = format!(
            "Project Changed Outside Hex At {}",
            super::verbatim_text::verbatim("/Users/Me/My Songs/Live Set.hex")
        );
        assert_eq!(
            text(message).as_ref(),
            "project changed outside hex at /Users/Me/My Songs/Live Set.hex"
        );
        assert_eq!(text("Plain Status").as_ref(), "plain status");
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
        assert_eq!(color(PaletteRole::ClipOutline), 0x000000);
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
    fn clip_outline_follows_custom_background() {
        let _restore = RestoreTheme(preferences());
        apply_preferences(hex_preferences::ThemePreferences {
            background: hex_preferences::RgbColor::new(0x80, 0x70, 0x60),
            ..hex_preferences::ThemePreferences::default()
        });
        assert_eq!(color(PaletteRole::ClipOutline), 0x16120e);
    }

    #[test]
    fn time_selection_is_a_near_white_band_that_keeps_clip_content_legible() {
        let _restore = RestoreTheme(preferences());
        apply_preferences(hex_preferences::ThemePreferences::default());
        let fill = time_selection_fill();
        let over = |base: u32| mix(base, fill, TIME_SELECTION_OPACITY);
        assert!(contrast_ratio(fill, 0xffffff) < 1.15);
        // The band meets the 3:1 non-text contrast over empty lanes while dark
        // clip labels and waveforms keep a readable margin through it.
        let background = color(PaletteRole::Background);
        assert!(contrast_ratio(over(background), background) >= 3.0);
        assert!(
            contrast_ratio(
                over(color(PaletteRole::ClipFill)),
                over(color(PaletteRole::ClipText))
            ) >= 2.5
        );
        assert!(
            contrast_ratio(
                over(color(PaletteRole::ClipFill)),
                over(color(PaletteRole::Waveform))
            ) >= 2.5
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
    #[test]
    fn inactive_clip_surfaces_are_neutral_and_labels_remain_legible() {
        let _restore = RestoreTheme(preferences());
        for background in [0x101010, 0xf5f0e6, 0x2a1438] {
            apply_preferences(hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(
                    (background >> 16) as u8,
                    (background >> 8) as u8,
                    background as u8,
                ),
                ..hex_preferences::ThemePreferences::default()
            });
            for selected in [false, true] {
                let expected = clip_activity_fill(
                    hex_project::ElementColor::Gray,
                    hex_project::ClipFlavor::Muted,
                    selected,
                    false,
                );
                assert_eq!(expected >> 16, expected & 0xff);
                assert_eq!((expected >> 8) & 0xff, expected & 0xff);
                assert!(contrast_ratio(expected, 0x000000) >= 4.5);
                for color in hex_project::ElementColor::ALL {
                    for flavor in [
                        hex_project::ClipFlavor::Muted,
                        hex_project::ClipFlavor::Accent,
                    ] {
                        assert_eq!(clip_activity_fill(color, flavor, selected, false), expected);
                        assert_eq!(
                            clip_activity_fill(color, flavor, selected, true),
                            clip_fill(color, flavor, selected)
                        );
                    }
                }
            }
            assert_ne!(
                clip_activity_fill(
                    hex_project::ElementColor::Gray,
                    hex_project::ClipFlavor::Muted,
                    false,
                    false
                ),
                clip_activity_fill(
                    hex_project::ElementColor::Gray,
                    hex_project::ClipFlavor::Muted,
                    true,
                    false
                )
            );
        }
    }

    #[test]
    fn clip_flavors_remain_distinct_selected_and_legible_across_theme_surfaces() {
        let _restore = RestoreTheme(preferences());
        let defaults = hex_preferences::ThemePreferences::default();
        let mut weakest = f32::INFINITY;
        for surface in [
            defaults.background.packed(),
            0x000000,
            0xf5f0e6,
            0x2a1438,
            0x808080,
        ] {
            apply_preferences(hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(
                    (surface >> 16) as u8,
                    (surface >> 8) as u8,
                    surface as u8,
                ),
                ..defaults
            });
            for hue in hex_project::ElementColor::ALL {
                let colors = element_colors(hue);
                for selected in [false, true] {
                    let regular = clip_fill(hue, hex_project::ClipFlavor::Muted, selected);
                    let accent = clip_fill(hue, hex_project::ClipFlavor::Accent, selected);
                    let separation = contrast_ratio(regular, accent);
                    assert_eq!(
                        accent,
                        if selected {
                            lighten(colors.accent, 22)
                        } else {
                            colors.accent
                        }
                    );
                    let channels = |color: u32| [color >> 16 & 255, color >> 8 & 255, color & 255];
                    let saturation = |color| {
                        let c = channels(color);
                        let max = *c.iter().max().unwrap() as f32;
                        let min = *c.iter().min().unwrap() as f32;
                        (max - min) / max
                    };
                    assert!(
                        channels(regular)
                            .into_iter()
                            .zip(channels(accent))
                            .all(|(muted, accent)| muted < accent),
                        "muted clips must be darker for {hue:?}"
                    );
                    assert!(
                        saturation(regular) > saturation(accent),
                        "muted clips must be more saturated for {hue:?}"
                    );
                    weakest = weakest.min(separation);
                    for (fill, cell_text) in
                        [(regular, colors.muted_text), (accent, colors.accent_text)]
                    {
                        // Actual body ink is black; Console uses the cached text
                        // role, and title strips derive text from their final fill.
                        assert!(contrast_ratio(fill, 0x000000) >= 4.5);
                        assert!(contrast_ratio(fill, cell_text) >= 4.5);
                        assert!(
                            contrast_ratio(fill, readable_text(fill, defaults.text.packed()))
                                >= 4.5
                        );
                    }
                    println!(
                        "palette_swatch surface={surface:06x} hue={hue:?} selected={selected} regular={regular:06x} accent={accent:06x} separation={separation:.4}"
                    );
                }
            }
        }
        // Both flavors retain black ink: the darkest red therefore has less
        // luminance separation than the former pale tint. Selection must keep
        // the dark, saturated flavor visibly distinct as well as readable.
        assert!(
            weakest >= 1.35,
            "weakest regular/accent separation: {weakest:.4}:1"
        );
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
