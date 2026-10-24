//! Real shaped-name input, raw clipboard and UTF-16 IME geometry; no filesystem jobs.
use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};

fn config() -> TextInputConfig {
    let style = crate::ControlStyle {
        background: 0x202020,
        surface: 0x303030,
        raised: 0x383838,
        rail: 0x282828,
        border: 0x505050,
        text: 0xe0e0e0,
        muted: 0xaaaaaa,
        accent: 0x55ddee,
        accent_text: 0x202020,
        numeric_fill: 0x31515a,
        hover_text: 0xe0e0e0,
        focus_marker: 0xaaaaaa,
        dial_rail: 0,
        hover: 0x404040,
        font_size: 12.0,
        text_case: crate::TextCase::Lowercase,
    };
    let action_style = crate::ActionStyle {
        background: style.surface,
        text: style.text,
        hover_background: style.hover,
        hover_text: None,
    };
    TextInputConfig {
        id: "clip-name-input".into(),
        kind: TextInputKind::Value,
        style,
        action_style,
        font_family: "test".into(),
        placeholder: "".into(),
        clear_name: "clear".into(),
        clear_glyph: "×".into(),
        max_bytes: 1024,
    }
}
fn name(content: String, cx: &mut Context<TextInput>) -> TextInput {
    let mut input = TextInput::new(cx, config());
    input.set_text(&content, cx);
    input.select_all_text(cx);
    input
}
struct NameCaseRoot {
    input: Entity<TextInput>,
}
impl gpui::Render for NameCaseRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let input = self.input.clone();
        div()
            .w(px(400.0))
            .key_context("hex_track_name")
            .child(input)
    }
}

fn fixture(cx: &mut TestAppContext, original: &str) -> (gpui::AnyWindowHandle, Entity<TextInput>) {
    cx.update(super::bind_text_input_keys);
    let handle = cx.add_window(|window, cx| {
        let input = cx.new(|cx| name(original.to_owned(), cx));
        input.update(cx, |input, _| input.focus(window));
        NameCaseRoot { input }
    });
    let input = handle
        .root(cx)
        .unwrap()
        .read_with(cx, |view, _| view.input.clone());
    (*handle, input)
}

#[gpui::test]
fn name_case_paste_keyboard_clipboard_and_reopening_preserve_source(cx: &mut TestAppContext) {
    let original = "İA🎛 /Samples/Kick.WAV";
    let (handle, input) = fixture(cx, original);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    input.read_with(visual, |input, _| {
        assert_eq!(input.content, original);
        assert_eq!(input.painted_text(), Some(original.to_lowercase().as_str()));
    });
    visual.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(original.into())));
    visual.simulate_keystrokes("cmd-a cmd-v home right");
    visual.simulate_input("Z");
    visual.run_until_parked();
    let expected = "İZA🎛 /Samples/Kick.WAV";
    input.read_with(visual, |input, _| {
        assert_eq!(input.content, expected);
        assert_eq!(input.painted_text(), Some(expected.to_lowercase().as_str()));
    });
    visual.simulate_keystrokes("cmd-a cmd-x cmd-v cmd-a cmd-c");
    visual.run_until_parked();
    visual.update(|_, cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), expected));
    input.read_with(visual, |input, _| assert_eq!(input.content, expected));
    // Reopen with exactly the submitted/persisted case, without normalizing it.
    let (reopened_handle, reopened) = fixture(cx, expected);
    let reopened_visual = &mut VisualTestContext::from_window(reopened_handle, cx);
    reopened_visual.run_until_parked();
    reopened.read_with(reopened_visual, |input, _| {
        assert_eq!(input.content, expected);
        assert_eq!(input.painted_text(), Some(expected.to_lowercase().as_str()));
    });
}

#[gpui::test]
fn name_case_marked_unicode_and_scrolled_caret_use_original_utf16(cx: &mut TestAppContext) {
    let (handle, input) = fixture(cx, "Original");
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "İA🎹", Some(1..1), window, cx);
        })
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            assert_eq!(input.content, "İA🎹");
            assert_eq!(input.painted_text(), Some("i\u{307}a🎹"));
            assert_eq!(input.marked_text_range(window, cx), Some(0..4));
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                1..1
            );
            let bounds = input.last_bounds.unwrap();
            let caret = input.bounds_for_range(1..1, bounds, window, cx).unwrap();
            assert_eq!(
                input.character_index_for_point(caret.center(), window, cx),
                Some(1)
            );
            // A real mouse click at that position must keep the same source boundary.
            assert_eq!(input.index_for_point(caret.center()), "İ".len());
            let mut actual = None;
            assert_eq!(
                input
                    .text_for_range(0..4, &mut actual, window, cx)
                    .as_deref(),
                Some("İA🎹")
            );
        })
    });
    visual.simulate_keystrokes("cmd-a");
    let long = format!("{}END", "İ".repeat(80));
    visual.simulate_input(&long);
    visual.run_until_parked();
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            assert_eq!(input.content, long);
            assert_eq!(input.painted_text(), Some(long.to_lowercase().as_str()));
            let end = long.encode_utf16().count();
            let bounds = input.last_bounds.unwrap();
            let caret = input
                .bounds_for_range(end..end, bounds, window, cx)
                .unwrap();
            assert!(caret.left() >= bounds.left() && caret.right() <= bounds.right());
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                end..end
            );
            assert!(input.marked_text_range(window, cx).is_none());
        })
    });
}

#[gpui::test]
fn name_case_double_click_and_composition_keep_raw_unicode_selection(cx: &mut TestAppContext) {
    let original = "İAe\u{301}🎛";
    let (handle, input) = fixture(cx, original);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let at = visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input
                .bounds_for_range(1..1, input.last_bounds.unwrap(), window, cx)
                .unwrap()
                .center()
        })
    });
    for (count, expected) in [(1, 1..1), (3, 0..6), (1, 1..1), (1, 0..6)] {
        for click_count in [count, count + 1] {
            visual.simulate_event(gpui::MouseDownEvent {
                position: at,
                button: gpui::MouseButton::Left,
                modifiers: gpui::Modifiers::default(),
                click_count,
                first_mouse: false,
            });
            visual.simulate_event(gpui::MouseUpEvent {
                position: at,
                button: gpui::MouseButton::Left,
                modifiers: gpui::Modifiers::default(),
                click_count,
            });
        }
        visual.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert_eq!(input.content, original);
                assert_eq!(
                    input.selected_text_range(false, window, cx).unwrap().range,
                    expected
                );
                assert!(
                    input.content.is_char_boundary(input.selected.start)
                        && input.content.is_char_boundary(input.selected.end)
                );
            })
        });
    }
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(Some(0..6), "İ🎛", Some(0..1), window, cx)
        })
    });
    visual.run_until_parked();
    let at = visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input
                .bounds_for_range(1..1, input.last_bounds.unwrap(), window, cx)
                .unwrap()
                .center()
        })
    });
    for click_count in [1, 2] {
        visual.simulate_event(gpui::MouseDownEvent {
            position: at,
            button: gpui::MouseButton::Left,
            modifiers: gpui::Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        visual.simulate_event(gpui::MouseUpEvent {
            position: at,
            button: gpui::MouseButton::Left,
            modifiers: gpui::Modifiers::default(),
            click_count,
        });
    }
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            assert_eq!(input.content, "İ🎛");
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                0..3
            );
            assert_eq!(input.marked_text_range(window, cx), None);
        })
    });
    visual.simulate_keystrokes("cmd-c");
    visual.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("İ🎛")
        )
    });
}

#[gpui::test]
fn search_limits_invalid_ranges_and_disabled_input_preserve_text(cx: &mut TestAppContext) {
    cx.update(super::bind_text_input_keys);
    let handle = cx.add_window(|window, cx| {
        let input = cx.new(|cx| {
            TextInput::new(
                cx,
                TextInputConfig {
                    kind: TextInputKind::Search,
                    id: "query-fixture".into(),
                    max_bytes: 6,
                    ..config()
                },
            )
        });
        input.update(cx, |input, _| input.focus(window));
        NameCaseRoot { input }
    });
    let input = handle.read_with(cx, |r, _| r.input.clone()).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    visual.simulate_input("AB");
    visual.simulate_input("1234567");
    input.read_with(&visual, |i, _| assert_eq!(i.text(), "ab1234"));
    input.update(&mut visual, |i, cx| i.set_text("ab", cx));
    visual.update(|window, cx| {
        input.update(cx, |i, cx| {
            i.replace_text_in_range(None, "1234567", window, cx)
        })
    });
    input.read_with(&visual, |i, _| assert_eq!(i.text(), "ab"));
    visual.update(|window, cx| {
        input.update(cx, |i, cx| {
            i.replace_text_in_range(Some(std::ops::Range { start: 2, end: 1 }), "x", window, cx)
        })
    });
    visual.update(|window, cx| {
        input.update(cx, |i, cx| {
            i.replace_and_mark_text_in_range(
                None,
                "x",
                Some(std::ops::Range { start: 2, end: 1 }),
                window,
                cx,
            )
        })
    });
    input.read_with(&visual, |i, _| assert_eq!(i.text(), "ab"));
    input.update(&mut visual, |i, cx| i.set_enabled(false, cx));
    visual.run_until_parked();
    visual.simulate_input("x");
    visual.simulate_keystrokes("backspace cmd-x");
    input.read_with(&visual, |i, _| {
        assert_eq!(i.text(), "ab");
        assert!(!i.has_committed_search_text());
    });
    input.update(&mut visual, |i, cx| i.set_enabled(true, cx));
    visual.run_until_parked();
    let clear = visual.debug_bounds("query-fixture-clear").unwrap();
    visual.simulate_click(clear.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    input.read_with(&visual, |i, _| assert!(i.text().is_empty()));
}
