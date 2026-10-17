use super::*;

struct RestoreTheme(hex_preferences::ThemePreferences);
impl Drop for RestoreTheme {
    fn drop(&mut self) {
        theme::apply_preferences(self.0);
    }
}

fn luminance(color: gpui::Rgba) -> f32 {
    let linear = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

#[test]
fn device_panel_title_render_style_meets_text_contrast_without_changing_palette() {
    let _restore = RestoreTheme(theme::preferences());
    let colors = hex_preferences::ThemePreferences {
        background: hex_preferences::RgbColor::new(48, 48, 48),
        primary: hex_preferences::RgbColor::new(121, 121, 121),
        text: hex_preferences::RgbColor::new(255, 255, 255),
        secondary: hex_preferences::RgbColor::new(255, 255, 255),
    };
    assert!(
        theme::colors_are_legible(colors),
        "valid existing palette policy"
    );
    theme::apply_preferences(colors);
    for warning in [false, true] {
        // Inspect the actual Div style supplied to both rendered owner headers.
        let mut label = title_label("finish the active device value first".into(), warning);
        let foreground = label.text_style().as_ref().unwrap().color.unwrap().to_rgb();
        let background = rgb(theme::color(theme::PaletteRole::Surface));
        let first = luminance(foreground);
        let second = luminance(background);
        let contrast = (first.max(second) + 0.05) / (first.min(second) + 0.05);
        assert!(
            contrast >= 4.5,
            "warning={warning}, actual rendered title contrast={contrast}"
        );
        assert_eq!(theme::preferences(), colors);
    }
}

#[test]
fn full_metadata_cache_keeps_unavailable_parameters_and_unsupported_errors_without_entities() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "metadata".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    for type_id in ["hex.utility", "hex.test.unsupported"] {
        let state = if type_id == "hex.utility" {
            hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static(type_id),
            )
            .unwrap()
        } else {
            NativeDeviceState::new(
                hex_lattice_contract::DeviceTypeId::from_static(type_id),
                1,
                Vec::new(),
            )
            .unwrap()
        };
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap();
    }
    let devices = editor.project().track(track).unwrap().devices();
    let mut panel = DevicePanelState::default();
    for device in devices {
        cache_device_metadata(&mut panel, device, NativeDeviceClass::Effect);
    }
    assert_eq!(panel.views.len(), 2);
    assert!(panel.controls.is_empty());
    assert!(panel.views[&devices[1].id()].2.is_err());
    let expected =
        hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, devices[0].state())
            .unwrap();
    let cached = panel.views[&devices[0].id()].2.as_ref().unwrap();
    assert_eq!(cached.parameters.len(), expected.parameters.len());
    for (first, second) in cached.parameters.iter().zip(&expected.parameters) {
        assert_eq!(first.descriptor, second.descriptor);
        assert_eq!(first.value, second.value);
        assert_eq!(first.enabled, second.enabled);
    }
    // Availability is metadata, not a reason to discard a descriptor.
    panel
        .views
        .get_mut(&devices[0].id())
        .unwrap()
        .2
        .as_mut()
        .unwrap()
        .parameters[0]
        .enabled = false;
    cache_device_metadata(&mut panel, &devices[0], NativeDeviceClass::Effect);
    assert!(!panel.views[&devices[0].id()].2.as_ref().unwrap().parameters[0].enabled);
    assert!(panel.controls.is_empty());
}
