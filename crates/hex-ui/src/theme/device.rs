//! Product-owned device roles. Widgets receive resolved colors, never identities.
use super::{PaletteRole, color, readable_text};
use hex_widgets::{ActionStyle, ControlStyle, DeviceFrameStyle};

/// Visual purpose, independent of device type, layout and parameter identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceStyleRole {
    Control,
    Layout,
    Display,
    Readout,
    ErrorDisplay,
}

/// Resolve the active product palette for production panels and widget galleries.
/// Updating these tokens does not replace retained controls or interaction state.
pub(crate) fn device_style(role: DeviceStyleRole) -> ControlStyle {
    let surface = color(PaletteRole::Surface);
    let display = color(PaletteRole::Raised);
    let text = color(PaletteRole::Text);
    let mut style = ControlStyle {
        background: color(PaletteRole::Background),
        surface,
        raised: display,
        rail: color(PaletteRole::Border),
        border: color(PaletteRole::Border),
        text: readable_text(surface, text),
        muted: color(PaletteRole::Muted),
        accent: color(PaletteRole::Secondary),
        accent_text: readable_text(color(PaletteRole::Secondary), text),
        numeric_fill: color(PaletteRole::Border),
        hover_text: readable_text(color(PaletteRole::Hover), text),
        focus_marker: text,
        dial_rail: color(PaletteRole::Border),
        hover: color(PaletteRole::Hover),
        selection: color(PaletteRole::Secondary),
        invalid: color(PaletteRole::Primary),
        font_size: 9.0,
        text_case: hex_widgets::TextCase::Lowercase,
    };
    match role {
        DeviceStyleRole::Control => {}
        DeviceStyleRole::Layout => {
            // An idle header is the same surround as the body. Recessed displays
            // and grouped rails remain distinct without an identity-specific skin.
            style.background = display;
            style.raised = surface;
            style.rail = color(PaletteRole::Background);
            style.border = surface;
        }
        DeviceStyleRole::Display | DeviceStyleRole::Readout | DeviceStyleRole::ErrorDisplay => {
            style.background = display;
            style.rail = color(PaletteRole::Border);
            style.text = readable_text(display, text);
            if role == DeviceStyleRole::Readout {
                style.text = readable_text(display, style.accent);
            } else if role == DeviceStyleRole::ErrorDisplay {
                style.text = readable_text(display, color(PaletteRole::Primary));
            }
        }
    }
    style.numeric_fill = [0.35, 0.30, 0.25, 0.20, 0.15, 0.10, 0.0]
        .into_iter()
        .map(|opacity| super::mix(style.rail, style.accent, opacity))
        .find(|fill| super::contrast_ratio(*fill, style.text) >= 4.5)
        .unwrap_or(style.rail);
    // Text selection sits behind field text on `raised`; keep that text readable.
    style.selection = [0.4, 0.3, 0.2, 0.1]
        .into_iter()
        .map(|opacity| super::mix(style.raised, style.accent, opacity))
        .find(|fill| super::contrast_ratio(*fill, style.text) >= 4.5)
        .unwrap_or(style.raised);
    style
}

/// One selected-header policy for every instrument, effect and return shell.
/// Header paint is isolated from the neutral body and uses a readable foreground.
/// The reserved shell stroke stays neutral and visible in every selection state.
pub(crate) fn device_frame_style(selected: bool) -> DeviceFrameStyle {
    let mut style = device_style(DeviceStyleRole::Layout);
    style.border = color(PaletteRole::Border);
    let frame = style.with_device_selection(selected);
    if selected {
        frame.with_header_colors(style.accent, readable_text(style.accent, style.text))
    } else {
        frame
    }
}

/// Stable musical identities a/b/c/d, shared by badges and routing diagrams.
/// These are identity cues, not neutral surfaces or editable palette roles.
/// Foregrounds are resolved against them rather than assuming dark text.
pub(crate) fn oscillator_identity_color(index: usize) -> Option<u32> {
    [0xe2cf25, 0x22bcb5, 0x6374e8, 0xf18428].get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RestoreTheme(hex_preferences::ThemePreferences);

    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            super::super::apply_preferences(self.0);
        }
    }

    #[test]
    fn device_roles_follow_contrasting_palettes_and_keep_readable_foregrounds() {
        let _restore = RestoreTheme(super::super::preferences());
        let palettes = [
            hex_preferences::ThemePreferences::default(),
            hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(225, 229, 238),
                text: hex_preferences::RgbColor::new(24, 27, 34),
                primary: hex_preferences::RgbColor::new(145, 25, 45),
                secondary: hex_preferences::RgbColor::new(16, 70, 145),
            },
        ];
        let mut layouts = Vec::new();
        for palette in palettes {
            super::super::apply_preferences(palette);
            let layout = device_style(DeviceStyleRole::Layout);
            let display = device_style(DeviceStyleRole::Display);
            let readout = device_style(DeviceStyleRole::Readout);
            let error = device_style(DeviceStyleRole::ErrorDisplay);
            assert!(super::super::contrast_ratio(error.background, error.text) >= 4.5);
            let control = device_style(DeviceStyleRole::Control);
            assert_eq!(layout.surface, color(PaletteRole::Surface));
            assert_eq!(layout.raised, layout.surface);
            assert_eq!(display.background, layout.background);
            assert_ne!(layout.surface, layout.rail);
            assert_ne!(layout.rail, layout.background);
            assert!(super::super::contrast_ratio(layout.surface, layout.text) >= 4.5);
            assert!(super::super::contrast_ratio(display.background, display.text) >= 4.5);
            assert!(super::super::contrast_ratio(readout.background, readout.text) >= 4.5);
            assert_ne!(control.text, control.muted);
            assert_eq!(control.hover, color(PaletteRole::Hover));
            assert_eq!(control.focus_marker, palette.text.packed());
            assert_eq!(control.accent, palette.secondary.packed());
            assert_eq!(control.invalid, palette.primary.packed());
            assert_ne!(control.selection, control.raised);
            assert!(super::super::contrast_ratio(control.selection, control.text) >= 4.5);
            assert_eq!(oscillator_identity_color(0), Some(0xe2cf25));
            assert_eq!(oscillator_identity_color(3), Some(0xf18428));
            assert_eq!(oscillator_identity_color(4), None);
            layouts.push(layout);
        }
        assert_ne!(layouts[0], layouts[1]);
    }
}

/// Semantic action intent; device identity never selects header metrics/colors.
#[derive(Clone, Copy)]
pub(crate) enum DeviceActionRole {
    Normal,
    Destructive,
    Enabled,
    Bypassed,
}

pub(crate) fn device_header_action_style(selected: bool, role: DeviceActionRole) -> ActionStyle {
    // Power is a canonical device state, independent of the selected header.
    // Keep the accent on hover too, so focus/hover cannot suggest a bypass edit.
    let layout = device_style(DeviceStyleRole::Layout);
    let background = match role {
        DeviceActionRole::Enabled => color(PaletteRole::Primary),
        DeviceActionRole::Bypassed => color(PaletteRole::Border),
        _ if selected => layout.accent,
        _ => layout.surface,
    };
    let preferred = color(match role {
        DeviceActionRole::Normal | DeviceActionRole::Enabled => PaletteRole::Text,
        DeviceActionRole::Bypassed => PaletteRole::Muted,
        DeviceActionRole::Destructive => PaletteRole::Primary,
    });
    let hover_background = if matches!(role, DeviceActionRole::Enabled) {
        super::mix(background, color(PaletteRole::Text), 0.12)
    } else {
        color(PaletteRole::Hover)
    };
    ActionStyle {
        background,
        text: readable_text(background, preferred),
        hover_background,
        hover_text: Some(readable_text(hover_background, preferred)),
    }
}

/// Existing enabled-red/bypassed-gray palette tokens for the boolean header
/// indicator. Selection and transient input never replace the acknowledged color.
pub(crate) fn device_enable_colors() -> (u32, u32) {
    (
        device_header_action_style(false, DeviceActionRole::Enabled).background,
        device_header_action_style(false, DeviceActionRole::Bypassed).background,
    )
}

pub(crate) fn device_segment_style(selected: bool) -> ActionStyle {
    let background = color(if selected {
        PaletteRole::Secondary
    } else {
        PaletteRole::Raised
    });
    let hover_background = if selected {
        background
    } else {
        color(PaletteRole::Hover)
    };
    ActionStyle {
        background,
        text: readable_text(background, color(PaletteRole::Text)),
        hover_background,
        hover_text: Some(readable_text(hover_background, color(PaletteRole::Text))),
    }
}

pub(crate) fn device_identity_style(index: usize, enabled: bool, selected: bool) -> ControlStyle {
    let mut style = device_style(DeviceStyleRole::Control);
    style.surface = if enabled {
        oscillator_identity_color(index).unwrap_or(style.accent)
    } else {
        device_style(DeviceStyleRole::Layout).background
    };
    style.text = if enabled {
        readable_text(style.surface, style.text)
    } else {
        style.muted
    };
    style.border = if selected {
        color(PaletteRole::Text)
    } else {
        color(PaletteRole::Border)
    };
    style
}

#[cfg(test)]
mod action_tests {
    use super::*;

    #[test]
    fn header_and_segment_actions_remain_readable_on_selected_and_hover_surfaces() {
        let previous = super::super::preferences();
        for palette in [
            previous,
            hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(225, 229, 238),
                text: hex_preferences::RgbColor::new(24, 27, 34),
                primary: hex_preferences::RgbColor::new(145, 25, 45),
                secondary: hex_preferences::RgbColor::new(16, 70, 145),
            },
        ] {
            super::super::apply_preferences(palette);
            let controls = device_style(DeviceStyleRole::Control);
            assert!(super::super::contrast_ratio(controls.accent, controls.accent_text) >= 4.5);
            assert!(super::super::contrast_ratio(controls.hover, controls.hover_text) >= 4.5);
            // Filled numeric rails paint accent at 35% over the rail; text must
            // remain readable both above the fill and above the empty rail.
            let filled = controls.numeric_fill;
            assert!(super::super::contrast_ratio(controls.rail, controls.text) >= 4.5);
            assert!(super::super::contrast_ratio(filled, controls.text) >= 4.5);
            let workspace = super::super::action_style();
            assert!(super::super::contrast_ratio(workspace.background, workspace.text) >= 4.5);
            assert!(
                super::super::contrast_ratio(
                    workspace.hover_background,
                    workspace.hover_text.unwrap_or(workspace.text)
                ) >= 4.5
            );
            for selected in [false, true] {
                for role in [
                    DeviceActionRole::Normal,
                    DeviceActionRole::Destructive,
                    DeviceActionRole::Enabled,
                    DeviceActionRole::Bypassed,
                ] {
                    let style = device_header_action_style(selected, role);
                    assert!(super::super::contrast_ratio(style.background, style.text) >= 4.5);
                    assert!(
                        super::super::contrast_ratio(
                            style.hover_background,
                            style.hover_text.unwrap_or(style.text)
                        ) >= 4.5
                    );
                }
                let segment = device_segment_style(selected);
                if selected {
                    assert_eq!(segment.hover_background, segment.background);
                }
                assert!(super::super::contrast_ratio(segment.background, segment.text) >= 4.5);
                assert!(
                    super::super::contrast_ratio(
                        segment.hover_background,
                        segment.hover_text.unwrap_or(segment.text)
                    ) >= 4.5
                );
            }
        }
        super::super::apply_preferences(previous);
    }
}

/// Input remains subdued; output uses readable foreground behind the EQ curve.
pub(crate) fn device_analyzer_style() -> hex_widgets::ResponseGraphAnalyzerStyle {
    hex_widgets::ResponseGraphAnalyzerStyle {
        input: color(PaletteRole::Muted),
        output: readable_text(color(PaletteRole::Raised), color(PaletteRole::Text)),
    }
}
