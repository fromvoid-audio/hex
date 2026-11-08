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

/// Workspace outlines stay visible above graphite surfaces without changing the
/// darker borders used by meters and controls. Custom palette roles still apply.
pub(crate) fn workspace_panel_style() -> hex_widgets::PanelStyle {
    hex_widgets::PanelStyle {
        surface: color(PaletteRole::Surface),
        border: mix(
            color(PaletteRole::Background),
            color(PaletteRole::Text),
            0.15,
        ),
    }
}

/// Shared product colors for all sample-level meters; widgets own no palette.
pub(crate) fn meter_colors() -> hex_widgets::MeterColors {
    hex_widgets::MeterColors {
        signal: color(PaletteRole::Secondary),
        near_peak: color(PaletteRole::Primary),
        clipping: 0xe34b4b,
    }
}

/// Quiet track/Return signal wells, separate from editable gain controls.
/// The live signal stays desaturated blue through the meter's +6 dB headroom.
pub(crate) fn track_meter_style() -> hex_widgets::ControlStyle {
    let rail = mix(color(PaletteRole::Background), 0x000000, 0.8);
    let text = color(PaletteRole::Text);
    hex_widgets::ControlStyle {
        rail,
        accent: mix(text, color(PaletteRole::Secondary), 0.35),
        border: mix(rail, text, 0.18),
        text: mix(rail, text, 0.45),
        ..control_style()
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

/// Color for a locked Arrangement play-start marker.
///
/// Preserve the theme accent where it separates from both the ordinary marker
/// and the ruler surface; otherwise choose the better of two fixed fallbacks.
pub(crate) fn locked_time_position_marker_color() -> u32 {
    locked_marker_color_for(
        color(PaletteRole::Surface),
        time_position_marker_color(),
        color(PaletteRole::Secondary),
    )
}

fn locked_marker_color_for(surface: u32, normal: u32, accent: u32) -> u32 {
    let score = |candidate| {
        (contrast_ratio(candidate, surface) / 3.0).min(contrast_ratio(candidate, normal) / 1.5)
    };
    if score(accent) >= 1.0 {
        return accent;
    }
    let light = 0x00c8ff;
    let dark = 0x0057c2;
    if score(light) >= score(dark) {
        light
    } else {
        dark
    }
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

/// Pale cyan distinguishes explicit track membership on the graphite workspace.
/// Keep an explicitly customized secondary color instead of imposing that hue.
pub(crate) fn track_selection_fill() -> u32 {
    let secondary = color(PaletteRole::Secondary);
    if secondary
        == hex_preferences::ThemePreferences::default()
            .secondary
            .packed()
    {
        0xa4ccd7
    } else {
        secondary
    }
}

/// Track membership paints below clips, independently of the brighter time band.
pub(crate) const TRACK_SELECTION_OPACITY: f32 = 0.28;

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
    /// Recording is red independently of the editable interaction highlight.
    Recording,
    /// Validation must remain recognizable when the highlight is changed.
    Error,
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
    clip_waveform_inks: [ClipWaveformInks; 10],
    inactive_waveform_inks: [u32; 2],
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
    error: u32,
    waveform: u32,
}

impl ThemeTokens {
    fn from_preferences(colors: hex_preferences::ThemePreferences) -> Self {
        let background = colors.background.packed();
        let text = colors.text.packed();
        let primary = colors.primary.packed();
        let elements = hex_project::ElementColor::ALL
            .map(|identity| resolve_element_colors(identity, lighten(background, 5)));
        let muted_clip_fills = elements.map(|colors| muted_clip_fill(colors.accent));
        let surface = lighten(background, 5);
        let event_fill = mix(background, text, 0.15);
        let clip_waveform_inks = std::array::from_fn(|index| {
            ClipWaveformInks::resolve(
                elements[index].accent,
                muted_clip_fills[index],
                surface,
                event_fill,
            )
        });
        let neutral = elements[hex_project::ElementColor::Gray as usize].accent;
        let inactive_waveform_inks = [false, true].map(|selected| {
            waveform_ink_for_background(neutral, arrangement_clip_body_fill(0x888888, selected))
        });
        Self {
            colors,
            muted_clip_fills,
            clip_waveform_inks,
            inactive_waveform_inks,
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
            error: [0xe35c5c, 0xb32828, 0x7a1111, 0x4c0000, 0xffcccc]
                .into_iter()
                .find(|red| contrast_ratio(*red, background) >= 4.5)
                .unwrap_or_else(|| contrast_text(background, 0xe35c5c)),
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
            PaletteRole::Recording => 0xc33737,
            PaletteRole::Error => self.error,
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

pub(crate) fn contrast_ratio(first: u32, second: u32) -> f32 {
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
    // Frame r188's eight hue families use the bright clip shade as the accent;
    // the existing surface mix and contrast-bound darkening resolve the other tiers.
    let hue = match identity {
        ElementColor::Gray => 0xa4a4a4,
        ElementColor::Red => 0xce7967,
        ElementColor::Orange => 0xd79659,
        ElementColor::Amber => 0xc5ae5b,
        ElementColor::Olive => 0x95b56b,
        ElementColor::Green => 0x6bb68b,
        ElementColor::Cyan => 0x6bb3aa,
        ElementColor::Blue => 0x809fca,
        ElementColor::Violet => 0xb780ad,
        ElementColor::Rose => 0xd37c94,
    };
    // Neutral markers stay neutral even when a custom surface is tinted.
    let surface = if identity == ElementColor::Gray {
        let channel = ((surface >> 16 & 255) + (surface >> 8 & 255) + (surface & 255)) / 3;
        channel * 0x010101
    } else {
        surface
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
/// Actual backdrop used by the clip's shared waveform painter. Paint context
/// never participates in the waveform geometry cache or saved project state.
#[derive(Clone, Copy)]
pub(crate) enum ClipWaveformBackground {
    Arrangement {
        flavor: hex_project::ClipFlavor,
        selected: bool,
    },
    Events,
    Source,
}

/// Shared fade-handle feedback; capture and receipt owners choose the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FadeHandleState {
    Idle,
    Hover,
    Active,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FadeHandleStyle {
    pub fill: u32,
    pub stroke: u32,
}

pub(crate) fn fade_handle_style(state: FadeHandleState) -> FadeHandleStyle {
    let (color, opacity) = match state {
        FadeHandleState::Idle => {
            return FadeHandleStyle {
                fill: 0x00000020,
                stroke: 0x000000cc,
            };
        }
        FadeHandleState::Hover => (color(PaletteRole::Secondary), 0x59),
        FadeHandleState::Active => (color(PaletteRole::Primary), 0xbf),
    };
    FadeHandleStyle {
        fill: (color << 8) | opacity,
        stroke: (color << 8) | 0xff,
    }
}

#[derive(Clone, Copy)]
struct ClipWaveformInks {
    muted: [u32; 2],
    accent: [u32; 2],
    source: u32,
    events: u32,
}
impl ClipWaveformInks {
    fn resolve(hue: u32, muted: u32, source: u32, events: u32) -> Self {
        Self {
            muted: [false, true].map(|selected| {
                waveform_ink_for_background(hue, arrangement_clip_body_fill(muted, selected))
            }),
            accent: [false, true].map(|selected| {
                waveform_ink_for_background(hue, arrangement_clip_body_fill(hue, selected))
            }),
            source: waveform_ink_for_background(hue, source),
            events: waveform_ink_for_background(hue, events),
        }
    }
}

fn waveform_ink_for_background(hue: u32, background: u32) -> u32 {
    let dark = mix(0x000000, hue, 0.18);
    let light = mix(hue, 0xffffff, 0.80);
    if contrast_ratio(dark, background) >= contrast_ratio(light, background) {
        dark
    } else {
        light
    }
}

/// Passive fragments already carry their resolved clip fill, without an owner ID.
/// Their bounded paint-only tone stays in that hue and changes no sample geometry.
pub(crate) fn waveform_ink_for_fill(fill: u32) -> u32 {
    waveform_ink_for_background(fill, fill)
}

/// Constant-time palette query. Resolve relative-luminance contrast once per
/// theme, including the selected Arrangement body lift. Inactive ink stays
/// neutral; the existing waveform opacity remains with its presentation owner.
pub(crate) fn clip_waveform_ink(
    identity: hex_project::ElementColor,
    background: ClipWaveformBackground,
    active: bool,
) -> u32 {
    ACTIVE_THEME.with(|theme| {
        let theme = theme.get();
        let hue = if active {
            identity
        } else {
            hex_project::ElementColor::Gray
        };
        let inks = theme.clip_waveform_inks[hue as usize];
        match background {
            ClipWaveformBackground::Arrangement { flavor, selected } => {
                if !active {
                    return theme.inactive_waveform_inks[usize::from(selected)];
                }
                let values = match flavor {
                    hex_project::ClipFlavor::Muted => inks.muted,
                    hex_project::ClipFlavor::Accent => inks.accent,
                };
                values[usize::from(selected)]
            }
            ClipWaveformBackground::Events => inks.events,
            ClipWaveformBackground::Source => inks.source,
        }
    })
}

/// Darken the track hue by removing neutral light, increasing saturation without
/// shifting its hue. Resolve once per theme; retain 4.5:1 contrast for the existing
/// note ink and Console foregrounds, including historical gray.
fn muted_clip_fill(accent: u32) -> u32 {
    (0..=40)
        .rev()
        .map(|amount| darken(accent, amount))
        .find(|fill| contrast_ratio(*fill, 0x000000) >= 4.5)
        .unwrap_or(accent)
}

/// Black shading above a clip's fade envelope, at 40 percent opacity. Shared
/// canonical and projected clip layers use it independently of curve/handle ink.
pub(crate) const CLIP_FADE_SHADE: u32 = 0x00000066;

const CLIP_SELECTION_LIFT: u32 = 22;

/// Additional white mixed into selected Arrangement bodies after the ordinary
/// selection lift. Header strips and Console cells retain their own fills.
const SELECTED_CLIP_BODY_WHITENING: f32 = 0.35;

/// Derive the Arrangement content surface from its unselected header color.
/// This changes no geometry and requires no project traversal.
pub(crate) fn arrangement_clip_body_fill(header: u32, selected: bool) -> u32 {
    if selected {
        mix(
            lighten(header, CLIP_SELECTION_LIFT),
            0xffffff,
            SELECTED_CLIP_BODY_WHITENING,
        )
    } else {
        header
    }
}

/// Shared clip palette for Console cells, drag previews and unselected titles.
/// The persisted `Muted` flavor is darker and more saturated; Accent keeps the track hue.
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
    if selected {
        lighten(base, CLIP_SELECTION_LIFT)
    } else {
        base
    }
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

#[derive(Clone, Copy)]
pub(crate) struct SectionFocusHatch {
    pub angle_degrees: f32,
    pub spacing: f32,
    pub alpha: u8,
    pub stroke_device_px: f32,
    pub outline_device_px: f32,
}

pub(crate) const SECTION_FOCUS_HATCH: SectionFocusHatch = SectionFocusHatch {
    angle_degrees: 45.0,
    spacing: 8.0,
    alpha: 0x59,
    stroke_device_px: 1.0,
    outline_device_px: 2.0,
};

pub(crate) fn section_focus_ink(identity: hex_project::ElementColor) -> u32 {
    (element_colors(identity).accent << 8) | u32::from(SECTION_FOCUS_HATCH.alpha)
}

/// Restrained Main section surfaces share the clip body selection lift and
/// primary outline. Unselected identity and hover remain unchanged.
pub(crate) fn section_colors(
    identity: hex_project::ElementColor,
    selected: bool,
    hovered: bool,
) -> (u32, u32, u32) {
    let surface = color(PaletteRole::Surface);
    let hue = element_colors(identity).muted;
    let base = mix(surface, hue, if hovered { 0.44 } else { 0.30 });
    let fill = arrangement_clip_body_fill(base, selected);
    let text = contrast_text(fill, color(PaletteRole::Text));
    let border = if selected {
        color(PaletteRole::Primary)
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
    fn sections_share_clip_selection_lift_outline_and_legible_labels() {
        for identity in hex_project::ElementColor::ALL {
            for hovered in [false, true] {
                let normal = section_colors(identity, false, hovered);
                let selected = section_colors(identity, true, hovered);
                assert_eq!(selected.0, arrangement_clip_body_fill(normal.0, true));
                assert_eq!(selected.2, color(PaletteRole::Primary));
                assert!(contrast_ratio(selected.0, selected.1) >= 4.5);
                assert_eq!(
                    section_focus_ink(identity) >> 8,
                    element_colors(identity).accent
                );
            }
        }
    }

    #[test]
    fn fade_handle_feedback_stays_distinct_when_highlights_match() {
        let _restore = RestoreTheme(preferences());
        let defaults = preferences();
        apply_preferences(hex_preferences::ThemePreferences {
            secondary: defaults.primary,
            ..defaults
        });
        let idle = fade_handle_style(FadeHandleState::Idle);
        let hover = fade_handle_style(FadeHandleState::Hover);
        let active = fade_handle_style(FadeHandleState::Active);
        assert_ne!(idle, hover);
        assert_ne!(hover, active);
        assert_ne!(idle, active);
        assert_eq!(hover.stroke >> 8, defaults.primary.packed());
        assert_eq!(active.stroke >> 8, defaults.primary.packed());
        assert!((hover.fill & 255) < (active.fill & 255));
    }

    #[test]
    fn waveform_hues_contrast_with_each_actual_clip_backdrop() {
        for background in [0x000000, 0x101010, 0x2a1438, 0x808080, 0xf5f0e6, 0xffffff] {
            let colors = hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(
                    (background >> 16) as u8,
                    (background >> 8) as u8,
                    background as u8,
                ),
                ..hex_preferences::ThemePreferences::default()
            };
            let tokens = ThemeTokens::from_preferences(colors);
            for identity in hex_project::ElementColor::ALL {
                let index = identity as usize;
                let inks = tokens.clip_waveform_inks[index];
                let hue = tokens.elements[index].accent;
                for (ink, fill) in [(inks.source, tokens.surface), (inks.events, tokens.clip)] {
                    assert!(
                        contrast_ratio(ink, fill) >= 3.0,
                        "{identity:?}: {ink:06x}/{fill:06x}"
                    );
                    assert_ne!(ink, 0);
                    assert_ne!(ink, 0xffffff);
                }
                for selected in [false, true] {
                    for (inks, fill) in [
                        (inks.muted, tokens.muted_clip_fills[index]),
                        (inks.accent, hue),
                    ] {
                        let fill = arrangement_clip_body_fill(fill, selected);
                        let ink = inks[usize::from(selected)];
                        assert!(
                            contrast_ratio(ink, fill) >= 3.0,
                            "{identity:?}: {ink:06x}/{fill:06x}"
                        );
                        assert!(contrast_ratio(waveform_ink_for_fill(fill), fill) >= 3.0);
                    }
                    let ink = tokens.inactive_waveform_inks[usize::from(selected)];
                    assert_eq!(ink >> 16, (ink >> 8) & 255);
                    assert_eq!(ink >> 16, ink & 255);
                    assert!(
                        contrast_ratio(ink, arrangement_clip_body_fill(0x888888, selected)) >= 3.0
                    );
                }
            }
        }
    }

    #[test]
    fn locked_marker_color_avoids_matching_normal_and_surface() {
        for (surface, normal) in [(0x000000, 0xffffff), (0xffffff, 0x000000)] {
            for accent in [normal, surface] {
                let locked = locked_marker_color_for(surface, normal, accent);
                assert_ne!(locked, normal);
                assert_ne!(locked, surface);
                assert!(contrast_ratio(locked, normal) >= 1.5);
                assert!(contrast_ratio(locked, surface) >= 3.0);
            }
        }
        assert_eq!(
            locked_marker_color_for(0x000000, 0xffffff, 0x00c8ff),
            0x00c8ff
        );
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
        assert_eq!(track_selection_fill(), 0xa4ccd7);
        assert_eq!(workspace_panel_style().surface, 0x202020);
        assert_eq!(workspace_panel_style().border, 0x363636);
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
    fn graphite_defaults_are_neutral_and_recording_errors_ignore_custom_highlights() {
        let _restore = RestoreTheme(preferences());
        let defaults = hex_preferences::ThemePreferences::default();
        apply_preferences(defaults);
        for role in [
            PaletteRole::Background,
            PaletteRole::Surface,
            PaletteRole::Raised,
            PaletteRole::Hover,
            PaletteRole::Border,
            PaletteRole::Text,
            PaletteRole::Muted,
        ] {
            let rgb = color(role);
            assert_eq!((rgb >> 16) & 255, (rgb >> 8) & 255);
            assert_eq!((rgb >> 8) & 255, rgb & 255);
        }
        assert_eq!(color(PaletteRole::Primary), 0x3b95df);
        assert_eq!(color(PaletteRole::Secondary), 0x75baff);
        let recording = color(PaletteRole::Recording);
        let error = color(PaletteRole::Error);
        assert!(contrast_ratio(recording, color(PaletteRole::Background)) >= 3.0);
        assert!(contrast_ratio(error, color(PaletteRole::Raised)) >= 4.5);
        let custom = hex_preferences::ThemePreferences {
            primary: hex_preferences::RgbColor::new(0x90, 0xdd, 0x60),
            ..defaults
        };
        apply_preferences(custom);
        assert_eq!(preferences(), custom);
        assert_eq!(color(PaletteRole::Recording), recording);
        assert_eq!(color(PaletteRole::Error), error);
        assert_ne!(color(PaletteRole::Primary), recording);
        assert_eq!(control_style().invalid, error);
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
                    if hue == hex_project::ElementColor::Gray {
                        assert_eq!(saturation(regular), 0.0);
                        assert_eq!(saturation(accent), 0.0);
                    } else {
                        assert!(
                            saturation(regular) > saturation(accent),
                            "muted clips must be more saturated for {hue:?}"
                        );
                    }
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
    fn neutral_markers_do_not_inherit_custom_surface_tint() {
        for surface in [0x000000, 0x202020, 0xf5f0e6, 0x2a1438] {
            let colors = resolve_element_colors(hex_project::ElementColor::Gray, surface);
            for fill in [colors.muted, colors.accent] {
                assert_eq!(fill >> 16 & 255, fill >> 8 & 255);
                assert_eq!(fill >> 8 & 255, fill & 255);
                assert!(contrast_ratio(fill, 0x000000) >= 4.5);
            }
        }
    }

    #[test]
    fn both_flavors_keep_ink_legible_and_identities_distinct() {
        let mut hues = std::collections::BTreeSet::new();
        for identity in hex_project::ElementColor::ALL {
            let colors = element_colors(identity);
            assert!(hues.insert(colors.accent));
            assert_ne!(colors.muted, colors.accent);
            assert!(contrast_ratio(colors.muted, colors.muted_text) >= 4.5);
            assert!(contrast_ratio(colors.accent, colors.accent_text) >= 4.5);
            // Note thumbnails retain neutral black ink.
            assert!(contrast_ratio(colors.muted, 0x000000) >= 4.5);
            assert!(
                contrast_ratio(resolve_element_colors(identity, 0x000000).muted, 0x000000) >= 4.5
            );
        }
        assert_eq!(hues.len(), 10);
    }
}
