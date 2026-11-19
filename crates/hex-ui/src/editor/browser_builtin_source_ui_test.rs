use super::*;

#[gpui::test]
fn editor_asset_source_refuses_missing_catalog(cx: &mut TestAppContext) {
    cx.update(|app| assert!(crate::assets::validate(app).is_err()));
    cx.update(|app| app.use_test_assets(crate::assets::HexAssets));
    cx.update(|app| assert!(crate::assets::validate(app).is_ok()));
}

// This owning fixture binds its actual caller files and catalog jobs to one root.
struct PreferencesTestDirectory(
    std::path::PathBuf,
    std::sync::Arc<crate::editor::test_environment::Environment>,
);
impl PreferencesTestDirectory {
    fn new() -> Self {
        let environment = crate::editor::test_environment::Environment::new().unwrap();
        Self(environment.root().to_owned(), environment)
    }
}

fn place_icon_preferences_window(
    cx: &mut TestAppContext,
    directory: &PreferencesTestDirectory,
    config: &std::path::Path,
    recent: &std::path::Path,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let home = directory.0.join("icon-home");
    let music = home.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(music.join("fixture.wav"), mono_pcm_wav(24)).unwrap();
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_with_browser_paths(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Err("offline icon test".into()),
                    (
                        config.to_owned(),
                        recent.to_owned(),
                        directory.0.join("icon-theme.json"),
                        directory.0.join("project-directory.json"),
                    ),
                    None,
                    crate::editor::test_environment::Inputs {
                        existing_environment: Some(std::sync::Arc::clone(&directory.1)),
                        home: Some(home),
                        ..Default::default()
                    },
                )
            })
        })
        .unwrap();
    editor.read_with(cx, |view, _| {
        assert!(std::sync::Arc::ptr_eq(&directory.1, &view.test_environment));
        assert_eq!(view.browser.test_preferences_path(), config);
        assert_eq!(view.recent_projects.test_path(), recent);
        assert_eq!(
            view.theme_settings.test_path(),
            directory.0.join("icon-theme.json")
        );
        assert_eq!(
            view.project_directory.test_path(),
            directory.0.join("project-directory.json")
        );
        assert_eq!(view.browser.home, directory.0.join("icon-home"));
        assert_eq!(view.browser.music, music);
    });
    (handle, editor)
}

#[gpui::test]
fn browser_builtin_source_actual_right_click_has_icon_grid(cx: &mut TestAppContext) {
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("builtin-browser.json");
    let (handle, editor) =
        place_icon_preferences_window(cx, &directory, &config, &directory.0.join("recent.json"));
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(
        !visual
            .rendered_monochrome_colors("browser-picker-glyph-place-0")
            .is_empty(),
        "the place row paints an embedded vector"
    );
    let row = visual.debug_bounds("browser-place-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    assert!(
        visual.debug_bounds("context-menu-icon-grid").is_some(),
        "Music right-click must open existing bundled icon grid"
    );
    visual.run_until_parked();
    assert!(
        !visual
            .rendered_monochrome_colors("context-menu-choice-1")
            .is_empty(),
        "the icon grid paints an embedded vector"
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
            crate::theme::PaletteRole::Text,
        )));
        for selector in ["browser-picker-glyph-place-0", "context-menu-choice-1"] {
            assert!(
                visual
                    .rendered_monochrome_colors(selector)
                    .contains(&expected),
                "{selector} must use the current text color"
            );
        }
    }
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
        assert_eq!(
            view.browser_place_icon_path(&view.browser.home),
            "icons/browser-place/folder.svg"
        )
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
                assert_eq!(view.browser.search.read(cx).text(), "unchanged query");
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
        assert_eq!(
            view.browser_place_icon_path(&view.browser.home),
            "icons/browser-place/waveform.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&view.browser.music),
            "icons/browser-place/folder.svg"
        );
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
        } else if row == "browser-place-0" {
            // Traverse actual reset -> glyph row -> footer, then dispatch Remove by Enter.
            visual.simulate_keystrokes("right down down enter");
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
    assert_eq!(saved.additional(), std::slice::from_ref(&custom));
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
                assert_eq!(input.text(), "kick hats");
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

#[gpui::test]
fn browser_hidden_source_picker_restores_same_path_visibility_and_preferences(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("picker-browser.json");
    let recent = directory.0.join("recent.json");
    let custom = directory.0.join("custom-picker");
    std::fs::create_dir(&custom).unwrap();
    let wav = custom.join("keep.wav");
    let bytes = mono_pcm_wav(24);
    std::fs::write(&wav, &bytes).unwrap();
    let mut saved = hex_preferences::BrowserPlaces::new([custom.clone()]).unwrap();
    saved.set_icon(&custom, Some("waveform")).unwrap();
    hex_preferences::save(&config, &saved).unwrap();
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (roots, home, music) = editor.read_with(visual, |view, _| {
        (
            view.sample_roots(),
            view.browser.home.clone(),
            view.browser.music.clone(),
        )
    });
    for (row, path) in [
        ("browser-place-2", custom.clone()),
        ("browser-place-0", music),
        ("browser-place-1", home),
    ] {
        let bounds = visual.debug_bounds(row).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        right_click(visual, row);
        choose(visual, 8);
        editor.read_with(visual, |view, _| {
            assert!(!view.browser_place_visible(&path));
            assert_eq!(view.browser.section, BrowserSection::All);
        });
        assert!(visual.debug_bounds(row).is_none());
        editor.update(visual, |view, cx| view.open_browser_folder(cx));
        assert!(
            visual.did_prompt_for_paths(),
            "actual picker request must exist"
        );
        visual.simulate_paths_selection(Some(vec![path.clone()]));
        visual.run_until_parked();
        editor.read_with(visual, |view, _| {
            assert!(
                view.browser_place_visible(&path),
                "picked hidden source must be visible: {path:?}"
            );
            assert_eq!(view.browser.section, BrowserSection::Place(path.clone()));
            assert_eq!(view.sample_roots(), roots);
            assert_eq!(
                view.browser_place_icon_path(&custom),
                "icons/browser-place/waveform.svg"
            );
            assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
        });
        assert!(visual.debug_bounds(row).is_some());
        assert_eq!(std::fs::read(&wav).unwrap(), bytes);
        let deadline = crate::test_wait::hang_deadline();
        loop {
            visual.run_until_parked();
            let saved = hex_preferences::load(&config).unwrap();
            let visible = editor.read_with(visual, |view, _| {
                if path == view.browser.home {
                    !saved.builtin_hidden(hex_preferences::BrowserBuiltin::Home)
                } else if path == view.browser.music {
                    !saved.builtin_hidden(hex_preferences::BrowserBuiltin::Music)
                } else {
                    !saved.place_hidden(&path)
                }
            });
            if visible {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "restored preference save timed out"
            );
            std::thread::yield_now();
        }
        let (reopened_handle, reopened) =
            place_icon_preferences_window(cx, &directory, &config, &recent);
        let reopened_visual = &mut VisualTestContext::from_window(reopened_handle, cx);
        reopened_visual.run_until_parked();
        reopened.read_with(reopened_visual, |view, _| {
            assert!(view.browser_place_visible(&path));
            assert_eq!(view.sample_roots(), roots);
            assert_eq!(
                view.browser_place_icon_path(&custom),
                "icons/browser-place/waveform.svg"
            );
        });
        assert!(reopened_visual.debug_bounds(row).is_some());
        eprintln!("ACTUAL_HIDDEN_PICKER_RESTORED {row}");
    }
}

#[gpui::test]
fn browser_hidden_source_picker_cancel_and_refusal_keep_visible_selection(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("refusal-browser.json");
    let recent = directory.0.join("recent.json");
    let custom = directory.0.join("custom-refusal");
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
    let custom_row = visual.debug_bounds("browser-place-2").unwrap();
    visual.simulate_click(custom_row.center(), Modifiers::default());
    right_click(visual, "browser-place-2");
    choose(visual, 8);
    let deadline = crate::test_wait::hang_deadline();
    loop {
        visual.run_until_parked();
        let ready = editor.update(visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.state.is_some()
                && view.worker_requests_pending == 0
                && hex_preferences::load(&config)
                    .unwrap()
                    .place_hidden(&custom)
        });
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual startup/save did not settle"
        );
        std::thread::yield_now();
    }
    let saved_bytes = std::fs::read(&config).unwrap();
    let (before, roots) = editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::All);
        (view.state.as_ref().unwrap().clone(), view.sample_roots())
    });
    for refusal in ["cancel", "read-only", "loading", "platform-error"] {
        editor.update(visual, |view, cx| {
            view.browser.preferences_writable = refusal != "read-only";
            view.browser.preferences_loading = refusal == "loading";
            view.open_browser_folder(cx);
        });
        assert!(visual.did_prompt_for_paths());
        match refusal {
            "cancel" => visual.simulate_paths_selection(None),
            "platform-error" => visual.simulate_paths_error("fixture picker error"),
            _ => visual.simulate_paths_selection(Some(vec![custom.clone()])),
        }
        visual.run_until_parked();
        editor.read_with(visual, |view, _| {
            assert!(!view.browser_place_visible(&custom), "{refusal}");
            assert_eq!(view.browser.section, BrowserSection::All, "{refusal}");
            assert_eq!(view.sample_roots(), roots);
            assert_eq!(view.worker_requests_pending, 0);
            assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
            let state = view.state.as_ref().unwrap();
            assert_eq!(state.project, before.project);
            assert_eq!(
                (
                    state.revision,
                    state.can_undo,
                    state.can_redo,
                    state.unsaved
                ),
                (
                    before.revision,
                    before.can_undo,
                    before.can_redo,
                    before.unsaved
                )
            );
            if refusal == "platform-error" {
                assert!(
                    view.error
                        .as_deref()
                        .is_some_and(|error| error.contains("fixture picker error"))
                );
            }
        });
        assert!(visual.debug_bounds("browser-place-2").is_none());
        assert_eq!(std::fs::read(&config).unwrap(), saved_bytes);
        assert_eq!(std::fs::read(&wav).unwrap(), bytes);
        eprintln!("ACTUAL_HIDDEN_PICKER_REFUSAL {refusal}");
    }
    editor.update(visual, |view, cx| {
        view.browser.preferences_writable = true;
        view.browser.preferences_loading = false;
        view.select_browser_source(BrowserSection::Samples, cx);
        view.open_browser_folder(cx);
    });
    assert!(visual.did_prompt_for_paths());
    visual.simulate_paths_selection(Some(vec![custom.clone()]));
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert!(view.browser_place_visible(&custom));
        assert_eq!(
            view.browser.section,
            BrowserSection::Samples,
            "existing Samples picker convention retained"
        );
    });
}
