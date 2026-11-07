use super::*;
use gpui::Focusable;

struct RestoreTheme(hex_preferences::ThemePreferences);
impl Drop for RestoreTheme {
    fn drop(&mut self) {
        theme::apply_preferences(self.0);
    }
}

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn light_palette() -> hex_preferences::ThemePreferences {
    hex_preferences::ThemePreferences {
        background: hex_preferences::RgbColor::new(225, 229, 238),
        text: hex_preferences::RgbColor::new(24, 27, 34),
        primary: hex_preferences::RgbColor::new(145, 25, 45),
        secondary: hex_preferences::RgbColor::new(16, 70, 145),
    }
}

#[gpui::test]
fn device_theme_refresh_preserves_canonical_and_preview_numeric_drafts(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    theme::apply_preferences(hex_preferences::ThemePreferences::default());
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let revision = state.revision;
    let project = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let parameter = hex_lattice_contract::ParameterId::from_static(33);
    let key = ParameterKey { device, parameter };
    let canonical = editor.read_with(cx, |view, _| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&key] else {
            panic!("envelope numeric control")
        };
        entity.clone()
    });
    // Inline readouts use the same semantic style regardless of device identity.
    enter_typed_value(&mut visual, device, parameter, "invalid");
    let canonical_focus = cx.update(|app| canonical.focus_handle(app));
    theme::apply_preferences(light_palette());
    editor.update(cx, |_, cx| cx.notify());
    visual.run_until_parked();
    editor.read_with(cx, |view, app| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&key] else {
            panic!("retained envelope numeric control")
        };
        assert_eq!(entity.entity_id(), canonical.entity_id());
        assert!(entity.read(app).is_editing());
        assert_eq!(entity.read(app).display_text(), "invalid");
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, project);
    });
    assert!(visual.update(|window, _| canonical_focus.is_focused(window)));
    assert_eq!(
        visual.rendered_background(selector(format!(
            "device-{}-parameter-33-value",
            device.value()
        ))),
        Some(gpui::Background::from(gpui::rgb(
            theme::device_style(theme::DeviceStyleRole::Readout).background
        )))
    );
    assert_eq!(
        visual.rendered_background(selector(format!("operator-detail-{}", device.value()))),
        Some(gpui::Background::from(gpui::rgb(
            device_layout_style().background
        )))
    );
    visual.simulate_keystrokes("escape");
    theme::apply_preferences(hex_preferences::ThemePreferences::default());
    editor.update(cx, |_, cx| cx.notify());
    visual.run_until_parked();
    let field = operator_panel::preview::Field::PitchEnvelope;
    let preview = editor.read_with(cx, |view, _| {
        view.device_panel.operator_previews[&device].controls[&field]
            .entity
            .clone()
    });
    let value_selector = selector(format!(
        "operator-preview-{}-{}-value",
        device.value(),
        field as usize
    ));
    let value_bounds = visual
        .debug_bounds(value_selector)
        .expect("preview value field");
    visual.simulate_click(value_bounds.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid preview");
    visual.run_until_parked();
    let preview_focus = cx.update(|app| preview.focus_handle(app));
    theme::apply_preferences(light_palette());
    editor.update(cx, |_, cx| cx.notify());
    visual.run_until_parked();
    editor.read_with(cx, |view, app| {
        let retained = &view.device_panel.operator_previews[&device].controls[&field].entity;
        assert_eq!(retained.entity_id(), preview.entity_id());
        assert!(retained.read(app).is_editing());
        assert_eq!(retained.read(app).display_text(), "invalid preview");
        assert_eq!(view.state.as_ref().unwrap().project, project);
        assert_eq!(view.worker_requests_pending, 0);
    });
    assert!(visual.update(|window, _| preview_focus.is_focused(window)));
    assert_eq!(
        visual.rendered_background(value_selector),
        Some(gpui::Background::from(gpui::rgb(
            control_style().background
        )))
    );
    visual.simulate_keystrokes("escape");
}

#[gpui::test]
fn device_theme_refresh_keeps_open_selector_and_navigation_state(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    theme::apply_preferences(hex_preferences::ThemePreferences::default());
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .entry(device)
            .or_default()
            .page = operator_panel::OperatorPage::Routing;
        cx.notify();
    });
    visual.run_until_parked();
    let parameter = hex_lattice_contract::ParameterId::from_static(601);
    let key = ParameterKey { device, parameter };
    let choice = editor.read_with(cx, |view, _| {
        let ParameterControl::Choice { entity, .. } = &view.device_panel.controls[&key] else {
            panic!("voices selector")
        };
        entity.clone()
    });
    let bounds = visual
        .debug_bounds(selector(format!("device-{}-parameter-601", device.value())))
        .unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let popup = selector(format!("device-{}-parameter-601-popup", device.value()));
    assert!(visual.debug_bounds(popup).is_some());
    theme::apply_preferences(light_palette());
    editor.update(cx, |_, cx| cx.notify());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let ParameterControl::Choice { entity, .. } = &view.device_panel.controls[&key] else {
            panic!("retained voices selector")
        };
        assert_eq!(entity.entity_id(), choice.entity_id());
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Routing
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    assert!(
        visual.debug_bounds(popup).is_some(),
        "theme refresh leaves popup open"
    );
    assert_eq!(
        visual.rendered_background(selector(format!(
            "operator-preview-{}-toggle-0-indicator",
            device.value()
        ))),
        Some(gpui::Background::from(gpui::rgb(control_style().accent))),
        "retained view-only toggle receives the active palette"
    );
    assert_eq!(
        visual.rendered_background(popup),
        Some(gpui::Background::from(gpui::rgb(control_style().surface)))
    );
    assert!(visual.update(|window, app| choice.focus_handle(app).is_focused(window)));
    visual.simulate_keystrokes("escape");
}
