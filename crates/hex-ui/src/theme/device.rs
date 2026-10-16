//! Product-owned device roles. Widgets receive resolved colors, never identities.
use super::{PaletteRole, color, readable_text};
use hex_widgets::{ControlStyle, DeviceFrameStyle};

/// Visual purpose, independent of device type, layout and parameter identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceStyleRole {
    Control,
    Layout,
    Display,
    Readout,
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
        focus_marker: text,
        dial_rail: color(PaletteRole::Border),
        hover: color(PaletteRole::Hover),
        font_size: 9.0,
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
        DeviceStyleRole::Display | DeviceStyleRole::Readout => {
            style.background = display;
            style.rail = color(PaletteRole::Border);
            style.text = readable_text(display, text);
            if role == DeviceStyleRole::Readout {
                style.text = readable_text(display, style.accent);
            }
        }
    }
    style
}

/// One selected-header policy for every instrument, effect and return shell.
/// Header paint is isolated from the neutral body and uses a readable foreground.
pub(crate) fn device_frame_style(selected: bool) -> DeviceFrameStyle {
    let style = device_style(DeviceStyleRole::Layout);
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
            assert_eq!(oscillator_identity_color(0), Some(0xe2cf25));
            assert_eq!(oscillator_identity_color(3), Some(0xf18428));
            assert_eq!(oscillator_identity_color(4), None);
            layouts.push(layout);
        }
        assert_ne!(layouts[0], layouts[1]);
    }
}
