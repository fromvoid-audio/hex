
#[gpui::test]
fn browser_samples_count_tracks_hierarchy_items_and_flat_sample_matches(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let first = PreferencesTestDirectory::new();
    let second = PreferencesTestDirectory::new();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.places = vec![first.0.clone(), second.0.clone()];
        view.browser.directory = first.0.clone();
        view.browser.section = BrowserSection::Samples;
        view.browser.search.update(cx, |input, _| input.content.clear());
        view.browser.samples.replace(Default::default());
        view.browser.tree.source_changed();
        assert_eq!(view.browser_visible_entries(cx).len(), 2);
        assert_eq!(view.sample_results_count_text(cx), "2 items");
        cx.notify();
    });
    let nested = first.0.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("kick.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(&mut visual, |view, cx| view.expand_browser_folder(first.0.clone(), cx));
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|entry| entry.kind == EntryKind::Directory));
        assert_eq!(view.sample_results_count_text(cx), "3 items");
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| {
            input.content = "kick".into();
            cx.notify();
        });
        view.refresh_sample_catalog(cx);
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
        assert_eq!(view.sample_results_count_text(cx), "1 samples");
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, _| input.content = "missing".into());
        assert!(view.browser_visible_entries(cx).is_empty());
        assert_eq!(view.sample_results_count_text(cx), "0 samples");
    });
}
