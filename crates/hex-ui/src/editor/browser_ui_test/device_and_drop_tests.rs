use super::*;

#[gpui::test]
fn browser_sample_drop_above_first_track_shows_line_and_inserts_before_it(cx: &mut TestAppContext) {
    let (handle, editor, tracks, _directory, path) =
        browser_drag_fixture_with_tracks(cx, &["first", "second"]);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual.debug_bounds("browser-entry-0").expect("sample row");
    let selector = Box::leak(format!("arrangement-lane-{}", tracks[0].value()).into_boxed_str());
    let first = visual.debug_bounds(selector).expect("first lane");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(
        first.origin.x + first.size.width / 2.0,
        first.origin.y + px(1.0),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insert-preview").is_some());
    assert!(visual.debug_bounds("clip-drop-preview").is_none());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        let (dropped_path, anchor, _) = view
            .test_probes
            .last_enqueued_import_to_new_track
            .as_ref()
            .expect("new track import");
        assert_eq!(dropped_path, &path);
        assert_eq!(*anchor, Some(tracks[0]));
        assert!(view.test_probes.last_enqueued_import_to_track.is_none());
    });
}

#[gpui::test]
fn browser_click_and_right_request_preview_without_import_search_arrows_keep_caret(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, track, directory, path) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let row = visual.debug_bounds("browser-entry-0").expect("sample row");
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 1);
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("sample preview"))
        );
        assert!(view.pending_requests.is_empty());
        assert!(view.audio_pub.pending_audio_commands.is_empty());
    });
    // A real second mouse-down with a valid target track must never import.
    editor.update(visual, |view, _| view.selected_track = Some(track));
    for auto in [true, false] {
        editor.update(visual, |view, _| view.browser.auto_audition = auto);
        let (before, state) = editor.read_with(visual, |view, _| {
            (
                view.browser.playback_generation,
                view.state.clone().unwrap(),
            )
        });
        visual.simulate_event(gpui::MouseDownEvent {
            position: row.center(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
            modifiers: Modifiers::default(),
        });
        visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
        editor.read_with(visual, |view, _| {
            assert_eq!(view.browser.playback_generation, before + u64::from(auto));
            assert!(
                view.pending_requests.is_empty(),
                "double-click cannot enqueue import"
            );
            assert!(view.test_probes.last_enqueued_import_to_track.is_none());
            assert!(view.test_probes.last_enqueued_import_to_new_track.is_none());
            assert!(view.audio_pub.pending_audio_commands.is_empty());
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &state));
            assert!(view.browser.results_focused);
        });
    }
    editor.update(visual, |view, _| view.browser.auto_audition = true);
    let contexts = visual
        .update_window(handle, |_, window, cx| {
            let view = editor.read(cx);
            (
                window.context_stack(),
                view.browser_key_context(window, cx),
                view.browser.results_focused,
            )
        })
        .unwrap();
    assert_eq!(contexts.1, "hex_browser", "{contexts:?}");
    assert!(contexts.2, "{contexts:?}");
    visual.simulate_keystrokes("right");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 3, "{contexts:?}");
        assert!(view.pending_requests.is_empty());
    });
    // Clicking search restores text focus. Left/right then insert at the
    // restored caret; neither key may send an audition request.
    let search = visual.debug_bounds("browser-search").expect("search field");
    visual.simulate_mouse_down(search.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(search.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("right l o left right o");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "loo");
        assert_eq!(view.browser.playback_generation, 3);
    });
    std::fs::write(directory.0.join("second.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(visual, |view, cx| {
        // This section verifies manual Right-arrow independently of auto selection.
        view.browser.auto_audition = false;
        view.browser.search.update(cx, |input, cx| input.clear(cx));
        view.refresh_sample_catalog(cx);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("down right up right");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.selected_index, 0);
        assert_eq!(view.browser.playback_generation, 5);
        assert!(view.pending_requests.is_empty());
    });
    // Enter keeps the existing explicit import path.
    editor.update(visual, |view, _| view.selected_track = Some(track));
    visual.simulate_keystrokes("enter");
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 5);
        let (imported_path, imported_track, _, _) = view
            .test_probes
            .last_enqueued_import_to_track
            .as_ref()
            .expect("enter enqueues import to selected track");
        assert_eq!(imported_path, &path);
        assert_eq!(*imported_track, track);
    });
}

#[gpui::test]
fn browser_right_opens_selected_folder_without_sample_or_project_commands(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let folder = directory.0.join("folder");
    std::fs::create_dir(&folder).unwrap();
    let child = folder.join("project.hex");
    std::fs::write(&child, []).unwrap();
    let visual = &mut VisualTestContext::from_window(handle, cx);
    editor.update(visual, |view, cx| {
        view.browser.section = BrowserSection::Projects;
        view.browser.listing.as_mut().unwrap().entries = vec![LibraryEntry {
            name: "folder".into(),
            path: folder.clone(),
            kind: EntryKind::Directory,
        }];
        cx.notify();
    });
    visual.run_until_parked();
    let disclosure = visual.debug_bounds("browser-disclosure-0").unwrap();
    assert_eq!(
        editor.read_with(visual, |view, _| view.browser.tree.disclosure(&folder)),
        super::super::tree::Disclosure::Collapsed
    );
    struct RestoreTheme(hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            crate::theme::apply_preferences(self.0);
        }
    }
    let original = crate::theme::preferences();
    let _restore = RestoreTheme(original);
    for (background, text) in [(0x20, 0xe0), (0xe0, 0x20)] {
        let mut preferences = original;
        preferences.background = hex_preferences::RgbColor::new(background, background, background);
        preferences.text = hex_preferences::RgbColor::new(text, text, text);
        crate::theme::apply_preferences(preferences);
        editor.update(visual, |_, cx| cx.notify());
        visual.run_until_parked();
        let expected = gpui::Hsla::from(gpui::rgb(crate::theme::color(
            crate::theme::PaletteRole::Muted,
        )));
        assert!(
            visual
                .rendered_monochrome_colors("browser-disclosure-0")
                .contains(&expected)
        );
        assert_eq!(
            visual.debug_bounds("browser-disclosure-0"),
            Some(disclosure)
        );
    }
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    assert!(
        !visual
            .rendered_monochrome_colors("browser-disclosure-0")
            .is_empty()
    );
    assert_eq!(
        visual.debug_bounds("browser-disclosure-0"),
        Some(disclosure)
    );
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.directory, directory.0);
        assert_eq!(
            view.browser.tree.disclosure(&folder),
            super::super::tree::Disclosure::Expanded
        );
        assert_eq!(view.browser_visible_entries(cx)[1].path, child);
        assert_eq!(view.browser.playback_generation, 0);
        assert!(view.pending_requests.is_empty());
        assert!(view.audio_pub.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn split_browser_keeps_results_between_sidebar_tags_and_preview(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let sidebar = visual.debug_bounds("browser-sidebar").expect("sidebar");
    let row = visual.debug_bounds("browser-entry-0").expect("sample row");
    let tags = visual
        .debug_bounds("browser-tag-selector")
        .expect("tag selector");
    let preview = visual
        .debug_bounds("browser-selection-preview")
        .expect("preview");
    assert!(row.left() >= sidebar.right());
    assert!(row.top() >= tags.bottom());
    assert!(row.bottom() <= preview.top());
    let toggle = visual
        .debug_bounds("browser-control-≡")
        .expect("filter toggle");
    visual.simulate_mouse_down(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-tag-selector").is_none());
    let before_editor_row = visual.debug_bounds("browser-entry-0").expect("sample row");
    let before_editor_preview = visual
        .debug_bounds("browser-selection-preview")
        .expect("preview");
    let open = visual
        .debug_bounds("browser-control-⋮")
        .expect("editor toggle");
    visual.simulate_mouse_down(open.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(open.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let tag_editor = visual
        .debug_bounds("browser-tag-editor")
        .expect("tag editor");
    let row = visual.debug_bounds("browser-entry-0").expect("sample row");
    assert!(
        row.right() <= tag_editor.left(),
        "row {row:?}, tag editor {tag_editor:?}"
    );
    assert_eq!(
        row, before_editor_row,
        "opening the editor preserves results geometry"
    );
    assert_eq!(
        visual
            .debug_bounds("browser-selection-preview")
            .expect("preview"),
        before_editor_preview
    );
    let close = visual
        .debug_bounds("browser-control-⊗")
        .expect("close editor");
    visual.simulate_mouse_down(close.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(close.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-entry-0").expect("sample row"),
        before_editor_row
    );
    // Reopen through the selected item's mini strip as well.
    let open = visual
        .debug_bounds("browser-control-⋯")
        .expect("mini editor toggle");
    visual.simulate_mouse_down(open.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(open.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-entry-0").expect("sample row"),
        before_editor_row
    );
    // The outer edge follows the pointer even with the extra editor column open.
    let outer = visual
        .debug_bounds("browser-resize-handle")
        .expect("outer divider");
    let target = point(outer.left() - px(30.0), outer.center().y);
    visual.simulate_mouse_down(outer.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let resized = visual
        .debug_bounds("browser-resize-handle")
        .expect("resized outer divider");
    assert!((resized.left() - target.x).abs() < px(1.0));
    editor.update(visual, |view, cx| {
        view.browser.width = super::super::geometry::MIN_WIDTH;
        cx.notify();
    });
    visual.run_until_parked();
    let row = visual
        .debug_bounds("browser-entry-0")
        .expect("compact sample row");
    let preview = visual
        .debug_bounds("browser-selection-preview")
        .expect("compact preview");
    assert!(row.size.width > px(0.0));
    assert!(row.bottom() <= preview.top());
}

#[gpui::test]
fn browser_picker_resizes_to_icons_and_back_without_changing_browser_width(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let sidebar = visual.debug_bounds("browser-sidebar").expect("sidebar");
    let row = visual.debug_bounds("browser-entry-0").expect("sample row");
    let outer_before = visual
        .debug_bounds("browser-resize-handle")
        .expect("outer divider");
    let preview_before = visual
        .debug_bounds("browser-selection-preview")
        .expect("preview");
    let handle = visual
        .debug_bounds("browser-sidebar-resize")
        .expect("picker divider");
    let target = point(sidebar.left() + px(20.0), handle.center().y);
    visual.simulate_mouse_down(handle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let compact = visual.debug_bounds("browser-sidebar").expect("icon rail");
    assert_eq!(
        compact.size.width,
        visual.debug_bounds("browser-control-⚙").unwrap().size.width + px(1.0)
    );
    let compact_row = visual
        .debug_bounds("browser-entry-0")
        .expect("expanded results");
    assert!(compact_row.left() < row.left());
    let compact_preview = visual
        .debug_bounds("browser-selection-preview")
        .expect("wider preview");
    let outer = visual
        .debug_bounds("browser-resize-handle")
        .expect("outer divider");
    assert_eq!(outer, outer_before);
    assert!(compact_preview.size.width > preview_before.size.width);
    assert_eq!(compact_preview.right(), outer.left());
    assert_eq!(compact_preview.left(), compact_row.left());
    let section = visual
        .debug_bounds("browser-section-projects")
        .expect("projects icon");
    visual.simulate_mouse_move(section.center(), None, Modifiers::default());
    visual.simulate_mouse_down(section.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(section.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Projects)
    });
    visual.run_until_parked();
    let handle = visual
        .debug_bounds("browser-sidebar-resize")
        .expect("compact divider");
    let target = point(sidebar.right() + px(2.0), handle.center().y);
    visual.simulate_mouse_down(handle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("browser-sidebar")
            .expect("restored sidebar")
            .size
            .width,
        sidebar.size.width
    );
}

#[gpui::test]
fn browser_audition_strip_is_one_bounded_row(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [220.0, 300.0, 480.0] {
        editor.update(&mut visual, |view, cx| {
            view.browser.width = width;
            cx.notify();
        });
        visual.run_until_parked();
        let preview = visual
            .debug_bounds("browser-selection-preview")
            .expect("preview");
        assert!(
            preview.size.height <= px(50.0),
            "compact preview was {preview:?}"
        );
        let row = visual
            .debug_bounds("browser-audition-row")
            .expect("horizontal audition row");
        let auto = visual
            .debug_bounds("browser-audition-auto")
            .expect("auto toggle");
        let transport = visual
            .debug_bounds("browser-audition-transport")
            .expect("play/stop");
        let waveform = visual
            .debug_bounds("browser-audition-waveform")
            .expect("tiny waveform");
        let volume = visual
            .debug_bounds("browser-audition-volume")
            .expect("volume slider");
        assert_eq!(row.size.height, px(22.0));
        assert!(row.top() >= preview.top() && row.bottom() <= preview.bottom());
        assert!(auto.right() <= transport.left() && transport.right() <= waveform.left());
        assert!(waveform.right() <= volume.left());
        assert_eq!(volume.right(), row.right());
        assert!(auto.left() >= row.left());
        for control in [auto, transport, waveform, volume] {
            assert!(control.top() >= row.top() && control.bottom() <= row.bottom());
        }
    }
}

#[gpui::test]
fn browser_audition_volume_drag_preserves_playback_position_and_selection_level(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    std::fs::write(directory.0.join("next.wav"), mono_pcm_wav(16)).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.refresh_browser_tree(cx);
        view.browser.playback_fraction = 0.25;
        cx.notify();
    });
    visual.run_until_parked();
    let volume = visual.debug_bounds("browser-audition-volume").unwrap();
    assert_eq!(
        visual.debug_bounds("browser-audition-volume-fill"),
        Some(volume)
    );
    let down = volume.center();
    let quieter = down + point(px(0.0), px(11.0));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(quieter, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let fill = visual.debug_bounds("browser-audition-volume-fill").unwrap();
    assert_eq!(fill.left(), volume.left());
    assert_eq!(fill.right(), volume.right());
    assert_eq!(fill.bottom(), volume.bottom());
    assert_eq!(fill.size.height, volume.size.height / 2.0);
    visual.simulate_mouse_up(quieter, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.audition_gain, 0.5);
        assert_eq!(view.browser.playback_generation, 0);
        assert_eq!(view.browser.playback_fraction, 0.25);
        assert!(!view.browser.audition_gain_dragging);
    });

    let next = visual.debug_bounds("browser-entry-1").unwrap().center();
    visual.simulate_mouse_down(next, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(next, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.selected_index, 1);
        assert_eq!(view.browser.audition_gain, 0.5);
        assert_eq!(view.browser.playback_generation, 0);
    });
}

#[gpui::test]
fn browser_audition_toggle_keeps_manual_play_and_result_focus(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let auto = visual.debug_bounds("browser-audition-auto").unwrap();
    visual.simulate_mouse_down(auto.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(auto.center(), MouseButton::Left, Modifiers::default());
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(!view.browser.auto_audition);
        assert_eq!(view.browser.playback_generation, 0);
        assert!(view.browser.results_focused);
    });
    let transport = visual.debug_bounds("browser-audition-transport").unwrap();
    visual.simulate_mouse_down(transport.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(transport.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 1);
        assert!(view.browser.results_focused);
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Unavailable
        );
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("sample preview"))
        );
        assert_eq!(view.status.as_str(), "sample preview unavailable");
        assert!(view.audio_pub.pending_audio_commands.is_empty());
    });
    // The control must retain browser key focus so Right remains manual audition.
    visual.simulate_keystrokes("right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 2)
    });
    visual.simulate_mouse_down(auto.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(auto.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(view.browser.auto_audition);
        assert_eq!(view.browser.playback_generation, 3);
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn browser_auto_navigation_auditions_new_selection_but_not_boundary_repeats(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    std::fs::write(directory.0.join("second.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.refresh_browser_tree(cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.results_focused = false;
        view.browser.selected_index = 0;
        view.move_browser_selection(true, cx);
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_generation, 1)
    });
    editor.update(&mut visual, |view, cx| {
        view.move_browser_selection(true, cx)
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.selected_index, 1);
        assert_eq!(view.browser.playback_generation, 2);
    });
    editor.update(&mut visual, |view, cx| {
        view.move_browser_selection(true, cx)
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.browser.playback_generation, 2,
            "holding Down at the last row cannot replay it"
        )
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.move_browser_selection(false, cx);
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.selected_index, 0);
        assert_eq!(view.browser.playback_generation, 2);
    });
}

#[gpui::test]
fn browser_result_reset_cancels_preparing_and_playing_audition(cx: &mut TestAppContext) {
    let (handle, editor, _, _, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        for phase in [
            hex_audio::sample_preview::Phase::Preparing,
            hex_audio::sample_preview::Phase::Playing,
        ] {
            view.browser.playback_phase = phase;
            view.browser.playback_output = Some((1, 1));
            view.browser.playback_path = Some("obsolete.wav".into());
            let before = view.browser.playback_generation;
            view.reset_browser_results();
            assert_eq!(view.browser.playback_generation, before + 1);
            assert!(view.browser.playback_output.is_none());
            assert!(view.browser.playback_pending.is_none());
            assert!(view.browser.playback_path.is_none());
            assert_eq!(
                view.browser.playback_phase,
                hex_audio::sample_preview::Phase::Idle
            );
        }
    });
}

#[gpui::test]
fn browser_superseded_preview_preparations_are_cancelled(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    std::fs::write(directory.0.join("second.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.refresh_browser_tree(cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        // An audition still decoding when the selection moves on is cancelled.
        let audition = Arc::new(AtomicBool::new(false));
        view.browser.playback_in_flight = Some(Arc::clone(&audition));
        view.browser.results_focused = false;
        view.browser.selected_index = 0;
        view.move_browser_selection(true, cx);
        assert!(audition.load(Ordering::Relaxed), "new selection cancels it");
        let stopped = Arc::new(AtomicBool::new(false));
        view.browser.playback_in_flight = Some(Arc::clone(&stopped));
        view.cancel_browser_sample_playback();
        assert!(stopped.load(Ordering::Relaxed), "stop cancels it");
        view.browser.playback_in_flight = None;

        let key = |name: &str| super::super::PreviewKey {
            path: directory.0.join(name),
            sample_rate: 48_000,
        };
        let job = |key| super::super::PreviewRequest {
            key,
            slot: super::super::PreviewSlot::default(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let selected = job(key("selected.wav"));
        view.browser.preview_selected = Some(selected.key.clone());
        view.browser.preview_in_flight = Some(selected.clone());
        view.queue_browser_audio_preview(
            key("hovered.wav"),
            super::super::PreviewSlot::default(),
            cx,
        );
        assert!(
            !selected.cancel.load(Ordering::Relaxed),
            "the selected row keeps its waveform job"
        );
        let hovered = job(key("hovered.wav"));
        view.browser.preview_in_flight = Some(hovered.clone());
        view.queue_browser_audio_preview(
            key("later.wav"),
            super::super::PreviewSlot::default(),
            cx,
        );
        assert!(
            hovered.cancel.load(Ordering::Relaxed),
            "a superseded hover waveform job is cancelled"
        );
        assert_eq!(
            view.browser
                .preview_pending
                .as_ref()
                .map(|pending| &pending.key),
            Some(&key("later.wav"))
        );
        view.browser.preview_in_flight = None;
        view.browser.preview_pending = None;
    });
}

#[gpui::test]
fn browser_waveform_position_observes_rendered_pcm_not_click_or_decode(cx: &mut TestAppContext) {
    use hex_audio::sample_preview::Phase;
    let (_, editor, _, _, _) = browser_drag_fixture(cx);
    let (mut control, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
    let token = control.sample_preview.request().unwrap();
    control
        .sample_preview
        .publish_from(
            token,
            hex_engine::Audio::new(48_000, vec![[0.5; 2]; 10]).unwrap(),
            5,
        )
        .unwrap();
    editor.update(cx, |view, _| {
        view.browser.playback_output = Some((1, token));
        assert!(view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        assert_eq!(view.browser.playback_phase, Phase::Preparing);
        assert_eq!(view.browser.playback_fraction, 0.5);
        assert!(!view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
    });
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    editor.update(cx, |view, _| {
        assert!(view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        assert_eq!(view.browser.playback_phase, Phase::Playing);
        assert_eq!(view.browser.playback_fraction, 0.7);
    });
    callback.render(&mut [[0.0; 2]; 8]).unwrap();
    editor.update(cx, |view, _| {
        assert!(view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        assert_eq!(view.browser.playback_phase, Phase::Finished);
        assert_eq!(view.browser.playback_fraction, 1.0);
        assert!(view.browser.playback_output.is_none());
        assert!(!view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        view.cancel_browser_sample_playback();
        assert_eq!(view.browser.playback_fraction, 0.0);
        assert!(!view.accept_browser_sample_snapshot(Some(control.sample_preview.snapshot())));
        assert!(view.audio_pub.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn browser_new_selection_clears_completed_position_without_auto_audition(cx: &mut TestAppContext) {
    let (_, editor, _, _, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.playback_phase = hex_audio::sample_preview::Phase::Finished;
        view.browser.playback_fraction = 1.0;
        view.browser.playback_path = Some("old.wav".into());
        let generation = view.browser.playback_generation;
        view.browser_selection_audition(Some("new.wav".into()), cx);
        assert_eq!(view.browser.playback_generation, generation);
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Idle
        );
        assert_eq!(view.browser.playback_fraction, 0.0);
        assert!(view.browser.playback_path.is_none());
        assert!(view.audio_pub.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn browser_source_groups_have_exactly_one_persistent_selected_row(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| view.browser.auto_audition = false);
    visual.run_until_parked();
    let selectors = [
        "browser-section-samples",
        "browser-section-devices",
        "browser-section-projects",
        "browser-section-all",
        "browser-place-0",
        "browser-collection-0",
    ];
    let selected = gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Secondary)).into();
    let idle = gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Surface)).into();
    let minimum = f32::from(visual.debug_bounds("browser-control-⚙").unwrap().size.width) + 1.0;
    let places = editor.read_with(&visual, |view, _| view.browser.places.clone());
    for width in [124.0, 64.0, 63.0, minimum] {
        let compact = width < 64.0;
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert!(view.browser.layout.sidebar_drag.is_some())
        });
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("browser-sidebar").unwrap().size.width,
            px(width)
        );
        editor.read_with(&visual, |view, _| {
            assert!(view.browser.layout.sidebar_drag.is_none());
            assert_eq!(view.browser.places, places);
            assert!(!view.browser.auto_audition);
        });
        for active in [0, 4, 1, 4, 5, 0, 5, 4, 2, 3] {
            let bounds = visual.debug_bounds(selectors[active]).unwrap();
            let before_hover = editor.read_with(&visual, |view, _| {
                (
                    view.browser.section.clone(),
                    view.browser.playback_generation,
                    view.browser.preview_preparations,
                )
            });
            // Exercise the full row hit target, including blank space beyond its icon.
            let hit = point(bounds.right() - px(1.0), bounds.center().y);
            visual.simulate_mouse_move(hit, None, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert_eq!(
                    (
                        view.browser.section.clone(),
                        view.browser.playback_generation,
                        view.browser.preview_preparations
                    ),
                    before_hover,
                    "picker hover cannot select, audition or prepare samples"
                );
            });
            visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(hit, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
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
                })
            });
            let source = editor.read_with(&visual, |view, _| view.browser.section.clone());
            visual.simulate_keystrokes("down");
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.browser.section, source);
                assert!(view.test_probes.last_enqueued_import_to_track.is_none());
                assert!(view.test_probes.last_enqueued_import_to_new_track.is_none());
            });
            for (index, selector) in selectors.iter().enumerate() {
                assert_eq!(
                    visual.rendered_background(selector),
                    Some(if index == active { selected } else { idle }),
                    "{selector} after selecting {} (compact={compact})",
                    selectors[active]
                );
            }
            editor.read_with(&visual, |view, cx| {
                assert!(view.audio_pub.pending_audio_commands.is_empty());
                if active == 4 {
                    assert!(
                        view.browser_visible_entries(cx)
                            .iter()
                            .all(|entry| entry.path.starts_with(&directory.0))
                    );
                    assert!(view.browser_visible_devices(cx).is_empty());
                }
                if active == 5 {
                    assert!(view.browser_visible_entries(cx).is_empty());
                    assert!(view.browser_visible_devices(cx).is_empty());
                }
            });
        }
    }
}

#[gpui::test]
fn browser_offline_and_removed_place_never_shows_stale_or_second_source(cx: &mut TestAppContext) {
    let (handle, editor, _, first, _) = browser_drag_fixture(cx);
    let offline = PreferencesTestDirectory::new();
    let ghost = offline.0.join("ghost.wav");
    std::fs::write(&ghost, mono_pcm_wav(16)).unwrap();
    let catalog = hex_library::catalog::scan_samples(std::slice::from_ref(&offline.0)).catalog;
    std::fs::remove_file(&ghost).unwrap();
    std::fs::remove_dir(&offline.0).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.places.push(offline.0.clone());
        view.browser.music = offline.0.clone();
        view.browser.home = first.0.clone();
        view.browser.samples.replace(catalog);
        view.browser.search.update(cx, |input, cx| {
            input.set_text("ghost", cx);
            cx.notify();
        });
        view.select_browser_source(BrowserSection::Place(offline.0.clone()), cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser.section,
            BrowserSection::Place(offline.0.clone())
        );
        assert_eq!(view.browser.directory, offline.0);
        assert!(view.browser.error.is_some());
        assert!(
            view.browser_visible_entries(cx).is_empty(),
            "offline place must not expose stale indexed matches"
        );
        assert!(view.browser.listing.is_none());
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.places.retain(|path| *path != offline.0);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Samples);
        assert!(
            view.browser
                .places_status
                .as_ref()
                .unwrap()
                .contains("selected place removed")
        );
    });
}

#[gpui::test]
fn browser_picker_glyph_alignment_and_continuous_minimum_geometry(cx: &mut TestAppContext) {
    let (handle, _editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let control = visual.debug_bounds("browser-control-⚙").unwrap();
    // Existing 22px browser controls supply the usable hit size; the sidebar
    // additionally owns its existing one-pixel right border.
    let minimum = f32::from(control.size.width) + 1.0;
    let rows = [
        ("browser-collection-0", "browser-picker-glyph-collection-0"),
        ("browser-collection-1", "browser-picker-glyph-collection-1"),
        ("browser-collection-2", "browser-picker-glyph-collection-2"),
        ("browser-section-samples", "browser-picker-glyph-section-0"),
        ("browser-section-devices", "browser-picker-glyph-section-1"),
        ("browser-section-projects", "browser-picker-glyph-section-2"),
        ("browser-section-all", "browser-picker-glyph-section-3"),
        ("browser-place-0", "browser-picker-glyph-place-0"),
    ];
    let outer_before = visual.debug_bounds("browser-resize-handle").unwrap();
    let mut measurements = Vec::new();
    for requested in [124.0, 80.0, 64.0, 63.0, 60.0, 48.0, 36.0, minimum] {
        let before = visual.debug_bounds("browser-sidebar").unwrap();
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let target = divider.center() + point(px(requested) - before.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let control = visual.debug_bounds("browser-control-⚙").unwrap();
        let glyphs = rows.map(|(row, glyph)| {
            let row_bounds = visual.debug_bounds(row).unwrap();
            let glyph_bounds = visual.debug_bounds(glyph).unwrap();
            println!("picker requested={requested} actual={} {glyph}: row={row_bounds:?} glyph={glyph_bounds:?} control={control:?}", f32::from(sidebar.size.width));
            (row_bounds, glyph_bounds)
        });
        measurements.push((requested, sidebar, control, glyphs));
        assert_eq!(
            visual.debug_bounds("browser-resize-handle").unwrap(),
            outer_before
        );
    }
    for (requested, sidebar, control, glyphs) in measurements {
        assert_eq!(
            sidebar.size.width,
            px(requested),
            "picker resize must be continuous down to the existing-control-derived minimum"
        );
        for (row, glyph) in glyphs {
            assert_eq!(
                glyph.center().x,
                control.center().x,
                "all picker glyphs must share the control column at width {requested}"
            );
            // GPUI rounds text origins to pixels while font line boxes may be fractional.
            assert!(f32::from(glyph.center().y - row.center().y).abs() <= 0.5);
            assert!(glyph.size.width <= control.size.width);
            assert!(row.contains(&glyph.origin));
            assert!(glyph.right() <= row.right());
            assert!(row.size.width >= control.size.width);
            assert_eq!(row.size.height, px(18.0));
        }
    }
}

#[gpui::test]
fn browser_picker_resize_retains_rows_and_clips_long_names(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.browser.places[0] = directory.0.join("a very long custom place name");
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selectors = [
        "browser-collection-0",
        "browser-collection-2",
        "browser-section-samples",
        "browser-section-all",
        "browser-control-⚙",
        "browser-place-0",
    ];
    // Repeat with the real section-settings controls open: narrowing must not
    // remove that block and move Places either.
    for settings_open in [false, true] {
        if settings_open {
            let gear = visual.debug_bounds("browser-control-⚙").unwrap();
            visual.simulate_click(gear.center(), Modifiers::default());
            visual.run_until_parked();
        }
        let baseline = selectors.map(|selector| visual.debug_bounds(selector).unwrap());
        for width in [124.0, 110.0, 80.0, 64.0, 63.0, 48.0, 27.0, 23.0, 124.0] {
            let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
            let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
            let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
            visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            for (selector, before) in selectors.into_iter().zip(baseline) {
                let after = visual.debug_bounds(selector).unwrap();
                assert_eq!(
                    after.origin, before.origin,
                    "{selector} must not reflow at width {width}"
                );
                assert_eq!(
                    after.size.height, before.size.height,
                    "{selector} row height at {width}"
                );
            }
            for (label, row) in [
                ("browser-picker-label-collection-0", "browser-collection-0"),
                ("browser-picker-label-section-0", "browser-section-samples"),
                ("browser-picker-label-place-0", "browser-place-0"),
            ] {
                let label = visual
                    .debug_bounds(label)
                    .expect("label remains mounted at zero width");
                let row = visual.debug_bounds(row).unwrap();
                let available = (row.right() - label.left()).max(px(0.0));
                assert_eq!(
                    label.size.width, available,
                    "label uses only remaining row width"
                );
                assert!(
                    label.size.height <= row.size.height,
                    "label must stay one line"
                );
                if width <= 27.0 {
                    assert_eq!(
                        label.size.width,
                        px(0.0),
                        "no label space beside icon/gap/border"
                    );
                }
            }
        }
    }
}

#[gpui::test]
fn browser_folder_double_click_expands_inline_without_replacing_surrounding_rows(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, directory, original) = browser_drag_fixture(cx);
    let folder = directory.0.join("a");
    std::fs::create_dir(&folder).unwrap();
    let child = folder.join("kick.wav");
    std::fs::write(&child, mono_pcm_wav(24)).unwrap();
    let sibling = directory.0.join("b");
    std::fs::create_dir(&sibling).unwrap();
    editor.update(cx, |view, cx| {
        view.browser.section = BrowserSection::Place(directory.0.clone());
        view.browser.auto_audition = false;
        view.request_browser_scan(directory.0.clone(), cx);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let before = visual.debug_bounds(lane).expect("arrangement lane");
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_event(gpui::MouseDownEvent {
        position: row.center(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
        modifiers: Modifiers::default(),
    });
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser.directory, directory.0,
            "disclosure must not navigate to another page"
        );
        let entries = view.browser_visible_entries(cx);
        assert_eq!(
            entries.iter().map(|entry| &entry.path).collect::<Vec<_>>(),
            vec![&folder, &child, &sibling, &original]
        );
        assert!(view.pending_requests.is_empty());
        assert!(view.test_probes.last_enqueued_import_to_track.is_none());
    });
    assert_eq!(
        before,
        visual
            .debug_bounds(lane)
            .expect("retained arrangement lane")
    );
}
