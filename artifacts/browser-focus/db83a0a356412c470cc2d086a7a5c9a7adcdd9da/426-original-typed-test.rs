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
        ] {
            editor.update(visual, |view, cx| {
                view.mode = mode;
                view.browser.auto_audition = false;
                view.browser.search.update(cx, |input, cx| input.clear(cx));
                view.select_browser_source(BrowserSection::Place(samples.clone()), cx);
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
            visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
            if target.ends_with("resize") || target.ends_with("resize-handle") {
                visual.simulate_mouse_move(
                    bounds.center() + point(px(5.0), px(0.0)),
                    MouseButton::Left,
                    Modifiers::default(),
                );
            }
            visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let query = editor.read_with(visual, |view, cx| {
                view.browser.search.read(cx).content.clone()
            });
            visual.simulate_input("qxj");
            editor.read_with(visual, |view, cx| assert_eq!(view.browser.search.read(cx).content, query, "typing after {target} changed query"));
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
        }
    }
}
