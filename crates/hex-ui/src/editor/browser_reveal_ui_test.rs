//! Reveal uses the real virtual list and toolbar, without entering audition/import.
use super::*;

fn click_reveal(visual: &mut VisualTestContext) {
    let hit = visual
        .debug_bounds("browser-reveal-sample")
        .unwrap()
        .center();
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn browser_reveal_scrolls_both_directions_and_preserves_sample_and_audition(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser.auto_audition = false;
        let listing = view.browser.listing.as_mut().unwrap();
        listing.entries = (0..1000)
            .map(|i| LibraryEntry {
                name: format!("sample-{i:04}.wav"),
                path: directory.0.join(format!("sample-{i:04}.wav")),
                kind: EntryKind::WaveAudio,
            })
            .collect();
        view.browser.samples.invalidate_listing();
        cx.notify();
    });
    for (selected, offscreen) in [(850, 0), (10, 850)] {
        editor.update(visual, |view, cx| {
            view.browser.selected_index = selected;
            view.browser.results_focused = true;
            view.browser
                .scroll
                .scroll_to_item_strict(offscreen, gpui::ScrollStrategy::Top);
            cx.notify();
        });
        visual.run_until_parked();
        let selector = Box::leak(format!("browser-entry-{selected}").into_boxed_str());
        assert!(visual.debug_bounds(selector).is_none());
        let before = editor.read_with(visual, |view, cx| {
            (
                view.browser_selected_sample(cx),
                view.browser.playback_generation,
                view.browser.playback_path.clone(),
                view.browser.playback_phase,
                view.browser.preview_preparations,
                view.browser.search.read(cx).text().to_owned(),
                view.pending_requests.len(),
                view.pending_audio_commands.len(),
            )
        });
        click_reveal(visual);
        assert!(
            visual.debug_bounds(selector).is_some(),
            "selected sample must be rendered after reveal"
        );
        visual
            .update_window(handle, |_, window, cx| {
                editor.read_with(cx, |view, cx| {
                    assert!(view.browser.results_focus.is_focused(window));
                    assert!(view.browser.results_focused);
                    assert_eq!(view.browser.selected_index, selected);
                    assert_eq!(
                        (
                            view.browser_selected_sample(cx),
                            view.browser.playback_generation,
                            view.browser.playback_path.clone(),
                            view.browser.playback_phase,
                            view.browser.preview_preparations,
                            view.browser.search.read(cx).text().to_owned(),
                            view.pending_requests.len(),
                            view.pending_audio_commands.len()
                        ),
                        before
                    );
                })
            })
            .unwrap();
    }
}

#[gpui::test]
fn browser_reveal_is_inert_without_a_sample_and_rejects_stale_identity(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // No selection: a disabled click must preserve the text input focus and query.
    let hit = visual.debug_bounds("browser-search").unwrap().center();
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
    click_reveal(visual);
    visual
        .update_window(handle, |_, window, cx| {
            editor.read_with(cx, |view, cx| {
                assert!(
                    view.browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window)
                );
                assert!(!view.browser.results_focused);
                assert_eq!(view.browser.search.read(cx).text(), "");
            })
        })
        .unwrap();
    editor.update(visual, |view, cx| {
        view.browser.results_focused = true;
        view.browser.selected_index = 0;
        view.browser.auto_audition = false;
        cx.notify();
    });
    visual.run_until_parked();
    // Results disappear between paint and input. An obsolete stable key cannot
    // select the replacement at index zero, focus it or issue media work.
    visual
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.browser.listing.as_mut().unwrap().entries.clear();
                view.browser.samples.invalidate_listing();
                view.browser.search.read(cx).focus_handle().focus(window);
                view.reveal_browser_sample(&path, window, cx);
                assert!(
                    view.browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window)
                );
                assert!(
                    view.browser
                        .scroll
                        .0
                        .borrow()
                        .deferred_scroll_to_item
                        .is_none()
                );
                assert!(view.browser_selected_sample(cx).is_none());
                assert!(view.pending_requests.is_empty());
            })
        })
        .unwrap();
    visual.run_until_parked();
    click_reveal(visual);
}

#[gpui::test]
fn browser_reveal_resolves_remapped_identity_and_keeps_filters(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, _, directory, path) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser.results_focused = true;
        view.browser.auto_audition = false;
        let key = view.browser_selected_item(cx);
        let listing = view.browser.listing.as_mut().unwrap();
        listing.entries.extend((0..150).map(|i| LibraryEntry {
            name: format!("aaa-{i:04}.wav"),
            path: directory.0.join(format!("aaa-{i:04}.wav")),
            kind: EntryKind::WaveAudio,
        }));
        listing.entries.sort_by(|a, b| a.name.cmp(&b.name));
        view.browser.samples.invalidate_listing();
        // Same stable remapping used by asynchronous browser results.
        view.restore_browser_selection(key, cx);
        assert_eq!(view.browser_selected_sample(cx), Some(path.clone()));
        assert_eq!(view.browser.selected_index, 150);
        view.browser
            .scroll
            .scroll_to_item_strict(0, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-entry-150").is_none());
    click_reveal(visual);
    assert!(visual.debug_bounds("browser-entry-150").is_some());
    assert_eq!(
        visual.rendered_background("browser-entry-150").unwrap(),
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Secondary)).into()
    );
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("loop");
    visual.run_until_parked();
    let row = visual.debug_bounds("browser-entry-0").unwrap().center();
    visual.simulate_click(row, Modifiers::default());
    visual.run_until_parked();
    click_reveal(visual);
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "loop");
        assert_eq!(view.browser_selected_sample(cx), Some(path));
        assert!(view.pending_requests.is_empty());
    });
    // Searching for a missing result has no target and must not clear filters.
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("missing-sample");
    visual.run_until_parked();
    click_reveal(visual);
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "missing-sample");
        assert!(view.browser_selected_sample(cx).is_none());
    });
}
