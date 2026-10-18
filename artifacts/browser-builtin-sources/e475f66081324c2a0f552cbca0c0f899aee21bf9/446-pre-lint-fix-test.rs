use super::*;

#[gpui::test]
fn browser_builtin_source_actual_right_click_has_icon_grid(cx: &mut TestAppContext) {
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("builtin-browser.json");
    let (handle, _) =
        place_icon_preferences_window(cx, &directory, &config, &directory.0.join("recent.json"));
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let row = visual.debug_bounds("browser-place-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    assert!(
        visual.debug_bounds("context-menu-icon-grid").is_some(),
        "Music right-click must open existing bundled icon grid"
    );
}

fn right_click(visual: &mut VisualTestContext, selector: &'static str) {
    let row = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
}

fn choose(visual: &mut VisualTestContext, index: usize) {
    let selector = Box::leak(format!("context-menu-choice-{index}").into_boxed_str());
    let bounds = visual.debug_bounds(selector).unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn browser_builtin_source_icons_persist_target_reset_and_compact_geometry(cx: &mut TestAppContext) {
    use hex_preferences::BrowserBuiltin;
    cx.update(super::super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("builtin-icons.json");
    let recent = directory.0.join("recent.json");
    let mut preferences = hex_preferences::BrowserPlaces::default();
    preferences
        .set_builtin_icon(BrowserBuiltin::Home, Some("future-icon"))
        .unwrap();
    hex_preferences::save(&config, &preferences).unwrap();
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser_place_glyph(&view.browser.home), "□")
    });
    assert_eq!(
        hex_preferences::load(&config)
            .unwrap()
            .builtin_icon(BrowserBuiltin::Home),
        Some("future-icon")
    );
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("unchanged query");
    for (row, choice, builtin, id, width) in [
        (
            "browser-place-0",
            2,
            BrowserBuiltin::Music,
            Some("samples"),
            124.0,
        ),
        (
            "browser-place-1",
            6,
            BrowserBuiltin::Home,
            Some("waveform"),
            23.0,
        ),
        ("browser-place-0", 0, BrowserBuiltin::Music, None, 23.0),
    ] {
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        // Resize deliberately owns non-text navigation focus. Enter search explicitly again.
        visual.simulate_keystrokes("cmd-f");
        let before = editor.read_with(visual, |view, _| {
            (
                view.browser.section.clone(),
                view.browser.playback_generation,
            )
        });
        let bounds = visual.debug_bounds(row).unwrap();
        right_click(visual, row);
        assert!(visual.debug_bounds("context-menu-icon-grid").is_some());
        let remove = visual.debug_bounds("context-menu-choice-8").unwrap();
        let grid = visual.debug_bounds("context-menu-icon-grid").unwrap();
        assert!(remove.top() >= grid.bottom());
        choose(visual, choice);
        assert_eq!(
            hex_preferences::load(&config)
                .unwrap()
                .builtin_icon(builtin),
            id
        );
        assert_eq!(
            editor.read_with(visual, |view, _| (
                view.browser.section.clone(),
                view.browser.playback_generation
            )),
            before
        );
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                assert_eq!(view.browser.search.read(cx).content, "unchanged query");
                assert!(
                    view.browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window)
                );
                assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
            })
        });
        let glyph = visual
            .debug_bounds(if row == "browser-place-0" {
                "browser-picker-glyph-place-0"
            } else {
                "browser-picker-glyph-place-1"
            })
            .unwrap();
        assert_eq!(
            glyph.center().x,
            visual.debug_bounds("browser-control-⚙").unwrap().center().x
        );
        assert!(glyph.left() >= bounds.left() && glyph.right() <= bounds.right());
    }
    let (handle, reopened) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    reopened.read_with(visual, |view, _| {
        assert_eq!(view.browser_place_glyph(&view.browser.home), "≋");
        assert_eq!(view.browser_place_glyph(&view.browser.music), "□");
    });
}

#[gpui::test]
fn browser_builtin_source_remove_restore_retains_files_catalog_and_identity(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("hide-browser.json");
    let recent = directory.0.join("recent.json");
    let custom = directory.0.join("custom");
    std::fs::create_dir(&custom).unwrap();
    let wav = custom.join("keep.wav");
    let bytes = mono_pcm_wav(24);
    std::fs::write(&wav, &bytes).unwrap();
    hex_preferences::save(
        &config,
        &hex_preferences::BrowserPlaces::new([custom.clone()]).unwrap(),
    )
    .unwrap();
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let roots = editor.read_with(visual, |view, _| view.sample_roots());
    let catalog = editor.read_with(visual, |view, _| {
        view.browser
            .samples
            .catalog
            .entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>()
    });
    assert!(catalog.contains(&wav));
    for row in [
        "browser-place-0",
        "browser-place-1",
        "browser-place-2",
        "browser-section-samples",
    ] {
        let bounds = visual.debug_bounds(row).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
        right_click(visual, row);
        if row == "browser-section-samples" {
            let action = visual.debug_bounds("context-menu-item").unwrap();
            visual.simulate_click(action.center(), Modifiers::default());
            visual.run_until_parked();
        } else {
            choose(visual, 8);
        }
        assert!(
            visual.debug_bounds(row).is_none(),
            "removed source row remains visible: {row}"
        );
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                assert_eq!(view.browser.section, BrowserSection::All);
                assert!(
                    !view
                        .browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window)
                );
                assert_eq!(view.sample_roots(), roots);
                assert_eq!(
                    view.browser
                        .samples
                        .catalog
                        .entries()
                        .iter()
                        .map(|entry| entry.path.clone())
                        .collect::<Vec<_>>(),
                    catalog
                );
                assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
            })
        });
        assert_eq!(std::fs::read(&wav).unwrap(), bytes);
    }
    let (handle, reopened) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for row in [
        "browser-place-0",
        "browser-place-1",
        "browser-place-2",
        "browser-section-samples",
    ] {
        assert!(visual.debug_bounds(row).is_none());
    }
    reopened.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::All)
    });
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    for row in [
        "browser-place-visibility-0",
        "browser-place-visibility-1",
        "browser-place-visibility-2",
        "browser-section-visibility-0",
    ] {
        let bounds = visual.debug_bounds(row).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
    }
    for row in [
        "browser-place-0",
        "browser-place-1",
        "browser-place-2",
        "browser-section-samples",
    ] {
        assert!(visual.debug_bounds(row).is_some(), "restore failed: {row}");
    }
    assert_eq!(std::fs::read(&wav).unwrap(), bytes);
    let saved = hex_preferences::load(&config).unwrap();
    assert_eq!(saved.additional(), &[custom.clone()]);
    assert!(!saved.place_hidden(&custom));
    for builtin in [
        hex_preferences::BrowserBuiltin::Home,
        hex_preferences::BrowserBuiltin::Music,
        hex_preferences::BrowserBuiltin::Samples,
    ] {
        assert!(!saved.builtin_hidden(builtin));
    }
}

#[gpui::test]
fn browser_builtin_source_all_first_and_command_f_uses_existing_global_scope(
    cx: &mut TestAppContext,
) {
    use gpui::EntityInputHandler;
    cx.update(super::super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("cmd-f-browser.json");
    let custom = directory.0.join("custom");
    std::fs::create_dir(&custom).unwrap();
    hex_preferences::save(
        &config,
        &hex_preferences::BrowserPlaces::new([custom]).unwrap(),
    )
    .unwrap();
    let (handle, editor) =
        place_icon_preferences_window(cx, &directory, &config, &directory.0.join("recent.json"));
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("kick hats");
    for source in [
        "browser-section-samples",
        "browser-section-devices",
        "browser-section-projects",
        "browser-place-0",
        "browser-place-1",
        "browser-place-2",
        "browser-collection-0",
    ] {
        let bounds = visual.debug_bounds(source).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-f");
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| {
                assert_eq!(view.browser.section, BrowserSection::All);
                assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
            });
            let input = editor.read(cx).browser.search.clone();
            input.update(cx, |input, cx| {
                assert_eq!(input.content, "kick hats");
                assert!(input.focus_handle().is_focused(window));
                assert_eq!(
                    input.selected_text_range(false, window, cx).unwrap().range,
                    0..9
                );
            });
        });
    }
    // Existing configuration may hide All; Command-F must reveal it again without changing query.
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    let visibility = visual.debug_bounds("browser-section-visibility-3").unwrap();
    visual.simulate_click(visibility.center(), Modifiers::default());
    assert!(visual.debug_bounds("browser-section-all").is_none());
    visual.simulate_keystrokes("cmd-f");
    assert!(visual.debug_bounds("browser-section-all").is_some());
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    let all = visual.debug_bounds("browser-section-all").unwrap();
    for row in [
        "browser-section-samples",
        "browser-section-devices",
        "browser-section-projects",
    ] {
        assert!(visual.debug_bounds(row).unwrap().top() > all.top());
    }
    let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let target = divider.center() + point(px(23.0) - sidebar.size.width, px(0.0));
    visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    assert!(
        visual.debug_bounds("browser-section-all").unwrap().top()
            < visual
                .debug_bounds("browser-section-samples")
                .unwrap()
                .top()
    );
}
