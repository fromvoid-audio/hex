use super::*;

#[gpui::test]
fn browser_place_icon_popup_outside_click_does_not_start_arrangement_gesture(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, _| {
        view.browser.preferences_loading = false;
        view.browser.preferences_writable = true;
    });
    let row = visual.debug_bounds("browser-place-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    assert!(visual.debug_bounds("context-menu-panel").is_some());
    let lane = visual.debug_bounds("main-track-lane").unwrap();
    visual.simulate_mouse_down(lane.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(visual, |view, _| {
        assert!(
            view.interaction.time_selection_gesture.is_none(),
            "outside click through icon picker started arrangement selection"
        );
        assert!(view.interaction.time_range_move_gesture.is_none());
    });
    visual.simulate_mouse_up(lane.center(), MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn browser_place_icons_input_persistence_identity_fallback_and_geometry(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("browser.json");
    let recent = directory.0.join("recent.json");
    let first = directory.0.join("one/same");
    let second = directory.0.join("two/same");
    let unknown = directory.0.join("unknown");
    let mut saved =
        hex_preferences::BrowserPlaces::new([first.clone(), second.clone(), unknown.clone()])
            .unwrap();
    saved.set_icon(&unknown, Some("future-icon")).unwrap();
    hex_preferences::save(&config, &saved).unwrap();
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(
            view.browser_place_icon_path(&first),
            "icons/browser-place/folder.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&unknown),
            "icons/browser-place/folder.svg"
        );
        assert!(
            view.browser
                .places_status
                .as_deref()
                .unwrap()
                .contains("icons unavailable")
        );
    });
    visual.simulate_keystrokes("cmd-f");
    let row = visual.debug_bounds("browser-place-2").unwrap();
    let generation = editor.read_with(visual, |view, _| view.browser.playback_generation);
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    let popup = visual.debug_bounds("context-menu-panel").unwrap();
    visual.update(|window, _| {
        assert!(popup.left() >= px(0.0) && popup.top() >= px(0.0));
        assert!(popup.right() <= window.bounds().size.width);
        assert!(popup.bottom() <= window.bounds().size.height);
    });
    visual.simulate_keystrokes("space cmd-shift-t right right enter");
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.context_menu.is_none());
            assert!(
                view.browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
            assert_eq!(
                view.browser_place_icon_path(&first),
                "icons/browser-place/samples.svg"
            );
            assert_eq!(view.browser.section, BrowserSection::All);
            assert_eq!(view.browser.playback_generation, generation);
            assert!(view.pending_requests.is_empty());
            assert!(view.audio_pub.pending_audio_commands.is_empty());
        })
    });
    // A pointer choice for the same glyph is allowed on another, same-named Place.
    let row = visual.debug_bounds("browser-place-3").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    let sample = visual.debug_bounds("context-menu-choice-2").unwrap();
    visual.simulate_click(sample.center(), Modifiers::default());
    visual.run_until_parked();
    let saved = hex_preferences::load(&config).unwrap();
    assert_eq!(saved.icon(&first), Some("samples"));
    assert_eq!(saved.icon(&second), Some("samples"));
    assert_eq!(saved.icon(&unknown), Some("future-icon"));
    // Reordering rows never retargets assignment; unknown IDs remain recoverable.
    editor.update(visual, |view, cx| {
        view.browser.places.swap(2, 4);
        cx.notify();
    });
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
    let target = divider.center() + point(px(23.0) - sidebar.size.width, px(0.0));
    visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let control = visual.debug_bounds("browser-control-⚙").unwrap();
    for (index, expected) in [
        (2, "icons/browser-place/folder.svg"),
        (3, "icons/browser-place/samples.svg"),
        (4, "icons/browser-place/samples.svg"),
    ] {
        let row = visual
            .debug_bounds(Box::leak(format!("browser-place-{index}").into_boxed_str()))
            .unwrap();
        let glyph = visual
            .debug_bounds(Box::leak(
                format!("browser-picker-glyph-place-{index}").into_boxed_str(),
            ))
            .unwrap();
        assert_eq!(glyph.center().x, control.center().x);
        assert!(glyph.left() >= row.left() && glyph.right() <= row.right());
        editor.read_with(visual, |view, _| {
            assert_eq!(
                view.browser_place_icon_path(&view.browser.places[index]),
                expected
            )
        });
    }
    // Escape dismisses without choosing; normal left-click still selects this Place.
    let row = visual.debug_bounds("browser-place-4").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("down escape");
    assert_eq!(
        hex_preferences::load(&config).unwrap().icon(&first),
        Some("samples")
    );
    visual.simulate_click(row.center(), Modifiers::default());
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Place(first.clone()))
    });
    // Recreate the actual editor/preferences load path, using only disposable test files.
    let (reopened_handle, reopened) =
        place_icon_preferences_window(cx, &directory, &config, &recent);
    let reopened_visual = &mut VisualTestContext::from_window(reopened_handle, cx);
    reopened_visual.run_until_parked();
    reopened.read_with(reopened_visual, |view, _| {
        assert_eq!(
            view.browser_place_icon_path(&first),
            "icons/browser-place/samples.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&second),
            "icons/browser-place/samples.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&unknown),
            "icons/browser-place/folder.svg"
        );
        assert_eq!(
            view.browser.place_icons.get(&unknown).map(String::as_str),
            Some("future-icon")
        );
    });
}

#[gpui::test]
fn browser_place_icons_coalesced_save_errors_and_stale_menu_are_bounded(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("browser.json");
    let path = directory.0.join("added");
    let saved = hex_preferences::BrowserPlaces::new([path.clone()]).unwrap();
    hex_preferences::save(&config, &saved).unwrap();
    let (handle, editor) =
        place_icon_preferences_window(cx, &directory, &config, &directory.0.join("recent.json"));
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        for id in ["samples", "waveform", "projects"] {
            view.set_browser_place_icon(path.clone(), Some(id), cx);
        }
    });
    visual.run_until_parked();
    assert_eq!(
        hex_preferences::load(&config).unwrap().icon(&path),
        Some("projects")
    );
    let valid_bytes = std::fs::read(&config).unwrap();
    // A changed corrupt file cannot be overwritten by a later icon choice.
    let corrupt = format!(
        "{{\"version\":1,\"additional_places\":[],\"{}\":0}}",
        "bad".repeat(200)
    )
    .into_bytes();
    std::fs::write(&config, &corrupt).unwrap();
    let row = visual.debug_bounds("browser-place-2").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("right enter");
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert!(
            view.browser
                .places_status
                .as_deref()
                .unwrap()
                .contains("could not save")
        )
    });
    assert_eq!(std::fs::read(&config).unwrap(), corrupt);
    // Explicitly choosing the same unsaved icon retries after the external failure is repaired.
    std::fs::write(&config, &valid_bytes).unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        hex_preferences::load(&config).unwrap().icon(&path),
        Some("collection")
    );
    // Removed identity invalidates the existing menu before a queued choice can write.
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    editor.update(visual, |view, cx| {
        view.browser.places.retain(|place| place != &path);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(visual, |view, _| assert!(view.context_menu.is_none()));
    std::fs::write(&config, &corrupt).unwrap();
    let (other_handle, other) = place_icon_preferences_window(
        cx,
        &directory,
        &config,
        &directory.0.join("other-recent.json"),
    );
    let other_visual = &mut VisualTestContext::from_window(other_handle, cx);
    other_visual.run_until_parked();
    let status_row = other_visual.debug_bounds("browser-places-status").unwrap();
    other.update(other_visual, |view, cx| {
        view.browser.places_status = Some("could not load browser places".into());
        cx.notify();
    });
    let short_status = other_visual.debug_bounds("browser-places-status").unwrap();
    assert_eq!(
        status_row.size.height, short_status.size.height,
        "long errors must retain the actual theme's single-line status height"
    );
    // Reload the actual bounded load error for the remaining state assertions.
    other.update(other_visual, |view, cx| {
        view.browser_places_loaded(hex_preferences::load(&config), cx)
    });
    other.read_with(other_visual, |view, _| {
        assert!(
            view.browser
                .places_status
                .as_deref()
                .unwrap()
                .chars()
                .count()
                <= 200
        );
        assert!(!view.browser.preferences_writable);
        assert!(
            view.browser
                .places_status
                .as_deref()
                .unwrap()
                .contains("could not load")
        );
    });
    other_visual.update(|window, cx| {
        other.update(cx, |view, cx| {
            let custom = directory.0.join("unsaved");
            view.browser.places.push(custom.clone());
            view.open_browser_place_icons(custom, point(px(50.0), px(50.0)), window, cx);
            assert!(view.context_menu.is_none());
        })
    });
    assert_eq!(std::fs::read(&config).unwrap(), corrupt);
}

#[gpui::test]
fn browser_place_icons_catalog_uses_same_bounds_expanded_and_collapsed(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser.preferences_path = Some(directory.0.join("icon-preferences.json"));
        view.browser.preferences_loading = false;
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            for path in [view.browser.home.clone(), view.browser.music.clone()] {
                view.browser
                    .place_icons
                    .insert(path.clone(), "samples".into());
                assert_eq!(
                    view.browser_place_icon_path(&path),
                    "icons/browser-place/folder.svg"
                );
                view.open_browser_place_icons(path, point(px(0.0), px(0.0)), window, cx);
                assert!(
                    view.context_menu.is_none(),
                    "these built-ins are absent from this custom-only fixture"
                );
            }
            view.browser.preferences_loading = true;
            view.open_browser_place_icons(directory.0.clone(), point(px(0.0), px(0.0)), window, cx);
            assert!(
                view.context_menu.is_none(),
                "no icon choice may race startup load"
            );
            view.browser.preferences_loading = false;
        })
    });
    for width in [124.0, 23.0] {
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        for (index, (id, glyph)) in [
            (None, "icons/browser-place/folder.svg"),
            (Some("samples"), "icons/browser-place/samples.svg"),
            (Some("devices"), "icons/browser-place/devices.svg"),
            (Some("projects"), "icons/browser-place/projects.svg"),
            (Some("collection"), "icons/browser-place/collection.svg"),
            (Some("waveform"), "icons/browser-place/waveform.svg"),
            (Some("search"), "icons/browser-place/search.svg"),
        ]
        .into_iter()
        .enumerate()
        {
            let row = visual.debug_bounds("browser-place-0").unwrap();
            visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
            let choice_index = if index == 0 { 0 } else { index + 1 };
            let cell = visual
                .debug_bounds(Box::leak(
                    format!("context-menu-choice-{choice_index}").into_boxed_str(),
                ))
                .unwrap();
            visual.simulate_click(cell.center(), Modifiers::default());
            visual.run_until_parked();
            let bounds = visual.debug_bounds("browser-picker-glyph-place-0").unwrap();
            let control = visual.debug_bounds("browser-control-⚙").unwrap();
            assert_eq!(bounds.center().x, control.center().x);
            assert!(bounds.left() >= row.left() && bounds.right() <= row.right());
            assert!(f32::from(bounds.center().y - row.center().y).abs() <= 0.5);
            editor.read_with(visual, |view, _| {
                assert_eq!(view.browser_place_icon_path(&directory.0), glyph)
            });
            assert_eq!(
                hex_preferences::load(&directory.0.join("icon-preferences.json"))
                    .unwrap()
                    .icon(&directory.0),
                id
            );
        }
    }
}

#[gpui::test]
fn browser_place_icons_preference_fixture_bounds_startup_before_workers(cx: &mut TestAppContext) {
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("missing-browser.json");
    let recent = directory.0.join("recent.json");
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    // Check before parking/running any newly scheduled filesystem work.
    editor.read_with(cx, |view, _| {
        for path in [
            &view.browser.home,
            &view.browser.music,
            &view.browser.directory,
        ] {
            assert!(
                path.starts_with(&directory.0),
                "startup browser root escaped private fixture: {path:?}"
            );
        }
        assert_eq!(
            view.browser.places,
            vec![view.browser.music.clone(), view.browser.home.clone()]
        );
        assert_eq!(
            view.browser.preferences_path.as_deref(),
            Some(config.as_path())
        );
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert!(!view.browser.loading);
        assert!(!view.browser.preferences_loading);
        assert_eq!(view.browser.samples.catalog.entries().len(), 1);
        assert!(
            view.sample_roots()
                .iter()
                .all(|path| path.starts_with(&directory.0))
        );
        assert_eq!(view.browser.listing.as_ref().unwrap().entries.len(), 1);
        assert_eq!(
            view.browser.listing.as_ref().unwrap().entries[0].path,
            view.browser.music.join("fixture.wav")
        );
    });
    assert!(config.with_extension("samples-v1.db").is_file());
    assert!(
        !config.exists(),
        "missing explicit preferences must not be manufactured"
    );
}

#[gpui::test]
fn browser_place_icon_grid_uses_clicked_row_reset_and_reopens(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let config = directory.0.join("grid-browser.json");
    let recent = directory.0.join("recent.json");
    let first = directory.0.join("one/same");
    let second = directory.0.join("two/same");
    hex_preferences::save(
        &config,
        &hex_preferences::BrowserPlaces::new([first.clone(), second.clone()]).unwrap(),
    )
    .unwrap();
    let (handle, editor) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-f");
    for (row_index, choice, expected, width) in [
        (2, 2, Some("samples"), 124.0),
        (3, 6, Some("waveform"), 23.0),
        (2, 0, None, 23.0),
    ] {
        let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
        let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
        let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
        visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        let (source, generation) = editor.read_with(visual, |view, _| {
            (
                view.browser.section.clone(),
                view.browser.playback_generation,
            )
        });
        let row = visual
            .debug_bounds(Box::leak(
                format!("browser-place-{row_index}").into_boxed_str(),
            ))
            .unwrap();
        // Two positions inside the same row must produce the same row anchor.
        for fraction in [0.2, 0.8] {
            let hit = point(row.left() + row.size.width * fraction, row.center().y);
            visual.simulate_mouse_down(hit, MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(hit, MouseButton::Right, Modifiers::default());
            let grid = visual
                .debug_bounds("context-menu-icon-grid")
                .expect("actual row context menu must contain a compact icon grid");
            let first_cell = visual.debug_bounds("context-menu-choice-1").unwrap();
            let second_cell = visual.debug_bounds("context-menu-choice-2").unwrap();
            let next_row = visual.debug_bounds("context-menu-choice-5").unwrap();
            assert_eq!(first_cell.top(), second_cell.top());
            assert_eq!(first_cell.left(), next_row.left());
            assert!(next_row.top() > first_cell.top());
            assert!(grid.size.width <= px(120.0));
            let panel = visual.debug_bounds("context-menu-panel").unwrap();
            assert_eq!(panel.left(), row.left().max(px(4.0)));
            assert_eq!(panel.top(), row.bottom());
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_keystrokes("cmd-f");
        visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
        let current = editor.read_with(visual, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .position(|item| item.checked)
                .unwrap()
        });
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("context-menu-current-{current}").into_boxed_str()
                ))
                .is_some()
        );
        let cell = visual
            .debug_bounds(Box::leak(
                format!("context-menu-choice-{choice}").into_boxed_str(),
            ))
            .unwrap();
        if row_index == 3 {
            visual.simulate_keystrokes("right right down enter");
        } else {
            visual.simulate_click(cell.center(), Modifiers::default());
        }
        visual.run_until_parked();
        let path = if row_index == 2 { &first } else { &second };
        assert_eq!(hex_preferences::load(&config).unwrap().icon(path), expected);
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                assert!(view.context_menu.is_none());
                assert_eq!(view.browser.section, source);
                assert_eq!(view.browser.playback_generation, generation);
                assert!(
                    view.pending_requests.is_empty()
                        && view.audio_pub.pending_audio_commands.is_empty()
                );
                assert!(
                    view.browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window),
                    "restore actual previous search focus, do not invent entry"
                );
            })
        });
    }
    // A menu entered from non-text context restores that owner, not search.
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    let row = visual.debug_bounds("browser-place-3").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("left up escape");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.focus.is_focused(window));
            assert!(
                !view
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
        })
    });
    assert_eq!(
        hex_preferences::load(&config).unwrap().icon(&second),
        Some("waveform")
    );
    let (handle, reopened) = place_icon_preferences_window(cx, &directory, &config, &recent);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    reopened.read_with(visual, |view, _| {
        assert_eq!(
            view.browser_place_icon_path(&first),
            "icons/browser-place/folder.svg"
        );
        assert_eq!(
            view.browser_place_icon_path(&second),
            "icons/browser-place/waveform.svg"
        );
    });
}

#[gpui::test]
fn browser_picker_compact_gear_closes_without_expanding_then_opens_explicitly(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let closed_top = visual.debug_bounds("browser-place-0").unwrap().top();
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    let open_top = visual.debug_bounds("browser-place-0").unwrap().top();
    assert!(
        open_top > closed_top,
        "actual settings rows must be present"
    );
    let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let target = divider.center() + point(px(23.0) - sidebar.size.width, px(0.0));
    visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-place-0").unwrap().top(),
        open_top
    );
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    assert_eq!(
        visual.debug_bounds("browser-sidebar").unwrap().size.width,
        px(23.0),
        "closing compact settings must not widen the picker"
    );
    assert_eq!(
        visual.debug_bounds("browser-place-0").unwrap().top(),
        closed_top,
        "gear must remove the actual open settings rows"
    );
    // The existing deliberate compact-open behavior still expands usable settings.
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    assert_eq!(
        visual.debug_bounds("browser-sidebar").unwrap().size.width,
        px(124.0)
    );
    assert_eq!(
        visual.debug_bounds("browser-place-0").unwrap().top(),
        open_top
    );
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    assert_eq!(
        visual.debug_bounds("browser-place-0").unwrap().top(),
        closed_top
    );
    editor.read_with(visual, |view, _| {
        assert!(view.pending_requests.is_empty());
        assert!(view.audio_pub.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn browser_arrows_source_sample_right(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, true, false, false);
}

#[gpui::test]
fn browser_arrows_escape_sample_right(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, true, true, false);
}

#[gpui::test]
fn browser_arrows_source_folder_right(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, false, false, false);
}

#[gpui::test]
fn browser_arrows_escape_folder_right(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, false, true, false);
}

#[gpui::test]
fn browser_arrows_escape_folder_left(cx: &mut TestAppContext) {
    unhighlighted_arrow_action(cx, false, true, true);
}

#[gpui::test]
fn browser_arrows_selected_controls(cx: &mut TestAppContext) {
    for sample in [false, true] {
        let (handle, editor, _directory, samples) = arrow_row_fixture(cx, sample);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let category = if sample {
            "browser-place-0"
        } else {
            "browser-section-projects"
        };
        let category = visual.debug_bounds(category).unwrap().center();
        visual.simulate_click(category, Modifiers::default());
        visual.run_until_parked();
        let row = visual.debug_bounds("browser-entry-0").unwrap().center();
        visual.simulate_click(row, Modifiers::default());
        let before = editor.read_with(&visual, |v, _| {
            assert!(v.browser.results_focused);
            v.browser.playback_generation
        });
        visual.simulate_keystrokes("right");
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            if sample {
                assert!(v.browser.playback_generation > before);
            } else {
                assert_eq!(
                    v.browser.tree.disclosure(&samples.join("a")),
                    super::super::tree::Disclosure::Expanded
                );
            }
        });
        if !sample {
            visual.simulate_keystrokes("left");
            editor.read_with(&visual, |v, _| {
                assert_ne!(
                    v.browser.tree.disclosure(&samples.join("a")),
                    super::super::tree::Disclosure::Expanded
                )
            });
        }
    }
}

#[gpui::test]
fn browser_arrows_up_down_enter_results_and_command_f_keeps_text_input(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    let (handle, editor, _directory, _samples) = arrow_row_fixture(cx, true);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let place = visual.debug_bounds("browser-place-0").unwrap().center();
    visual.simulate_click(place, Modifiers::default());
    visual.run_until_parked();
    for key in ["down", "up"] {
        editor.read_with(&visual, |v, _| assert!(!v.browser.results_focused));
        visual.simulate_keystrokes(key);
        let before = editor.read_with(&visual, |v, _| {
            assert!(v.browser.results_focused);
            assert_eq!(v.browser.selected_index, 0);
            v.browser.playback_generation
        });
        visual.simulate_keystrokes("right");
        editor.read_with(&visual, |v, _| {
            assert!(v.browser.playback_generation > before)
        });
        visual.simulate_keystrokes("escape");
    }
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("loop");
    visual.simulate_keystrokes("cmd-f");
    let before = editor.read_with(&visual, |v, _| v.browser.playback_generation);
    visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                0..4
            )
        });
    });
    visual.simulate_keystrokes("right");
    visual.simulate_input("x");
    editor.read_with(&visual, |v, cx| {
        assert_eq!(v.browser.search.read(cx).text(), "loopx");
        assert_eq!(v.browser.playback_generation, before);
        assert!(v.pending_requests.is_empty() && v.audio_pub.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn sampler_menu_captures_stale_target_and_correlates_cancel_without_duplicate_load(
    cx: &mut TestAppContext,
) {
    use super::super::super::context_menu::EditorContextAction;
    let (handle, editor, tracks, _directory, path) =
        browser_drag_fixture_with_tracks(cx, &["target", "other"]);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            let state = view.state.as_ref().unwrap();
            let mut model = Editor::new((*state.project).clone(), 4).unwrap();
            model
                .execute(Command::InsertInstrument {
                    track: tracks[0],
                    state: hex_engine::default_native_device_state_for_class(
                        hex_engine::NativeDeviceClass::Instrument,
                        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler"),
                    )
                    .unwrap(),
                })
                .unwrap();
            let Created::Clip(clip) = model
                .execute(Command::PlaceMidiClip {
                    track: tracks[0],
                    start: hex_project::BeatTime::ZERO,
                    length: hex_project::BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            model
                .execute(Command::AddMidiNote {
                    clip,
                    start: hex_project::BeatTime::ZERO,
                    duration: hex_project::BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: Arc::new(model.project().clone()),
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: Default::default(),
                midi_preview: Arc::default(),
                gain_preview: state.gain_preview.clone(),
                timeline_frames: state.timeline_frames,
                can_undo: state.can_undo,
                can_redo: state.can_redo,
                revision: state.revision,
                unsaved: state.unsaved,
                document_path: state.document_path.clone(),
            }));
            view.selected_track = Some(tracks[0]);
            view.open_first_sampler_menu(path, point(px(20.0), px(100.0)), window, cx);
            let EditorContextAction::LoadFirstSampler(target) =
                view.context_menu.as_ref().unwrap().items[0].action.clone()
            else {
                panic!("sampler menu action")
            };
            assert_eq!(
                view.context_menu.as_ref().unwrap().items[0].label,
                "load into sampler"
            );
            assert!(
                matches!(target.slot, hex_session::service::SamplerLoadSlot::Existing { ref expected_state, .. }
                    if hex_engine::is_empty_sampler_state_on_control(expected_state))
            );
            assert!(
                !view.first_sampler_current(),
                "an empty sampler has no loaded audio requirement"
            );
            let generation = view.project_generation;
            let pending = view.worker_requests_pending;
            view.project_generation += 1;
            view.load_first_sampler(target.clone(), cx);
            assert_eq!(
                view.worker_requests_pending, pending,
                "stale menu must not enqueue"
            );
            assert!(!view.first_sampler_pending_matches(1));
            view.project_generation = generation;
            view.load_first_sampler(target.clone(), cx);
            assert!(view.first_sampler_pending_matches(1));
            let pending = view.worker_requests_pending;
            view.selected_track = Some(tracks[1]);
            view.load_first_sampler(target, cx);
            assert_eq!(
                view.worker_requests_pending, pending,
                "double action must not enqueue"
            );
            view.cancel_first_sampler_load(Some(1));
            view.first_sampler_load_complete(
                Some(hex_session::service::SamplerLoadCompletion {
                    request_id: 2,
                    project_generation: generation,
                    track: Some(tracks[0]),
                }),
                false,
            );
            assert!(
                view.first_sampler_pending_matches(1),
                "unrelated completion cannot clear intent"
            );
            view.first_sampler_load_complete(
                Some(hex_session::service::SamplerLoadCompletion {
                    request_id: 1,
                    project_generation: generation,
                    track: Some(tracks[0]),
                }),
                false,
            );
            assert!(!view.first_sampler_pending_matches(1));
            assert_eq!(view.status, "sampler load cancelled");
            assert_eq!(view.selected_track, Some(tracks[1]));
        });
    })
    .unwrap();
}

#[gpui::test]
fn browser_file_result_vectors_paint_in_light_and_dark_themes(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.browser.open = true;
        view.browser
            .listing
            .as_mut()
            .unwrap()
            .entries
            .push(LibraryEntry {
                name: "project.hex".into(),
                path: view.browser.directory.join("project.hex"),
                kind: EntryKind::Project,
            });
        view.browser.samples.invalidate_listing();
        view.browser.tree.source_changed();
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = [
        visual.debug_bounds("browser-file-icon-0").unwrap(),
        visual.debug_bounds("browser-file-icon-1").unwrap(),
    ];
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
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        let expected = gpui::Hsla::from(gpui::rgb(crate::theme::color(
            crate::theme::PaletteRole::Muted,
        )));
        for (index, selector) in ["browser-file-icon-0", "browser-file-icon-1"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(visual.debug_bounds(selector), Some(bounds[index]));
            assert!(
                visual
                    .rendered_monochrome_colors(selector)
                    .contains(&expected)
            );
        }
    }
}

#[gpui::test]
fn collections_stay_below_scrollable_sources_in_both_picker_sizes(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    visual.run_until_parked();
    let pinned = visual.debug_bounds("browser-collections").unwrap();
    let gear = visual.debug_bounds("browser-control-⚙").unwrap().center();
    visual.simulate_click(gear, Modifiers::default());
    assert_eq!(visual.debug_bounds("browser-collections").unwrap(), pinned);
    for window_size in [(880.0, 540.0), (1440.0, 800.0)] {
        visual.simulate_resize(gpui::size(px(window_size.0), px(window_size.1)));
        for width in [23.0, 124.0] {
            let sources = visual.debug_bounds("browser-sidebar-sources").unwrap();
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: sources.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(500.0))),
                ..Default::default()
            });
            let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
            let divider = visual
                .debug_bounds("browser-sidebar-resize")
                .unwrap()
                .center();
            let target = divider + point(px(width) - sidebar.size.width, px(0.0));
            visual.simulate_mouse_down(divider, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
            let sources = visual.debug_bounds("browser-sidebar-sources").unwrap();
            let collections = visual.debug_bounds("browser-collections").unwrap();
            let library = visual
                .debug_bounds("browser-picker-heading-library")
                .unwrap();
            assert_eq!(sidebar.size.width, px(width));
            assert_eq!(library.top(), sidebar.top());
            assert_eq!(sources.bottom(), collections.top());
            assert_eq!(collections.bottom(), sidebar.bottom());
            let first = visual.debug_bounds("browser-collection-0").unwrap();
            assert!(first.top() >= collections.top() && first.bottom() <= collections.bottom());
            visual.simulate_click(first.center(), Modifiers::default());
            visual.update(|window, cx| {
                editor.read_with(cx, |view, _| {
                    assert_eq!(view.browser.section, BrowserSection::Collection(0));
                    assert!(view.browser.results_focus.is_focused(window));
                })
            });
            let all = visual.debug_bounds("browser-section-all").unwrap().center();
            visual.simulate_click(all, Modifiers::default());
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: sources.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-500.0))),
                ..Default::default()
            });
            assert_eq!(
                visual.debug_bounds("browser-collections").unwrap(),
                collections
            );
            if window_size.1 == 540.0 {
                assert!(
                    visual
                        .debug_bounds("browser-picker-heading-library")
                        .unwrap()
                        .top()
                        < library.top(),
                    "the upper sources must actually scroll while collections stay fixed"
                );
            }
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, before.project);
                assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
                assert!(view.pending_requests.is_empty());
            });
        }
    }
}
