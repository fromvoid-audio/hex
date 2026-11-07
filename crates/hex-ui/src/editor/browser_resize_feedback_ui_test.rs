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

// PR323 moves the same feedback paint into a thin child; retain all original
// gesture, state, cursor/capture and color assertions against the actual paint.
fn paint_selector(handle: &str) -> &'static str {
    if handle == "browser-sidebar-resize" {
        "browser-sidebar-resize-line"
    } else {
        "browser-resize-line"
    }
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
                visual.rendered_background(paint_selector(selector)),
                Some(palette(crate::theme::PaletteRole::Secondary)),
                "idle hover must be cyan"
            );
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            for delta in [40.0, -300.0, 100.0] {
                let outside = point(start.x + px(delta), outside_y);
                visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
                assert!(active(&editor, visual, selector));
                assert_eq!(
                    visual.rendered_background(paint_selector(selector)),
                    Some(palette(crate::theme::PaletteRole::Secondary)),
                    "active {selector} lost cyan after leaving actual moving hit bounds"
                );
                let other = if selector == "browser-sidebar-resize" {
                    "browser-resize-handle"
                } else {
                    "browser-sidebar-resize"
                };
                assert_eq!(
                    visual.rendered_background(paint_selector(other)),
                    Some(palette(crate::theme::PaletteRole::Border)),
                    "only the active divider should latch cyan"
                );
                let current = visual.debug_bounds(selector).unwrap().center();
                visual.simulate_mouse_move(current, MouseButton::Left, Modifiers::default());
                assert_eq!(
                    visual.rendered_background(paint_selector(selector)),
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
                visual.rendered_background(paint_selector(selector)),
                Some(palette(if release_inside {
                    crate::theme::PaletteRole::Secondary
                } else {
                    crate::theme::PaletteRole::Border
                })),
                "release must return to actual hover/idle paint"
            );
            visual.simulate_mouse_move(point(start.x, outside_y), None, Modifiers::default());
            assert_eq!(
                visual.rendered_background(paint_selector(selector)),
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
            visual.rendered_background(paint_selector(selector)),
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
            visual.rendered_background(paint_selector(selector)),
            Some(palette(crate::theme::PaletteRole::Border))
        );
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    }
}

#[gpui::test]
fn browser_resize_feedback_open_editor_and_compact_picker_keep_gesture_paint(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for open in [false, true] {
        if open {
            let toggle = visual.debug_bounds("browser-control-⋮").unwrap();
            visual.simulate_click(toggle.center(), Modifiers::default());
            assert!(visual.debug_bounds("browser-tag-editor").is_some());
        }
        for width in [124.0, 23.0] {
            let before = visual.debug_bounds("browser-sidebar").unwrap();
            let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
            let target = divider.center() + point(px(width) - before.size.width, px(0.0));
            visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            assert_eq!(
                visual.debug_bounds("browser-sidebar").unwrap().size.width,
                px(width)
            );
            for selector in ["browser-sidebar-resize", "browser-resize-handle"] {
                let bounds = visual.debug_bounds(selector).unwrap();
                assert_eq!(
                    bounds.size.width,
                    px(4.0),
                    "paint/hit thickness belongs to separate2707"
                );
                visual.simulate_mouse_down(
                    bounds.center(),
                    MouseButton::Left,
                    Modifiers::default(),
                );
                let outside = point(
                    bounds.center().x,
                    visual.debug_bounds("browser-toolbar").unwrap().top() - px(8.0),
                );
                visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
                assert!(active(&editor, visual, selector));
                assert_eq!(
                    visual.rendered_background(paint_selector(selector)),
                    Some(palette(crate::theme::PaletteRole::Secondary))
                );
                visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
                assert!(!active(&editor, visual, selector));
                assert_eq!(
                    visual.rendered_background(paint_selector(selector)),
                    Some(palette(crate::theme::PaletteRole::Border))
                );
            }
        }
    }
    editor.read_with(visual, |view, _| {
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
    });
}

#[path = "browser/resize_capture_tests.rs"]
mod capture_tests;
