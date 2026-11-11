use super::*;

struct RestoreTheme(hex_preferences::ThemePreferences);
impl Drop for RestoreTheme {
    fn drop(&mut self) {
        theme::apply_preferences(self.0);
    }
}

fn contrast(first: gpui::Rgba, second: gpui::Rgba) -> f32 {
    let luminance = |c: gpui::Rgba| {
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
    };
    let (a, b) = (luminance(first), luminance(second));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn actual_meter_label_styles_meet_contrast_for_valid_saved_themes() {
    let _restore = RestoreTheme(theme::preferences());
    for colors in [
        hex_preferences::ThemePreferences::default(),
        hex_preferences::ThemePreferences {
            background: hex_preferences::RgbColor::new(48, 48, 48),
            primary: hex_preferences::RgbColor::new(121, 121, 121),
            text: hex_preferences::RgbColor::new(255, 255, 255),
            secondary: hex_preferences::RgbColor::new(255, 255, 255),
        },
    ] {
        assert!(theme::colors_are_legible(colors));
        theme::apply_preferences(colors);
        for input in [
            MeterInput::Offline,
            MeterInput::Stopped,
            MeterInput::Fault,
            MeterInput::Last {
                left: 0.5,
                right: 1.0,
            },
            MeterInput::Last {
                left: 2.0,
                right: f32::NAN,
            },
        ] {
            let mut component = render(input);
            assert_eq!(
                component.style().background.as_ref().unwrap().color(),
                Some(gpui::solid_background(rgb(theme::color(
                    theme::PaletteRole::Surface
                ))))
            );
            let mut label = field(input);
            let foreground = label.text_style().as_ref().unwrap().color.unwrap().to_rgb();
            assert!(contrast(foreground, rgb(theme::color(theme::PaletteRole::Surface))) >= 4.5);
            assert_eq!(theme::preferences(), colors);
        }
    }
}

#[cfg(feature = "ui-test-support")]
#[path = "tests/rendered.rs"]
mod rendered;
