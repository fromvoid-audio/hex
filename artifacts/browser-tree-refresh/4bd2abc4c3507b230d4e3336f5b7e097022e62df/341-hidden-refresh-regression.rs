
#[gpui::test]
fn browser_tree_refresh_updates_expanded_branch_hidden_by_indexed_query(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, visible_sample) = browser_drag_fixture(cx);
    let branch = directory.0.join("branch");
    std::fs::create_dir(&branch).unwrap();
    let old = branch.join("old.wav");
    let fresh = branch.join("fresh.wav");
    std::fs::write(&old, mono_pcm_wav(16)).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.select_browser_source(BrowserSection::Samples, cx);
        cx.notify();
    });
    visual.run_until_parked();
    for row in [0, 1] {
        let selector = Box::leak(format!("browser-disclosure-{row}").into_boxed_str());
        let disclosure = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    let before = editor.read_with(&visual, |view, cx| {
        assert!(view.browser_visible_entries(cx).iter().any(|entry| entry.path == old));
        view.browser.tree.requests_started
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| {
            input.content = "loop".into();
            cx.notify();
        });
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, visible_sample);
    });
    std::fs::remove_file(&old).unwrap();
    std::fs::write(&fresh, mono_pcm_wav(32)).unwrap();
    editor.update(&mut visual, |view, cx| view.refresh_browser_tree(cx));
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.browser.samples.catalog.entries().iter().any(|entry| entry.path == fresh));
        println!("hidden-branch refresh started {} folder requests", view.browser.tree.requests_started - before);
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| input.clear(cx));
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-entry-label-2").is_some(), "expanded child row must be rendered after clearing query");
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert!(entries.iter().any(|entry| entry.path == fresh), "clearing query restored stale cached contents despite refresh");
        assert!(!entries.iter().any(|entry| entry.path == old));
        assert_eq!(view.browser.tree.requests_started - before, 2, "only the two already-expanded folders need enumeration");
        assert!(view.pending_audio_commands.is_empty());
        assert!(view.last_enqueued_import_to_track.is_none());
    });
}
