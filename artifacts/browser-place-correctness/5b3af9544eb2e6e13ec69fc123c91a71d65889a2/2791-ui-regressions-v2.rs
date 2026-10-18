
#[gpui::test]
fn place_correctness_reselect_gates_cached_rows_and_old_audition_callbacks(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.browser.section = BrowserSection::Place(directory.0.clone());
        view.browser.loading = false;
        view.browser.error = None;
        view.browser.scans = Default::default();
        // Hold the worker lane so the fresh request remains deterministically pending.
        let old = view.browser.scans.request(directory.0.clone()).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&directory.0).unwrap();
        view.select_browser_source(BrowserSection::Place(directory.0.clone()), cx);
        assert!(view.browser.loading);
        assert!(
            view.browser_visible_entries(cx).is_empty(),
            "same-root reselect exposes retained offline listing before availability returns"
        );
        view.browser
            .search
            .update(cx, |input, _| input.content = "loop".into());
        assert!(
            view.browser_visible_entries(cx).is_empty(),
            "query exposes cached offline index before availability returns"
        );
        let generation = view.browser.playback_generation;
        // A closure painted before reselect may still carry the former path.
        view.play_browser_sample_from(path.clone(), 0.5, cx);
        assert_eq!(
            view.browser.playback_generation, generation,
            "old waveform callback admitted stale preview"
        );
        view.import_browser_audio(path.clone());
        assert!(
            view.last_enqueued_import_to_track.is_none(),
            "old result callback admitted stale import"
        );
        let (current, fresh) = view.browser.scans.complete(old.generation);
        assert!(!current);
        let fresh = fresh.unwrap();
        view.browser_scan_finished(fresh, hex_library::scan_directory(&directory.0), cx);
        assert!(!view.browser.loading);
        assert!(view.browser.error.is_some());
        assert!(view.browser_visible_entries(cx).is_empty());
        let generation = view.browser.playback_generation;
        view.play_browser_sample_from(path, 0.5, cx);
        assert_eq!(view.browser.playback_generation, generation);
        cx.notify();
    });
}

#[gpui::test]
fn place_correctness_refresh_retains_identity_only_after_current_success(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.section = BrowserSection::Place(directory.0.clone());
        view.browser.loading = false;
        view.browser.error = None;
        view.browser
            .search
            .update(cx, |input, _| input.content = "loop".into());
        view.browser.results_focused = true;
        view.browser.selected_index = 0;
        view.browser.scans = Default::default();
        let old = view.browser.scans.request(directory.0.clone()).unwrap();
        view.request_browser_scan(directory.0.clone(), cx);
        assert!(view.browser_visible_entries(cx).is_empty());
        // A concurrent catalog refresh must not discard the privately saved identity.
        std::fs::write(directory.0.join("another.wav"), mono_pcm_wav(16)).unwrap();
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
            cx,
        );
        // The superseded worker must not reauthorize rows.
        let (current, fresh) = view.browser.scans.complete(old.generation);
        assert!(!current);
        assert!(view.browser_visible_entries(cx).is_empty());
        view.browser_scan_finished(
            fresh.unwrap(),
            hex_library::scan_directory(&directory.0),
            cx,
        );
        assert!(!view.browser.loading);
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
        assert!(view.browser.results_focused);
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            path
        );
        // An available, freshly validated row still admits manual offset preview.
        let generation = view.browser.playback_generation;
        view.play_browser_sample_from(path, 0.5, cx);
        assert_eq!(view.browser.playback_generation, generation + 1);
        cx.notify();
    });
}
