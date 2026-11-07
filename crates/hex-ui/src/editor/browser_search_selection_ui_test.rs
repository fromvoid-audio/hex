//! Actual input/UTF-16 selection checks; never a native focus-bug acceptance claim.
use super::*;
use gpui::EntityInputHandler;
use hex_session::service::Request;
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
        assert_eq!(view.browser.search.read(cx).text(), "snare");
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
        assert_eq!(view.browser.search.read(cx).text(), "hat")
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
        assert_eq!(
            view.browser.search.read(cx).text(),
            "kick hats",
            "category click must leave actual text editing"
        )
    });
    let field = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    assert_eq!(selection(&editor, visual), 0..9);
    let start = caret_point(&editor, visual, 1);
    let end = caret_point(&editor, visual, 4);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.simulate_input("X");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "kx hats")
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
        assert_eq!(view.browser.search.read(cx).text(), "kick hatsick")
    });
    visual.simulate_keystrokes("cmd-a cmd-x");
    assert_eq!(selection(&editor, visual), 0..0);
    visual.simulate_keystrokes("cmd-v");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "kick hatsick")
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
        assert_eq!(view.browser.search.read(cx).text(), "x");
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert_eq!(view.browser.playback_generation, 0);
    });
}

// Only the new toggle regressions use this bounded pre-worker seam. Inject the
// actual root lease before startup; older fixtures retain their original scope.
fn toggle_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    std::sync::Arc<super::super::super::test_environment::Environment>,
    std::path::PathBuf,
) {
    let directory = super::super::super::test_environment::Environment::new().unwrap();
    let samples = directory.root().join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    std::fs::write(samples.join("loop.wav"), mono_pcm_wav(24)).unwrap();
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    std::sync::Arc::clone(&directory),
                )
            })
        })
        .unwrap();
    editor.read_with(cx, |view, _| {
        let root = directory.root();
        assert!(std::sync::Arc::ptr_eq(&directory, &view.test_environment));
        assert_eq!(view.browser.home, directory.home);
        assert!(samples.join("loop.wav").is_file());
        assert_eq!(
            view.browser.test_preferences_path(),
            root.join("config/browser.json")
        );
        assert_eq!(
            view.recent_projects.test_path(),
            root.join("config/recent-projects.json")
        );
        assert_eq!(
            view.theme_settings.test_path(),
            root.join("config/theme.json")
        );
        assert_eq!(
            view.project_directory.test_path(),
            root.join("config/project-directory.json")
        );
        assert_eq!(
            view.browser
                .test_preferences_path()
                .with_extension("samples-v1.db"),
            root.join("config/browser.samples-v1.db")
        );
        assert_eq!(view.browser.directory, samples);
        assert_eq!(view.browser.places, vec![samples.clone()]);
        assert_eq!(view.sample_roots(), vec![samples.clone()]);
    });
    (handle, editor, directory, samples)
}

fn double_click_at(
    visual: &mut VisualTestContext,
    at: gpui::Point<gpui::Pixels>,
    first_count: usize,
) {
    for click_count in [first_count, first_count + 1] {
        visual.simulate_event(gpui::MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        visual.simulate_event(gpui::MouseUpEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count,
        });
    }
}

#[gpui::test]
fn search_double_click_toggles_against_selection_before_first_click(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _directory, samples) = toggle_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("kick hats");
    visual.simulate_keystrokes("cmd-f");
    assert_eq!(selection(&editor, visual), 0..9);
    let at = caret_point(&editor, visual, 4);
    let before_requests = editor.read_with(visual, |view, _| {
        (
            view.browser.playback_generation,
            view.browser.preview_preparations,
            view.browser.tree.requests_started,
        )
    });
    for (count, expected) in [(1, 4..4), (3, 0..9), (1, 4..4), (1, 0..9)] {
        double_click_at(visual, at, count);
        assert_eq!(
            selection(&editor, visual),
            expected,
            "double gesture starting with click_count {count}"
        );
        editor.read_with(visual, |view, cx| {
            assert_eq!(view.browser.search.read(cx).text(), "kick hats");
            assert_eq!(
                (
                    view.browser.playback_generation,
                    view.browser.preview_preparations,
                    view.browser.tree.requests_started
                ),
                before_requests,
                "selection-only gesture emitted browser work"
            );
        });
    }
    // The first click has collapsed selection, but actual filesystem completions
    // and repaint between the clicks must not change the pre-gesture snapshot.
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    workers_and_ticks(&editor, visual, &samples);
    visual.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    visual.simulate_event(gpui::MouseUpEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
    });
    assert_eq!(selection(&editor, visual), 4..4);
    double_click_at(visual, at, 1);
    assert_eq!(selection(&editor, visual), 0..9);
    visual.simulate_resize(gpui::size(px(1100.0), px(800.0)));
    visual.run_until_parked();
    let start = caret_point(&editor, visual, 1);
    let end = caret_point(&editor, visual, 4);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    assert_eq!(selection(&editor, visual), 1..4);
    let at = caret_point(&editor, visual, 2);
    double_click_at(visual, at, 1);
    assert_eq!(
        selection(&editor, visual),
        0..9,
        "partial selection doubleclick selects all"
    );
    visual.simulate_mouse_move(end, None, Modifiers::default());
    assert_eq!(
        selection(&editor, visual),
        0..9,
        "released hover cannot toggle"
    );
    workers_and_ticks(&editor, visual, &samples);
    assert_eq!(selection(&editor, visual), 0..9);
    editor.read_with(visual, |view, _| {
        assert!(view.pending_audio_commands.is_empty());
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn search_empty_double_click_is_selection_only(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _directory, _samples) = toggle_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    let field = visual.debug_bounds("browser-search").unwrap();
    for count in [1, 3, 1] {
        double_click_at(visual, field.center(), count);
        assert_eq!(selection(&editor, visual), 0..0);
    }
    editor.read_with(visual, |view, cx| {
        assert!(view.browser.search.read(cx).text().is_empty());
        assert_eq!(view.browser.playback_generation, 0);
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
    });
}

fn invalidated_pair_caller(cx: &mut TestAppContext, invalidation: &str) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _directory, _samples) = toggle_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("kick hats");
    visual.simulate_keystrokes("cmd-f");
    let first = caret_point(&editor, visual, 4);
    visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(first, MouseButton::Left, Modifiers::default());
    match invalidation {
        "keyboard" => visual.simulate_keystrokes("left"),
        "text" => visual.simulate_input("x"),
        "ime" => visual.update(|window, cx| {
            let input = editor.read(cx).browser.search.clone();
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "İ🎛", Some(1..1), window, cx);
                input.unmark_text(window, cx);
            });
        }),
        "shift" => {
            let at = caret_point(&editor, visual, 6);
            let modifiers = Modifiers {
                shift: true,
                ..Default::default()
            };
            visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
            visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
        }
        "drag" => {
            let at = caret_point(&editor, visual, 6);
            visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        }
        _ => unreachable!(),
    }
    let at = caret_point(&editor, visual, 2);
    let before = editor.read_with(visual, |view, cx| {
        (
            view.browser.search.read(cx).text().to_owned(),
            view.browser.playback_generation,
            view.browser.preview_preparations,
            view.browser.tree.requests_started,
        )
    });
    for down in [true, false] {
        if down {
            visual.simulate_event(gpui::MouseDownEvent {
                position: at,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count: 2,
                first_mouse: false,
            });
        } else {
            visual.simulate_event(gpui::MouseUpEvent {
                position: at,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count: 2,
            });
        }
    }
    assert_eq!(
        selection(&editor, visual),
        2..2,
        "invalidated {invalidation} pair must use normal caret input"
    );
    editor.read_with(visual, |view, cx| {
        assert_eq!(
            (
                view.browser.search.read(cx).text().to_owned(),
                view.browser.playback_generation,
                view.browser.preview_preparations,
                view.browser.tree.requests_started
            ),
            before
        );
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
    });
    double_click_at(visual, at, 3);
    assert_eq!(
        selection(&editor, visual),
        0..before.0.encode_utf16().count(),
        "fresh rapid3/4 pair remains valid"
    );
}

#[gpui::test]
fn search_invalidated_pair_keyboard(cx: &mut TestAppContext) {
    invalidated_pair_caller(cx, "keyboard");
}
#[gpui::test]
fn search_invalidated_pair_text(cx: &mut TestAppContext) {
    invalidated_pair_caller(cx, "text");
}
#[gpui::test]
fn search_invalidated_pair_ime(cx: &mut TestAppContext) {
    invalidated_pair_caller(cx, "ime");
}
#[gpui::test]
fn search_invalidated_pair_shift(cx: &mut TestAppContext) {
    invalidated_pair_caller(cx, "shift");
}
#[gpui::test]
fn search_invalidated_pair_drag(cx: &mut TestAppContext) {
    invalidated_pair_caller(cx, "drag");
}

#[gpui::test]
fn toggle_fixture_same_root_retains_actual_catalog_job_after_view_and_caller_drop(
    cx: &mut TestAppContext,
) {
    use std::{
        sync::{Arc, atomic::Ordering},
        time::Instant,
    };
    let (handle, editor, directory, samples) = toggle_fixture(cx);
    let root = directory.root().to_owned();
    let weak = Arc::downgrade(&directory);
    let gate = directory.gate_next_catalog_save();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(
        gate.reached.load(Ordering::Acquire),
        "actual catalog worker at save gate"
    );
    assert!(!gate.finished.load(Ordering::Acquire));
    editor.read_with(&visual, |view, _| {
        assert!(Arc::ptr_eq(&directory, &view.test_environment));
        assert_eq!(view.browser.directory, samples);
        assert_eq!(
            view.browser
                .test_preferences_path()
                .with_extension("samples-v1.db"),
            root.join("config/browser.samples-v1.db")
        );
    });
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| Empty);
    });
    let editor_weak = editor.downgrade();
    drop(editor);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(
        editor_weak.upgrade().is_none(),
        "view retired before real save continuation"
    );
    drop(directory);
    assert_eq!(
        weak.strong_count(),
        1,
        "only actual held catalog job retains SAME fixture root"
    );
    assert!(root.is_dir());
    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(weak.upgrade().is_none(), "no caller/view/job Arc leak");
    let until = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(
            Instant::now() < until,
            "owned root cleanup timed out: {root:?}"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(!root.exists(), "drained catalog job cannot recreate root");
    eprintln!(
        "toggle_fixture_custody same_root=true real_save=true all_four_preferences=true view_retired=true cleaned=true"
    );
}

#[gpui::test]
fn clear_search_preserves_scope_focus_and_reserved_geometry(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    for width in [220.0, 480.0] {
        let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
        let visual = &mut VisualTestContext::from_window(handle, cx);
        editor.update(visual, |view, cx| {
            view.browser.width = width;
            cx.notify();
        });
        visual.run_until_parked();
        // This browser fixture begins with a projected UI snapshot. Establish
        // matching real worker state before comparing it across later ticks.
        editor.update(visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddTrack {
                name: "target".into()
            })))
        });
        super::super::super::clip_drag_ui_test::pump_move(&editor, visual);
        let before = editor.read_with(visual, |view, cx| {
            assert!(!view.browser_visible_entries(cx).is_empty());
            (
                view.browser.section.clone(),
                view.state.as_ref().unwrap().project.clone(),
                view.selected_track,
            )
        });
        let field = visual.debug_bounds("browser-search").unwrap();
        let toolbar = visual.debug_bounds("browser-toolbar").unwrap();
        assert_eq!(toolbar.size.height, px(24.0));
        assert_eq!(field.size.height, px(20.0));
        assert!(field.size.width >= px(50.0));
        assert!(visual.rendered_border("browser-search").is_none());
        let actions = [
            visual.debug_bounds("browser-history-back").unwrap(),
            visual.debug_bounds("browser-history-forward").unwrap(),
            field,
            visual.debug_bounds("browser-control-↻").unwrap(),
            visual.debug_bounds("browser-reveal-sample").unwrap(),
        ];
        for pair in actions.windows(2) {
            assert!(
                pair[0].right() <= pair[1].left(),
                "search actions retain their order"
            );
        }
        for action in actions {
            assert!(action.left() >= toolbar.left() && action.right() <= toolbar.right());
        }
        let slot = visual
            .debug_bounds("browser-search-clear-slot")
            .expect("reserved clear slot");
        assert!(visual.debug_bounds("browser-search-clear").is_none());
        visual.simulate_click(field.center(), Modifiers::default());
        visual.simulate_input("missing_query_Ω_long_enough_to_scroll_the_input");
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("browser-search"), Some(field));
        assert_eq!(visual.debug_bounds("browser-search-clear-slot"), Some(slot));
        let clear = visual.debug_bounds("browser-search-clear").unwrap();
        assert_eq!(clear.size, gpui::size(px(14.0), px(14.0)));
        let scale = visual.update(|window, _| window.scale_factor());
        assert_eq!(
            visual
                .rendered_surface_geometry("browser-search-clear")
                .unwrap()
                .0
                .top_left,
            px(7.0).scale(scale)
        );
        let text = visual.debug_bounds("browser-search-text").unwrap();
        assert!(text.right() <= clear.left() && clear.right() <= field.right());
        editor.read_with(visual, |view, cx| {
            assert!(view.browser_visible_entries(cx).is_empty())
        });
        visual.simulate_click(clear.center(), Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("browser-search-clear").is_none());
        assert_eq!(visual.debug_bounds("browser-search-clear-slot"), Some(slot));
        assert_eq!(selection(&editor, visual), 0..0);
        workers_and_ticks(&editor, visual, &directory.0);
        editor.read_with(visual, |view, cx| {
            assert!(view.browser.search.read(cx).text().is_empty());
            assert_eq!(
                (
                    view.browser.section.clone(),
                    view.state.as_ref().unwrap().project.clone(),
                    view.selected_track
                ),
                before
            );
            assert!(!view.browser_visible_entries(cx).is_empty());
            assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
            assert_eq!(view.browser.playback_generation, 0);
        });
        visual.simulate_input("loop");
        editor.read_with(visual, |view, cx| {
            assert_eq!(view.browser.search.read(cx).text(), "loop")
        });
    }
}

#[gpui::test]
fn clear_search_accessibility_refuses_old_text_and_uses_current_input_owner(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let bounds = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.simulate_input("old query");
    visual.run_until_parked();
    let request = |visual: &mut VisualTestContext| {
        visual.update(|window, _| {
            let node = window
                .accessibility_nodes()
                .find(|node| node.descriptor.name.as_ref() == "clear search")
                .expect("clear action semantics");
            assert_eq!(node.descriptor.value, gpui::AccessibilityValue::Button);
            gpui::AccessibilityRequest {
                id: node.descriptor.id,
                revision: node.descriptor.revision,
                action: gpui::AccessibilityAction::Press,
            }
        })
    };
    let stale = request(visual);
    visual.simulate_input(" newer text");
    visual.run_until_parked();
    assert!(!visual.update(|window, cx| window.perform_accessibility_request(&stale, cx)));
    let current = request(visual);
    assert!(visual.update(|window, cx| window.perform_accessibility_request(&current, cx)));
    visual.run_until_parked();
    assert!(!visual.update(|window, cx| window.perform_accessibility_request(&current, cx)));
    assert!(visual.debug_bounds("browser-search-clear").is_none());
    assert_eq!(selection(&editor, visual), 0..0);
    visual.simulate_input("replacement");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "replacement")
    });
}

#[gpui::test]
fn clear_search_cancels_composition_and_resets_selection(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let bounds = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    let mark = |visual: &mut VisualTestContext| {
        visual.update(|window, cx| {
            editor
                .read(cx)
                .browser
                .search
                .clone()
                .update(cx, |input, cx| {
                    input.replace_and_mark_text_in_range(None, "候補", Some(2..2), window, cx)
                });
        })
    };
    mark(visual);
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-search-clear").is_none(),
        "composition alone is not committed query text"
    );
    visual.simulate_input("kick");
    mark(visual);
    visual.run_until_parked();
    let clear = visual.debug_bounds("browser-search-clear").unwrap();
    visual.simulate_click(clear.center(), Modifiers::default());
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor
            .read(cx)
            .browser
            .search
            .clone()
            .update(cx, |input, cx| {
                assert!(input.text().is_empty());
                assert!(input.marked_text_range(window, cx).is_none());
            })
    });
    visual.simulate_input("loop");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "loop")
    });
}
