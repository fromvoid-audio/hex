//! History through real toolbar input over bounded private worker roots.
use super::*;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    PreferencesTestDirectory,
) {
    cx.update(crate::editor::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    std::fs::create_dir_all(directory.0.join("samples")).unwrap();
    std::fs::write(directory.0.join("samples/loop.wav"), mono_pcm_wav(24)).unwrap();
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
    (handle, editor, directory)
}

#[gpui::test]
fn browser_history_toolbar_restores_source_without_preview_or_import(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for selector in ["browser-section-devices", "browser-section-projects"] {
        let hit = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(hit, Modifiers::default());
        visual.run_until_parked();
    }
    let hit = visual
        .debug_bounds("browser-history-back")
        .unwrap()
        .center();
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Instruments);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert!(view.browser.playback_path.is_none());
    });
}

fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let point = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing {selector}"))
        .center();
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
}

fn query(editor: &gpui::Entity<EditorView>, cx: &mut VisualTestContext) -> String {
    editor.read_with(cx, |view, cx| view.browser.search.read(cx).content.clone())
}

#[gpui::test]
fn browser_history_typing_coalesces_until_navigation_and_branch_clears_forward(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(visual, "browser-search");
    for letter in ["l", "o", "o", "p"] {
        visual.simulate_input(letter);
        visual.run_until_parked();
    }
    click(visual, "browser-section-devices");
    click(visual, "browser-history-back");
    assert_eq!(query(&editor, visual), "loop");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Samples)
    });
    click(visual, "browser-history-back");
    assert_eq!(query(&editor, visual), "");
    click(visual, "browser-history-back"); // disabled boundary
    assert_eq!(query(&editor, visual), "");
    click(visual, "browser-history-forward");
    assert_eq!(query(&editor, visual), "loop");
    click(visual, "browser-history-back");
    click(visual, "browser-search");
    visual.simulate_input("新");
    visual.run_until_parked();
    click(visual, "browser-history-forward"); // old branch is gone
    assert_eq!(query(&editor, visual), "新");
    click(visual, "browser-history-back");
    assert_eq!(query(&editor, visual), "");
    // Command-F commits a distinct location, keeps its accepted selection behavior.
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("utility");
    visual.run_until_parked();
    assert_eq!(query(&editor, visual), "utility");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::All)
    });
    click(visual, "browser-history-back");
    assert_eq!(query(&editor, visual), "");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::All)
    });
    click(visual, "browser-history-back");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Samples)
    });
}

#[gpui::test]
fn browser_history_restores_native_item_and_scroll_identity_without_focus_or_audition(
    cx: &mut TestAppContext,
) {
    let (handle, editor, directory) = fixture(cx);
    // A positive scroll anchor far below the first viewport.
    for index in 0..80 {
        std::fs::write(
            directory.0.join(format!("samples/{index:03}.wav")),
            mono_pcm_wav(24),
        )
        .unwrap();
    }
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(visual, "browser-place-0");
    editor.update(visual, |view, cx| {
        view.browser.auto_audition = true;
        view.browser.results_focused = true;
        view.browser.selected_index = 62;
        view.browser
            .scroll
            .scroll_to_item(60, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    let (selected, top) = editor.read_with(visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        (
            entries[62].path.clone(),
            entries[view.browser.scroll.logical_scroll_top_index()]
                .path
                .clone(),
        )
    });
    click(visual, "browser-section-devices");
    let generation = editor.read_with(visual, |view, _| view.browser.playback_generation);
    click(visual, "browser-history-back");
    visual.update(|window,cx|editor.read_with(cx,|view,cx| {
        assert!(matches!(view.browser_selected_item(cx),Some(crate::editor::browser::BrowserItemKey::File(path)) if path==selected));
        let entries=view.browser_visible_entries(cx);
        assert_eq!(entries[view.browser.scroll.logical_scroll_top_index()].path,top);
        assert!(!view.browser.search.read(cx).focus_handle().is_focused(window));
        assert_eq!(view.browser.playback_generation,generation+1,"only cancellation, no replay");
        assert!(view.browser.playback_path.is_none());
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    }));
    // Restore again after deleting the exact selected item: deterministic clearing,
    // never a same-name or row-index substitute.
    click(visual, "browser-section-devices");
    std::fs::remove_file(&selected).unwrap();
    click(visual, "browser-history-back");
    editor.read_with(visual, |view, _| {
        assert!(!view.browser.results_focused);
        assert_eq!(view.browser.selected_index, 0);
    });
}

#[gpui::test]
fn browser_history_missing_place_remains_recoverable_and_late_scan_cannot_retarget(
    cx: &mut TestAppContext,
) {
    let (handle, editor, directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let missing = directory.0.join("offline");
    editor.update(visual, |view, cx| {
        view.browser.places.push(missing.clone());
        view.select_browser_source(BrowserSection::Place(missing.clone()), cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(visual, |view, _| assert!(view.browser.error.is_some()));
    click(visual, "browser-section-devices");
    click(visual, "browser-history-back");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::Place(missing.clone()));
        assert!(view.browser.error.is_some());
        assert!(view.browser_visible_entries(cx).is_empty());
    });
    click(visual, "browser-history-back");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Samples)
    });
    click(visual, "browser-history-forward");
    click(visual, "browser-section-projects");
    editor.update(visual, |view, cx| {
        let old = crate::editor::browser::ScanRequest {
            generation: view.browser.scans.generation.saturating_sub(1),
            directory: missing.clone(),
        };
        view.browser_scan_finished(
            old,
            Ok(DirectoryListing {
                directory: missing.clone(),
                entries: vec![LibraryEntry {
                    name: "late.wav".into(),
                    path: missing.join("late.wav"),
                    kind: EntryKind::WaveAudio,
                }],
                enumerated_entries: 1,
                truncated: false,
            }),
            cx,
        );
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::Projects);
        assert!(
            view.browser_visible_entries(cx)
                .iter()
                .all(|entry| entry.name != "late.wav")
        );
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn browser_history_query_back_restores_item_before_filter_and_ime_restore_is_not_an_edit(
    cx: &mut TestAppContext,
) {
    use gpui::EntityInputHandler;
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(visual, "browser-place-0");
    editor.update(visual, |view, _| view.browser.auto_audition = false);
    click(visual, "browser-entry-0");
    click(visual, "browser-search");
    visual.simulate_input("no-match");
    visual.run_until_parked();
    click(visual, "browser-history-back");
    editor.read_with(visual,|view,cx| {
        assert_eq!(view.browser.search.read(cx).content,"");
        assert!(matches!(view.browser_selected_item(cx),Some(crate::editor::browser::BrowserItemKey::File(path)) if path.ends_with("loop.wav")),"query back must restore the original highlighted identity");
    });
    click(visual, "browser-search");
    visual.update(|window, cx| {
        editor
            .read(cx)
            .browser
            .search
            .clone()
            .update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "音", Some(1..1), window, cx);
                assert!(input.marked_text_range(window, cx).is_some());
            })
    });
    visual.run_until_parked();
    click(visual, "browser-history-back");
    assert_eq!(query(&editor, visual), "");
    visual.update(|window, cx| {
        editor
            .read(cx)
            .browser
            .search
            .clone()
            .update(cx, |input, cx| {
                assert!(input.marked_text_range(window, cx).is_none());
                assert!(!input.focus_handle().is_focused(window));
            })
    });
    visual.run_until_parked();
    click(visual, "browser-history-forward");
    assert_eq!(query(&editor, visual), "音");
}

#[gpui::test]
fn browser_history_same_name_rows_collection_and_queued_query_observer_keep_latest_intent(
    cx: &mut TestAppContext,
) {
    let (handle, editor, directory) = fixture(cx);
    for folder in ["a", "b"] {
        std::fs::create_dir_all(directory.0.join("samples").join(folder)).unwrap();
        std::fs::write(
            directory.0.join("samples").join(folder).join("loop.wav"),
            mono_pcm_wav(24),
        )
        .unwrap();
    }
    let target = directory.0.join("samples/b/loop.wav");
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("loop");
    visual.run_until_parked();
    let index = editor.read_with(visual, |view, cx| {
        view.browser_visible_entries(cx)
            .iter()
            .position(|entry| entry.path == target)
            .unwrap()
    });
    editor.update(visual, |view, _| view.browser.auto_audition = false);
    let row = visual
        .debug_bounds(["browser-entry-0", "browser-entry-1", "browser-entry-2"][index])
        .unwrap()
        .center();
    visual.simulate_click(row, Modifiers::default());
    visual.run_until_parked();
    click(visual, "browser-collection-0");
    click(visual, "browser-history-back");
    editor.read_with(visual,|view,cx|assert!(matches!(view.browser_selected_item(cx),Some(crate::editor::browser::BrowserItemKey::File(path)) if path==target)));
    click(visual, "browser-history-forward");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Collection(0))
    });
    click(visual, "browser-history-back");
    // Queue an input notification and Back in the same UI update. The delayed
    // observer must acknowledge restoration, not reset its item or push history.
    editor.update(visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.restore_query("new query", cx));
        view.move_browser_history(false, cx);
    });
    visual.run_until_parked();
    assert_eq!(query(&editor, visual), "loop");
    editor.read_with(visual,|view,cx|assert!(matches!(view.browser_selected_item(cx),Some(crate::editor::browser::BrowserItemKey::File(path)) if path==target)));
    click(visual, "browser-history-forward");
    assert_eq!(query(&editor, visual), "new query");
    // Latest deliberate source selection cancels any deferred position restore.
    editor.update(visual, |view, cx| {
        view.move_browser_history(false, cx);
        view.select_browser_source(BrowserSection::Projects, cx);
    });
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Projects);
        assert!(!view.browser.results_focused);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn browser_history_transition_clears_observed_preview_presentation(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(visual, "browser-section-devices");
    click(visual, "browser-section-projects");
    let (mut control, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
    let generation = control.sample_preview.request().unwrap();
    control
        .sample_preview
        .publish(
            generation,
            hex_engine::Audio::new(48_000, vec![[0.25; 2]; 8]).unwrap(),
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    editor.update(visual, |view, cx| {
        assert!(view.accept_browser_sample_phase(control.sample_preview.snapshot().phase));
        assert_eq!(view.status.as_str(), "sample preview playing");
        cx.notify();
    });
    visual.run_until_parked();
    click(visual, "browser-history-back");
    editor.read_with(visual, |view, _| {
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Idle
        );
        assert_eq!(view.status.as_str(), "sample preview stopped");
        assert!(view.browser.playback_pending.is_none());
        assert!(view.browser.playback_output.is_none());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn browser_history_removed_or_hidden_source_fallback_does_not_push_or_clear_forward(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(visual, "browser-place-0");
    click(visual, "browser-section-devices");
    editor.update(visual, |view, _| view.browser.places.clear());
    click(visual, "browser-history-back");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::All);
        assert!(
            view.browser.places.is_empty(),
            "history cannot reconfigure a removed source"
        );
        assert!(
            view.browser
                .places_status
                .as_ref()
                .unwrap()
                .contains("history source unavailable")
        );
    });
    click(visual, "browser-history-forward");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Instruments)
    });
    click(visual, "browser-control-⚙");
    click(visual, "browser-section-visibility-1");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Samples)
    });
    click(visual, "browser-history-back");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::All)
    });
    click(visual, "browser-history-forward");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Samples);
        assert!(view.pending_requests.is_empty());
    });
}
