//! Shared button/key action over the real filtered virtual list and focus scopes.
use super::*;

fn click_random(visual: &mut VisualTestContext) {
    let button = visual.debug_bounds("browser-random-sample").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn random_button_and_r_reveal_offscreen_audio_and_share_auto_audition(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, path) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.listing.as_mut().unwrap().entries = (0..1000)
            .map(|i| LibraryEntry {
                name: format!("row-{i:04}"),
                path: if i == 850 {
                    path.clone()
                } else {
                    directory.0.join(format!("row-{i:04}"))
                },
                kind: if i == 850 {
                    EntryKind::WaveAudio
                } else {
                    EntryKind::Directory
                },
            })
            .collect();
        view.browser.samples.invalidate_listing();
        view.browser.reset_results();
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-entry-850").is_none());
    let state = editor.read_with(visual, |view, _| view.state.clone().unwrap());
    click_random(visual);
    let index = editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser_selected_sample(cx).as_ref(), Some(&path));
        assert_eq!(view.browser.playback_generation, 0);
        assert!(view.browser.results_focused);
        assert!(view.browser.selected_index > 500);
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &state));
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert!(view.last_enqueued_import_to_track.is_none());
        assert!(view.last_enqueued_import_to_new_track.is_none());
        view.browser.selected_index
    });
    let selector = Box::leak(format!("browser-entry-{index}").into_boxed_str());
    assert!(visual.debug_bounds(selector).is_some());
    editor.update(visual, |view, _| view.browser.auto_audition = true);
    visual.simulate_keystrokes("r");
    visual.run_until_parked();
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser_selected_sample(cx).as_ref(), Some(&path));
        assert_eq!(view.browser.playback_generation, 1);
        assert_eq!(view.browser.playback_path.as_ref(), Some(&path));
        assert!(view.pending_requests.is_empty());
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &state));
    });
    click_random(visual);
    assert_eq!(
        editor.read_with(visual, |view, _| view.browser.playback_generation),
        2
    );
}

#[gpui::test]
fn random_pick_obeys_all_search_projection_and_empty_results_leave_playback_unchanged(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, path) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.section = BrowserSection::All;
        view.browser.listing.as_mut().unwrap().entries = vec![
            LibraryEntry {
                name: "utility folder".into(),
                path: directory.0.join("utility folder"),
                kind: EntryKind::Directory,
            },
            LibraryEntry {
                name: "utility audio.wav".into(),
                path: path.clone(),
                kind: EntryKind::WaveAudio,
            },
            LibraryEntry {
                name: "utility project.hex".into(),
                path: directory.0.join("project.hex"),
                kind: EntryKind::Project,
            },
        ];
        view.browser.samples.invalidate_listing();
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("utility", cx));
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(visual, |view, cx| {
        assert!(
            view.browser_rows(cx).files.start > 0,
            "native devices precede files for this All search"
        )
    });
    click_random(visual);
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser_selected_sample(cx).as_ref(), Some(&path));
        assert!(view.browser.selected_index >= view.browser_rows(cx).files.start);
        assert_eq!(view.browser.playback_generation, 0);
    });
    editor.update(visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("no matching result", cx));
        cx.notify();
    });
    visual.run_until_parked();
    let snapshot = editor.read_with(visual, |view, _| {
        (
            view.browser.selected_index,
            view.browser.results_focused,
            view.browser.playback_generation,
            view.browser.playback_path.clone(),
            view.browser.playback_phase,
        )
    });
    click_random(visual);
    editor.read_with(visual, |view, _| {
        assert_eq!(
            (
                view.browser.selected_index,
                view.browser.results_focused,
                view.browser.playback_generation,
                view.browser.playback_path.clone(),
                view.browser.playback_phase
            ),
            snapshot
        )
    });
}

#[gpui::test]
fn r_types_in_search_and_does_not_pick_from_arrangement_focus(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let search = visual.debug_bounds("browser-search").unwrap().center();
    visual.simulate_click(search, Modifiers::default());
    visual.simulate_keystrokes("r");
    visual.run_until_parked();
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "r");
        assert_eq!(view.browser.playback_generation, 0);
    });
    editor.update(visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| input.clear(cx));
        view.browser.auto_audition = true;
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, cx| editor.read(cx).focus.focus(window));
    visual.simulate_keystrokes("r");
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(visual, |view, _| view.browser.playback_generation),
        0
    );
}
