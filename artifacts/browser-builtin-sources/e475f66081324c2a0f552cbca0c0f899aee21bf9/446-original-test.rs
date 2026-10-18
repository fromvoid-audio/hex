use super::*;

#[gpui::test]
fn browser_builtin_source_actual_right_click_has_icon_grid(cx: &mut TestAppContext) {
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("builtin-browser.json");
    let (handle, _) = place_icon_preferences_window(cx, &directory, &config, &directory.0.join("recent.json"));
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let row = visual.debug_bounds("browser-place-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    assert!(visual.debug_bounds("context-menu-icon-grid").is_some(), "Music right-click must open existing bundled icon grid");
}
