//! Actual rendered paint through resize gestures, separate from divider thickness.
use super::*;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    PreferencesTestDirectory,
) {
    cx.update(super::super::super::bind_editor_keys);
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

fn palette(role: crate::theme::PaletteRole) -> gpui::Background {
    gpui::rgb(crate::theme::color(role)).into()
}

fn active(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
    selector: &str,
) -> bool {
    editor.read_with(visual, |view, _| {
        if selector == "browser-sidebar-resize" {
            view.browser.layout.sidebar_drag.is_some()
        } else {
            view.browser.resizing
        }
    })
}

#[gpui::test]
fn browser_resize_feedback_stays_cyan_outside_moving_hit_bounds(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for selector in ["browser-sidebar-resize", "browser-resize-handle"] {
        for release_inside in [false, true] {
            let start = visual.debug_bounds(selector).unwrap().center();
            let outside_y = visual.debug_bounds("browser-toolbar").unwrap().top() - px(8.0);
            visual.simulate_mouse_move(start, None, Modifiers::default());
            assert_eq!(
                visual.rendered_background(selector),
                Some(palette(crate::theme::PaletteRole::Secondary)),
                "idle hover must be cyan"
            );
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            for delta in [40.0, -300.0, 100.0] {
                let outside = point(start.x + px(delta), outside_y);
                visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
                assert!(active(&editor, visual, selector));
                assert_eq!(
                    visual.rendered_background(selector),
                    Some(palette(crate::theme::PaletteRole::Secondary)),
                    "active {selector} lost cyan after leaving actual moving hit bounds"
                );
                let other = if selector == "browser-sidebar-resize" {
                    "browser-resize-handle"
                } else {
                    "browser-sidebar-resize"
                };
                assert_eq!(
                    visual.rendered_background(other),
                    Some(palette(crate::theme::PaletteRole::Border)),
                    "only the active divider should latch cyan"
                );
                let current = visual.debug_bounds(selector).unwrap().center();
                visual.simulate_mouse_move(current, MouseButton::Left, Modifiers::default());
                assert_eq!(
                    visual.rendered_background(selector),
                    Some(palette(crate::theme::PaletteRole::Secondary))
                );
            }
            let end = if release_inside {
                visual.debug_bounds(selector).unwrap().center()
            } else {
                point(start.x, outside_y)
            };
            visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            assert!(!active(&editor, visual, selector));
            assert_eq!(
                visual.rendered_background(selector),
                Some(palette(if release_inside {
                    crate::theme::PaletteRole::Secondary
                } else {
                    crate::theme::PaletteRole::Border
                })),
                "release must return to actual hover/idle paint"
            );
            visual.simulate_mouse_move(point(start.x, outside_y), None, Modifiers::default());
            assert_eq!(
                visual.rendered_background(selector),
                Some(palette(crate::theme::PaletteRole::Border))
            );
        }
    }
}

#[gpui::test]
fn browser_resize_feedback_lost_left_button_cancels_without_stale_capture(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for selector in ["browser-sidebar-resize", "browser-resize-handle"] {
        let start = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let outside = point(
            start.x + px(30.0),
            visual.debug_bounds("browser-toolbar").unwrap().top() - px(8.0),
        );
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(outside, None, Modifiers::default());
        assert!(
            !active(&editor, visual, selector),
            "{selector} retained stale activity after actual Left button was no longer held"
        );
        assert_eq!(
            visual.rendered_background(selector),
            Some(palette(crate::theme::PaletteRole::Border))
        );
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_move(
            outside + point(px(35.0), px(0.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(
            visual.debug_bounds(selector).unwrap(),
            bounds,
            "a later unrelated left drag must not resume an expired resize"
        );
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    }
}

#[gpui::test]
fn browser_resize_feedback_close_cancels_both_existing_gestures(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for selector in ["browser-sidebar-resize", "browser-resize-handle"] {
        let start = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        assert!(active(&editor, visual, selector));
        visual.simulate_keystrokes("cmd-alt-b");
        editor.read_with(visual, |view, _| {
            assert!(!view.browser.open);
            assert!(!view.browser.resizing && view.browser.layout.sidebar_drag.is_none());
        });
        visual.simulate_keystrokes("cmd-alt-b");
        let outside = point(
            start.x,
            visual.debug_bounds("browser-toolbar").unwrap().top() - px(8.0),
        );
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_move(
            outside + point(px(35.0), px(0.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(visual.debug_bounds(selector).unwrap(), bounds);
        assert_eq!(
            visual.rendered_background(selector),
            Some(palette(crate::theme::PaletteRole::Border))
        );
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    }
}
