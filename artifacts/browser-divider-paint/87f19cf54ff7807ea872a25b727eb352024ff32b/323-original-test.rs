//! Distinguish painted separator width from the unchanged resize capture gutter.
use super::*;

fn fixture(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>, PreferencesTestDirectory) {
    let directory = PreferencesTestDirectory::new();
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &directory.0.join("browser.json"), &directory.0.join("recent.json"));
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    (handle, editor, directory)
}

#[gpui::test]
fn browser_divider_paint_is_thin_without_shrinking_capture_bounds(cx: &mut TestAppContext) {
    let (handle, _editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for (handle, line) in [("browser-sidebar-resize", "browser-sidebar-resize-line"), ("browser-resize-handle", "browser-resize-line")] {
        let gutter = visual.debug_bounds(handle).unwrap();
        assert_eq!(gutter.size.width, px(4.0));
        let painted = if visual.rendered_background(handle).is_some() { gutter }
            else { visual.debug_bounds(line).expect("transparent gutter must retain a visible thin line") };
        assert!(painted.size.width >= px(1.0) && painted.size.width <= px(2.0), "{handle} paints {}px across its {}px capture area", painted.size.width, gutter.size.width);
        assert!(visual.rendered_background(line).is_some());
    }
}
