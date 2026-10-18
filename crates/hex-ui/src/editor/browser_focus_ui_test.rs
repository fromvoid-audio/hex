//! Actual pointer/focus/input checks over private startup roots; no audio output.
use super::*;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    PreferencesTestDirectory,
    std::path::PathBuf,
) {
    cx.update(crate::editor::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let samples = directory.0.join("samples");
    std::fs::create_dir_all(samples.join("folder")).unwrap();
    std::fs::write(samples.join("loop.wav"), mono_pcm_wav(24)).unwrap();
    std::fs::write(samples.join("folder/child.wav"), mono_pcm_wav(24)).unwrap();
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    &directory.0,
                )
            })
        })
        .unwrap();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.browser.directory, samples);
        assert_eq!(view.browser.places, vec![samples.clone()]);
        assert_eq!(view.sample_roots(), vec![samples.clone()]);
    });
    (handle, editor, directory, samples)
}

#[gpui::test]
fn browser_non_text_clicks_leave_real_query_focus_and_do_not_accept_typing(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _directory, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for mode in [
        crate::editor::EditorMode::Arrangement,
        crate::editor::EditorMode::Session,
    ] {
        for target in [
            "browser-section-samples",
            "browser-section-devices",
            "browser-section-projects",
            "browser-section-all",
            "browser-collection-0",
            "browser-place-0",
            "browser-entry-0",
            "browser-entry-1",
            "browser-disclosure-0",
            "browser-control-⚙",
            "browser-sidebar-resize",
            "browser-resize-handle",
            "browser-sidebar",
            "browser-selection-preview",
            "browser-history-back",
            "browser-audition-auto",
            "browser-device-effect-hex.utility",
        ] {
            editor.update(visual, |view, cx| {
                view.mode = mode;
                view.browser.auto_audition = false;
                view.browser.search.update(cx, |input, cx| input.clear(cx));
                view.select_browser_source(BrowserSection::Place(samples.clone()), cx);
                if target == "browser-device-effect-hex.utility" {
                    view.select_browser_source(BrowserSection::Instruments, cx);
                }
                cx.notify();
            });
            visual.run_until_parked();
            let search = visual.debug_bounds("browser-search").unwrap();
            visual.simulate_click(search.center(), Modifiers::default());
            visual.update(|window, cx| {
                editor.read_with(cx, |view, cx| {
                    assert!(
                        view.browser
                            .search
                            .read(cx)
                            .focus_handle()
                            .is_focused(window)
                    )
                })
            });
            let bounds = visual
                .debug_bounds(target)
                .unwrap_or_else(|| panic!("missing target {target}"));
            let hit = match target {
                "browser-sidebar" => point(bounds.center().x, bounds.bottom() - px(4.0)),
                "browser-selection-preview" => bounds.origin + point(px(5.0), px(5.0)),
                _ => bounds.center(),
            };
            visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
            if target.ends_with("resize") || target.ends_with("resize-handle") {
                visual.simulate_mouse_move(
                    bounds.center() + point(px(5.0), px(0.0)),
                    MouseButton::Left,
                    Modifiers::default(),
                );
            }
            let release = if target.ends_with("resize") || target.ends_with("resize-handle") {
                hit + point(px(5.0), px(0.0))
            } else {
                hit
            };
            visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let query = editor.read_with(visual, |view, cx| {
                view.browser.search.read(cx).content.clone()
            });
            visual.simulate_input("qxj");
            editor.read_with(visual, |view, cx| {
                assert_eq!(
                    view.browser.search.read(cx).content,
                    query,
                    "typing after {target} changed query"
                )
            });
            visual.update(|window, cx| {
                editor.read_with(cx, |view, cx| {
                    assert!(
                        !view
                            .browser
                            .search
                            .read(cx)
                            .focus_handle()
                            .is_focused(window),
                        "{target} must not own query text focus"
                    );
                    assert_eq!(view.browser_key_context(window, cx), "hex_browser");
                })
            });
            editor.update(visual, |view, cx| {
                assert_eq!(
                    view.browser.search.read(cx).content,
                    query,
                    "typing after {target} changed query"
                );
                view.refresh_sample_catalog(cx);
                view.request_browser_scan(samples.clone(), cx);
            });
            visual.run_until_parked();
            for _ in 0..4 {
                visual
                    .executor()
                    .advance_clock(std::time::Duration::from_millis(16));
                visual.run_until_parked();
            }
            visual.update(|window, cx| {
                editor.read_with(cx, |view, cx| {
                    assert!(
                        !view
                            .browser
                            .search
                            .read(cx)
                            .focus_handle()
                            .is_focused(window),
                        "worker restored query focus after {target}"
                    );
                    assert_eq!(view.browser.search.read(cx).content, query);
                    assert!(view.pending_requests.is_empty());
                    assert!(view.pending_audio_commands.is_empty());
                })
            });
            let search = visual.debug_bounds("browser-search").unwrap();
            visual.simulate_click(search.center(), Modifiers::default());
            visual.simulate_input("loop");
            editor.read_with(visual, |view, cx| {
                assert_eq!(view.browser.search.read(cx).content, "loop")
            });
            if target == "browser-control-⚙" {
                let gear = visual.debug_bounds(target).unwrap();
                visual.simulate_click(gear.center(), Modifiers::default());
            }
        }
    }
}

#[gpui::test]
fn browser_query_entry_navigation_escape_and_close_have_real_focus_owners(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    let (handle, editor, _directory, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    editor.update(visual, |view, cx| {
        view.browser.auto_audition = false;
        view.select_browser_source(BrowserSection::Place(samples.clone()), cx);
    });
    visual.run_until_parked();
    let search = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(search.center(), Modifiers::default());
    visual.simulate_input("loop");
    visual.simulate_keystrokes("down");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert_eq!(view.browser_key_context(window, cx), "hex_browser");
            assert!(
                !view
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
            assert!(view.browser.results_focus.is_focused(window));
            assert!(view.browser.results_focused);
        })
    });
    visual.simulate_input("qxj");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "loop")
    });
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.browser.search.read(cx).content.is_empty());
            assert_eq!(view.browser_key_context(window, cx), "hex_browser");
        })
    });
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("loop");
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("child");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "child")
    });
    let caret = visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            input
                .bounds_for_range(2..2, search, window, cx)
                .unwrap()
                .center()
        })
    });
    visual.simulate_click(caret, Modifiers::default());
    visual.simulate_input("Z");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "chzild")
    });
    editor.update(visual, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert_eq!(view.browser_key_context(window, cx), "hex_browser_search")
        })
    });
    let close = visual.debug_bounds("browser-control-×").unwrap();
    visual.simulate_click(close.center(), Modifiers::default());
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(!view.browser.open);
            assert!(view.focus.is_focused(window));
            assert!(
                !view
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
        })
    });
    visual.simulate_input("q");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "chzild")
    });
    visual.simulate_keystrokes("cmd-alt-b");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.browser.open);
            assert!(
                !view
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
        })
    });
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("loop");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "loop")
    });
}
