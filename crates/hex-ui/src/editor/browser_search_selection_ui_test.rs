//! Actual input/UTF-16 selection checks; never a native focus-bug acceptance claim.
use super::*;
use gpui::EntityInputHandler;
use std::ops::Range;

fn selection(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) -> Range<usize> {
    visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            input.selected_text_range(false, window, cx).unwrap().range
        })
    })
}

fn caret_point(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
    offset: usize,
) -> gpui::Point<gpui::Pixels> {
    let bounds = visual.debug_bounds("browser-search").unwrap();
    visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            input
                .bounds_for_range(offset..offset, bounds, window, cx)
                .unwrap()
                .center()
        })
    })
}

fn workers_and_ticks(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
    directory: &std::path::Path,
) {
    editor.update(visual, |view, cx| {
        view.refresh_sample_catalog(cx);
        view.request_browser_scan(directory.to_owned(), cx);
    });
    visual.run_until_parked();
    for _ in 0..4 {
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(16));
        visual.run_until_parked();
    }
}

#[gpui::test]
fn search_focus_click_selects_query_and_replaces_after_real_workers(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("kick hats");
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.focus.focus(window);
            cx.notify();
        })
    });
    let point = caret_point(&editor, visual, 4);
    visual.simulate_click(point, Modifiers::default());
    assert_eq!(
        selection(&editor, visual),
        0..9,
        "first focus click must select the entire query"
    );
    workers_and_ticks(&editor, visual, &directory.0);
    assert_eq!(selection(&editor, visual), 0..9);
    visual.simulate_input("snare");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "snare");
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert_eq!(view.browser.playback_generation, 0);
    });
    visual.simulate_keystrokes("cmd-f");
    assert_eq!(
        selection(&editor, visual),
        0..5,
        "Command-F selects all while already focused"
    );
    visual.simulate_input("loop");
    // Results use the same underlying FocusHandle; entering text mode is still a focus entry.
    visual.simulate_keystrokes("down");
    let point = caret_point(&editor, visual, 2);
    visual.simulate_click(point, Modifiers::default());
    assert_eq!(selection(&editor, visual), 0..4);
    visual.simulate_input("hat");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "hat")
    });
}

#[gpui::test]
fn search_mouse_drag_keeps_partial_selection_and_first_focus_drag_wins(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("kick hats");
    for entering in [false, true] {
        if entering {
            visual.update(|window, cx| {
                editor.update(cx, |view, cx| {
                    view.focus.focus(window);
                    cx.notify();
                })
            });
        }
        let start = caret_point(&editor, visual, 1);
        let end = caret_point(&editor, visual, 4);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        assert_eq!(
            selection(&editor, visual),
            if entering { 0..9 } else { 1..1 }
        );
        visual.update(|window, cx| {
            let input = editor.read(cx).browser.search.clone();
            input.update(cx, |input, cx| {
                assert_eq!(
                    input.text_for_range(0..9, &mut None, window, cx).as_deref(),
                    Some("kick hats")
                )
            });
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        assert_eq!(
            selection(&editor, visual),
            1..4,
            "held drag selects a substring"
        );
        visual.simulate_mouse_move(start, MouseButton::Left, Modifiers::default());
        assert_eq!(
            selection(&editor, visual),
            1..1,
            "drag can return to its anchor"
        );
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        workers_and_ticks(&editor, visual, &directory.0);
        assert_eq!(
            selection(&editor, visual),
            1..4,
            "release/late worker must not select all again"
        );
    }
    let category = visual.debug_bounds("browser-section-samples").unwrap();
    visual.simulate_click(category.center(), Modifiers::default());
    visual.simulate_resize(gpui::size(px(880.0), px(540.0)));
    visual.run_until_parked();
    assert_eq!(
        selection(&editor, visual),
        1..4,
        "category/layout changes retain text selection"
    );
    visual.simulate_input("X");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "kx hats")
    });
}

#[gpui::test]
fn search_keyboard_selection_keeps_combining_graphemes_whole(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("İ🎛");
    visual.simulate_keystrokes("cmd-a right left");
    assert_eq!(selection(&editor, visual), 2..2);
    visual.simulate_keystrokes("left");
    assert_eq!(
        selection(&editor, visual),
        0..0,
        "caret must cross i + combining dot as one grapheme"
    );
}

#[gpui::test]
fn search_selection_clipboard_ime_empty_and_long_query_keep_text_ownership(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("kick hats");
    let caret = caret_point(&editor, visual, 1);
    visual.simulate_click(caret, Modifiers::default());
    assert_eq!(
        selection(&editor, visual),
        1..1,
        "already focused click positions caret"
    );
    visual.simulate_keystrokes("shift-right shift-right shift-right cmd-c end cmd-v");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "kick hatsick")
    });
    visual.simulate_keystrokes("cmd-a cmd-x");
    assert_eq!(selection(&editor, visual), 0..0);
    visual.simulate_keystrokes("cmd-v");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "kick hatsick")
    });
    visual.simulate_keystrokes("cmd-f");
    visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "İ🎛", Some(1..1), window, cx);
            assert_eq!(input.marked_text_range(window, cx), Some(0..4));
        });
    });
    visual.simulate_keystrokes("cmd-f");
    assert_eq!(selection(&editor, visual), 0..4);
    visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            assert_eq!(input.marked_text_range(window, cx), None)
        });
    });
    visual.simulate_input("hat");
    visual.simulate_keystrokes("cmd-f backspace");
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.focus.focus(window);
            cx.notify();
        })
    });
    let empty = caret_point(&editor, visual, 0);
    visual.simulate_mouse_down(empty, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        empty + point(px(1.0), px(0.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(empty, MouseButton::Left, Modifiers::default());
    assert_eq!(selection(&editor, visual), 0..0);
    let long = "a".repeat(300) + "🎛";
    visual.simulate_input(&long);
    visual.simulate_keystrokes("cmd-f");
    assert_eq!(selection(&editor, visual), 0..302);
    let bounds = visual.debug_bounds("browser-search").unwrap();
    // Selection follows a held pointer outside the field and releases outside safely.
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(bounds.left() - px(10_000.0), bounds.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(bounds.right() + px(10_000.0), bounds.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let outside = point(bounds.right() + px(10_000.0), bounds.center().y);
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    let released = selection(&editor, visual);
    assert!(released.end <= 302);
    visual.simulate_event(gpui::MouseMoveEvent {
        position: bounds.center(),
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    assert_eq!(
        selection(&editor, visual),
        released,
        "hover cannot mutate released selection"
    );
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("x");
    visual.simulate_keystrokes("cmd-shift-t");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "x");
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert_eq!(view.browser.playback_generation, 0);
    });
}
