//! Distinguish painted separator width from the unchanged resize capture gutter.
use super::*;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    PreferencesTestDirectory,
) {
    let directory = PreferencesTestDirectory::new();
    let (handle, editor) = place_icon_preferences_window(
        cx,
        &directory,
        &directory.0.join("browser.json"),
        &directory.0.join("recent.json"),
    );
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    (handle, editor, directory)
}

#[gpui::test]
fn browser_divider_paint_is_thin_without_shrinking_capture_bounds(cx: &mut TestAppContext) {
    let (handle, _editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for (handle, line) in [
        ("browser-sidebar-resize", "browser-sidebar-resize-line"),
        ("browser-resize-handle", "browser-resize-line"),
    ] {
        let gutter = visual.debug_bounds(handle).unwrap();
        assert_eq!(gutter.size.width, px(4.0));
        let painted = if visual.rendered_background(handle).is_some() {
            gutter
        } else {
            visual
                .debug_bounds(line)
                .expect("transparent gutter must retain a visible thin line")
        };
        assert!(
            painted.size.width >= px(1.0) && painted.size.width <= px(2.0),
            "{handle} paints {}px across its {}px capture area",
            painted.size.width,
            gutter.size.width
        );
        assert!(visual.rendered_background(line).is_some());
    }
}

fn assert_thin(visual: &mut VisualTestContext, handle: &'static str, line: &'static str) {
    let gutter = visual.debug_bounds(handle).unwrap();
    let paint = visual.debug_bounds(line).unwrap();
    assert_eq!(gutter.size.width, px(4.0));
    assert_eq!(paint.size.width, px(1.0));
    assert_eq!(paint.left(), gutter.left());
    assert_eq!(paint.top(), gutter.top());
    assert_eq!(paint.bottom(), gutter.bottom());
    assert!(paint.right() < gutter.right());
    assert!(
        visual.rendered_background(handle).is_none(),
        "capture gutter must not paint a broad quad"
    );
    assert!(visual.rendered_background(line).is_some());
}

#[gpui::test]
fn browser_divider_paint_padding_and_line_both_resize_with_thin_hover_and_drag(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let secondary: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Secondary)).into();
    for open in [false, true] {
        if open {
            let toggle = visual.debug_bounds("browser-control-⋮").unwrap();
            visual.simulate_click(toggle.center(), Modifiers::default());
            assert!(visual.debug_bounds("browser-tag-editor").is_some());
        }
        for width in [124.0, 23.0] {
            let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
            let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
            let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
            visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            assert_eq!(
                visual.debug_bounds("browser-sidebar").unwrap().size.width,
                px(width)
            );
            for (handle, line) in [
                ("browser-sidebar-resize", "browser-sidebar-resize-line"),
                ("browser-resize-handle", "browser-resize-line"),
            ] {
                for x in [0.5, 3.5] {
                    assert_thin(visual, handle, line);
                    let gutter = visual.debug_bounds(handle).unwrap();
                    let point = point(gutter.left() + px(x), gutter.center().y);
                    visual.simulate_mouse_move(point, None, Modifiers::default());
                    assert!(matches!(
                        visual.rendered_cursor_style(),
                        Some(gpui::CursorStyle::ResizeLeftRight | gpui::CursorStyle::ResizeColumn)
                    ));
                    assert_eq!(
                        visual.rendered_background(line),
                        Some(secondary),
                        "hover in transparent padding must highlight only the line"
                    );
                    assert_thin(visual, handle, line);
                    let before_sidebar = visual.debug_bounds("browser-sidebar").unwrap();
                    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
                    editor.read_with(visual, |view, _| {
                        assert!(if handle == "browser-sidebar-resize" {
                            view.browser.layout.sidebar_drag.is_some()
                        } else {
                            view.browser.resizing
                        });
                    });
                    let target = point + gpui::point(px(16.0), px(0.0));
                    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
                    if handle == "browser-sidebar-resize" {
                        assert!(
                            visual.debug_bounds("browser-sidebar").unwrap().size.width
                                > before_sidebar.size.width,
                            "transparent padding must actually resize the picker"
                        );
                    } else {
                        assert!(
                            visual.debug_bounds(handle).unwrap().left() > gutter.left(),
                            "transparent padding must actually resize the outer browser"
                        );
                    }
                    assert_eq!(visual.rendered_background(line), Some(secondary));
                    assert_thin(visual, handle, line);
                    let outside = gpui::point(
                        target.x,
                        visual.debug_bounds("browser-toolbar").unwrap().top() - px(8.0),
                    );
                    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
                    assert_eq!(
                        visual.rendered_background(line),
                        Some(secondary),
                        "PR324 active color must survive pointer departure"
                    );
                    assert_thin(visual, handle, line);
                    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
                    assert_thin(visual, handle, line);
                    editor.read_with(visual, |view, _| {
                        assert!(
                            !view.browser.resizing && view.browser.layout.sidebar_drag.is_none()
                        )
                    });
                }
            }
        }
    }
}
