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
        selection: 0x3350a0,
        invalid: 0xff554c,
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
fn selection_highlight_paints_the_injected_style_color(cx: &mut TestAppContext) {
    const SELECTION: u32 = 0x2a6f3b;
    let handle = cx.add_window(|window, cx| {
        let input = cx.new(|cx| {
            let mut config = config();
            config.style.selection = SELECTION;
            let mut input = TextInput::new(cx, config);
            input.set_text("kick", cx);
            input.select_all_text(cx);
            input
        });
        input.update(cx, |input, _| input.focus(window));
        NameCaseRoot { input }
    });
    let visual = &mut VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    let fills = visual.rendered_filled_quads("clip-name-input");
    assert!(
        fills
            .iter()
            .any(|(_, fill)| *fill == gpui::Background::from(gpui::rgb(SELECTION))),
        "selection must follow ControlStyle::selection: {fills:?}"
    );
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

#[gpui::test]
fn block_caret_reserves_a_character_cell_at_empty_and_scrolled_end_positions(
    cx: &mut TestAppContext,
) {
    for text in [
        "",
        "İA🎛 /Samples/a-very-long-name-that-scrolls-beyond-the-field.WAV",
    ] {
        let (handle, input) = fixture(cx, text);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_keystrokes("end");
        visual.run_until_parked();
        visual.update(|window, cx| {
            let bounds = input.read(cx).last_bounds.expect("painted input");
            let mut element = TextInputElement {
                input: input.clone(),
            };
            let state = element.prepaint(None, None, bounds, &mut (), window, cx);
            let caret = state.cursor.expect("collapsed caret").bounds;
            assert!(caret.size.width > px(1.0));
            assert_eq!(caret.size.height, bounds.size.height);
            assert!(caret.left() >= bounds.left() && caret.right() <= bounds.right());
        });
        if !text.is_empty() {
            visual.simulate_keystrokes("cmd-a");
            visual.run_until_parked();
            visual.update(|window, cx| {
                let bounds = input.read(cx).last_bounds.expect("painted input");
                let mut element = TextInputElement {
                    input: input.clone(),
                };
                assert!(
                    element
                        .prepaint(None, None, bounds, &mut (), window, cx)
                        .cursor
                        .is_none()
                );
            });
        }
    }
}

struct CaptureSlot(Entity<TextInput>);
impl gpui::Render for CaptureSlot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_col().child(self.0.clone())
    }
}
struct CaptureRoot {
    input: Entity<TextInput>,
    slot: Entity<CaptureSlot>,
    actions: std::rc::Rc<std::cell::Cell<usize>>,
}
impl gpui::Render for CaptureRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let down = self.actions.clone();
        let up = self.actions.clone();
        let wheel = self.actions.clone();
        let mut slot = div().w(px(160.)).h(px(24.));
        div()
            .size_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(160.))
                    .h(px(24.))
                    .child(gpui::AnyView::from(self.slot.clone()).cached(slot.style().clone())),
            )
            .child(
                div()
                    .id("outside")
                    .absolute()
                    .left(px(240.))
                    .top_0()
                    .w(px(80.))
                    .h(px(80.))
                    .cursor(CursorStyle::Crosshair)
                    .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                    .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                    .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
            )
    }
}
fn capture_fixture(
    cx: &mut TestAppContext,
    kind: TextInputKind,
) -> (
    gpui::AnyWindowHandle,
    Entity<TextInput>,
    std::rc::Rc<std::cell::Cell<usize>>,
) {
    cx.update(super::bind_text_input_keys);
    let actions = std::rc::Rc::new(std::cell::Cell::new(0));
    let handle = cx.add_window(|_, cx| {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, TextInputConfig { kind, ..config() });
            input.set_text("İA🎛 /Samples/Kick.WAV", cx);
            input
        });
        let slot = cx.new(|_| CaptureSlot(input.clone()));
        CaptureRoot {
            input,
            slot,
            actions: actions.clone(),
        }
    });
    let input = handle.read_with(cx, |r, _| r.input.clone()).unwrap();
    (*handle, input, actions)
}

#[gpui::test]
fn text_selection_owns_buttons_wheel_cursor_and_release_without_breaking_clear(
    cx: &mut TestAppContext,
) {
    for kind in [TextInputKind::Search, TextInputKind::Value] {
        let (handle, input, actions) = capture_fixture(cx, kind);
        let mut v = VisualTestContext::from_window(handle, cx);
        v.simulate_resize(gpui::size(px(400.), px(100.)));
        v.run_until_parked();
        let start = v.debug_bounds("clip-name-input").unwrap().center();
        let outside = point(px(260.), px(10.));
        v.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        assert!(
            input.read_with(cx, |i, _| i.mouse_selection.is_some()),
            "selection start kind={kind:?} root={:?} clear={:?}",
            v.debug_bounds("clip-name-input"),
            v.debug_bounds("clip-name-input-clear")
        );
        v.simulate_mouse_down(outside, MouseButton::Right, gpui::Modifiers::default());
        v.simulate_event(gpui::ScrollWheelEvent {
            position: outside,
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(20.))),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "held text selection must isolate other mouse actions"
        );
        v.simulate_mouse_move(outside, Some(MouseButton::Left), gpui::Modifiers::default());
        v.run_until_parked();
        assert_eq!(v.rendered_cursor_style(), Some(CursorStyle::IBeam));
        v.update(|window, cx| {
            window
                .root::<CaptureRoot>()
                .unwrap()
                .unwrap()
                .update(cx, |_, cx| cx.notify())
        });
        v.run_until_parked();
        v.simulate_mouse_up(outside, MouseButton::Left, gpui::Modifiers::default());
        v.simulate_mouse_up(outside, MouseButton::Right, gpui::Modifiers::default());
        v.run_until_parked();
        assert_eq!(
            actions.get(),
            0,
            "release cannot activate the element underneath"
        );
        input.read_with(cx, |input, _| {
            assert!(input.mouse_selection.is_none());
            assert_eq!(input.text(), "İA🎛 /Samples/Kick.WAV");
            assert!(
                input.content.is_char_boundary(input.selected.start)
                    && input.content.is_char_boundary(input.selected.end)
            );
        });
        assert_eq!(v.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        v.simulate_mouse_down(outside, MouseButton::Right, gpui::Modifiers::default());
        assert_eq!(
            actions.get(),
            1,
            "ordinary input must resume after selection"
        );
        v.simulate_mouse_up(outside, MouseButton::Right, gpui::Modifiers::default());
        if kind == TextInputKind::Search {
            let clear = v.debug_bounds("clip-name-input-clear").unwrap().center();
            v.simulate_click(clear, gpui::Modifiers::default());
            v.run_until_parked();
            assert!(
                input.read_with(cx, |i, _| i.text().is_empty()),
                "clear must keep its own mouse-down action"
            );
        }
    }
}

#[gpui::test]
fn text_selection_retires_on_replacement_composition_and_focus_loss(cx: &mut TestAppContext) {
    for terminal in 0..3 {
        let (handle, input, actions) = capture_fixture(cx, TextInputKind::Value);
        let mut v = VisualTestContext::from_window(handle, cx);
        v.simulate_resize(gpui::size(px(400.), px(100.)));
        v.run_until_parked();
        let start = v.debug_bounds("clip-name-input").unwrap().center();
        v.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        v.run_until_parked();
        match terminal {
            0 => input.update(cx, |i, cx| i.set_text("Replacement 🎹", cx)),
            1 => v.update(|window, cx| {
                input.update(cx, |i, cx| {
                    i.replace_and_mark_text_in_range(
                        Some(0..i.content.encode_utf16().count()),
                        "İA🎛",
                        Some(1..1),
                        window,
                        cx,
                    )
                })
            }),
            _ => v.update(|window, _| window.blur()),
        }
        v.run_until_parked();
        let retained = input.read_with(cx, |i, _| {
            assert!(
                i.mouse_selection.is_none(),
                "terminal {terminal} must retire input without waiting for release"
            );
            (i.content.clone(), i.selected.clone(), i.marked.clone())
        });
        let outside = point(px(260.), px(10.));
        v.simulate_mouse_move(outside, Some(MouseButton::Left), gpui::Modifiers::default());
        v.simulate_mouse_up(outside, MouseButton::Left, gpui::Modifiers::default());
        v.run_until_parked();
        assert_eq!(
            actions.get(),
            0,
            "terminal {terminal} leaked its old release"
        );
        input.read_with(cx, |i, _| {
            assert_eq!(
                (i.content.clone(), i.selected.clone(), i.marked.clone()),
                retained,
                "old pointer must preserve newer text/caret/composition"
            )
        });
        if terminal == 1 {
            v.update(|window, cx| {
                input.update(cx, |i, cx| {
                    assert_eq!(i.marked_text_range(window, cx), Some(0..4));
                    assert_eq!(
                        i.selected_text_range(false, window, cx).unwrap().range,
                        1..1
                    );
                })
            });
        }
    }
}

/// Manual CPU-only selection comparison through complete model/repaint turns.
/// Native text shaping/IME, canonical consumers, GPU and setup/release are excluded.
#[gpui::test]
#[ignore = "manual optimized text selection/CPU-scene profile; no native acceptance"]
fn profile_text_selection(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_TEXT_SELECTION_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for (kind, mode) in [(0, TextInputKind::Search), (1, TextInputKind::Value)] {
        let (handle, input, _) = capture_fixture(cx, mode);
        let mut v = VisualTestContext::from_window(handle, cx);
        v.simulate_resize(gpui::size(px(400.), px(100.)));
        v.run_until_parked();
        let bounds = v.debug_bounds("clip-name-input").unwrap();
        assert!(
            bounds.size.width >= px(150.),
            "profile needs a laid-out text field, not the clear slot"
        );
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            v.simulate_mouse_down(
                bounds.center(),
                MouseButton::Left,
                gpui::Modifiers::default(),
            );
            v.run_until_parked();
            assert!(input.read_with(cx, |i, _| i.mouse_selection.is_some()));
            let start = std::time::Instant::now();
            let mut at = bounds.center();
            for step in 0..8 {
                at = bounds.origin
                    + point(
                        bounds.size.width * (0.1 + ((round + step) % 7) as f32 * 0.1),
                        bounds.size.height / 2.,
                    );
                v.simulate_mouse_move(at, Some(MouseButton::Left), gpui::Modifiers::default());
                v.run_until_parked();
            }
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.;
            v.simulate_mouse_up(at, MouseButton::Left, gpui::Modifiers::default());
            v.run_until_parked();
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = v.update(|w, _| w.scale_factor());
        println!(
            "TEXT_SELECTION_PROFILE kind={kind} size=400x100 scale={scale} warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}

#[gpui::test]
fn same_position_input_restarts_hidden_caret(cx: &mut TestAppContext) {
    let (handle, input) = fixture(cx, "x");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("end");
    visual.run_until_parked();
    let caret = gpui::Background::from(gpui::rgb(config().style.accent)).opacity(0.5);
    let is_visible = |visual: &mut VisualTestContext| {
        visual
            .rendered_filled_quads("clip-name-input")
            .iter()
            .any(|(_, fill)| *fill == caret)
    };
    assert!(is_visible(&mut visual));
    visual
        .executor()
        .advance_clock(std::time::Duration::from_millis(510));
    visual.run_until_parked();
    assert!(!is_visible(&mut visual));
    // Delete at the end changes neither the text nor the caret geometry.
    visual.simulate_keystrokes("delete");
    visual.run_until_parked();
    assert!(is_visible(&mut visual));
    visual
        .executor()
        .advance_clock(std::time::Duration::from_millis(510));
    visual.run_until_parked();
    assert!(!is_visible(&mut visual));
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    assert!(is_visible(&mut visual));
    visual
        .executor()
        .advance_clock(std::time::Duration::from_millis(510));
    visual.run_until_parked();
    assert!(!is_visible(&mut visual));
    visual.update(|window, cx| input.update(cx, |input, cx| input.unmark_text(window, cx)));
    visual.run_until_parked();
    assert!(is_visible(&mut visual));
    visual.update(|window, _| window.blur());
    visual.run_until_parked();
    assert!(!is_visible(&mut visual));
}

struct RemovableRoot {
    input: Entity<TextInput>,
    shown: bool,
    blink_notifies: usize,
    _blink: gpui::Subscription,
}
impl gpui::Render for RemovableRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(400.0))
            .children(self.shown.then(|| self.input.clone()))
    }
}

#[gpui::test]
fn removing_a_focused_input_stops_its_blink_wakes(cx: &mut TestAppContext) {
    let handle = cx.add_window(|window, cx| {
        // A collapsed selection paints the caret.
        let input = cx.new(|cx| TextInput::new(cx, config()));
        input.update(cx, |input, _| input.focus(window));
        // The idle blink timer is the input's only notify source here.
        let blink = cx.observe(&input, |root: &mut RemovableRoot, _, _| {
            root.blink_notifies += 1;
        });
        RemovableRoot {
            input,
            shown: true,
            blink_notifies: 0,
            _blink: blink,
        }
    });
    let root = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let notifies =
        |visual: &mut VisualTestContext| root.read_with(visual, |root, _| root.blink_notifies);
    let start = notifies(&mut visual);
    visual
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    visual.run_until_parked();
    let blinking = notifies(&mut visual) - start;
    assert!(
        (3..=5).contains(&blinking),
        "a focused idle caret steps about twice a second, got {blinking} wakes in 2 s"
    );
    visual.update(|_, cx| {
        root.update(cx, |root, cx| {
            root.shown = false;
            cx.notify();
        })
    });
    visual.run_until_parked();
    let removed = notifies(&mut visual);
    visual
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    visual.run_until_parked();
    // GPUI keeps the last painted frame's keyed state until the next draw, so
    // the one wake already scheduled may still land; it draws nothing new.
    let settled = notifies(&mut visual);
    assert!(
        settled - removed <= 1,
        "a removed caret kept waking: {} wakes in 2 s",
        settled - removed
    );
    visual
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    visual.run_until_parked();
    assert_eq!(
        notifies(&mut visual),
        settled,
        "a removed caret must stop scheduling blink wakes"
    );
}
