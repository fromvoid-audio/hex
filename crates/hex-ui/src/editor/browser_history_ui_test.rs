//! History through real toolbar input over bounded private worker roots.
use super::*;

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
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    std::sync::Arc::clone(&directory.1),
                )
            })
        })
        .unwrap();
    editor.read_with(cx, |view, _| {
        assert!(std::sync::Arc::ptr_eq(&directory.1, &view.test_environment));
        assert_eq!(view.browser.directory, directory.0.join("samples"));
        for (actual, name) in [
            (view.browser.test_preferences_path(), "browser.json"),
            (view.recent_projects.test_path(), "recent-projects.json"),
            (view.theme_settings.test_path(), "theme.json"),
            (view.project_directory.test_path(), "project-directory.json"),
        ] {
            assert_eq!(actual, directory.0.join("config").join(name));
        }
    });
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

#[gpui::test]
fn browser_history_hidden_custom_home_music_never_restore_through_back_or_forward(
    cx: &mut TestAppContext,
) {
    for row in ["browser-place-2", "browser-place-1", "browser-place-0"] {
        for forward in [false, true] {
            let (handle, editor, directory) = builtin_history_fixture(cx);
            let visual = &mut VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let custom = directory.0.join("history-custom");
            std::fs::create_dir(&custom).unwrap();
            let wav = custom.join("keep.wav");
            let bytes = mono_pcm_wav(24);
            std::fs::write(&wav, &bytes).unwrap();
            editor.update(visual, |view, cx| {
                view.browser.auto_audition = false;
                view.open_browser_folder(cx);
            });
            assert!(visual.did_prompt_for_paths());
            visual.simulate_paths_selection(Some(vec![custom.clone()]));
            visual.run_until_parked();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let ready = editor.update(visual, |view, cx| {
                    if view.tick(cx) {
                        cx.notify();
                    }
                    view.state.is_some() && view.worker_requests_pending == 0
                });
                if ready {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            let (target, roots, before) = editor.read_with(visual, |view, _| {
                assert_eq!(view.browser.section, BrowserSection::Samples);
                let target = match row {
                    "browser-place-2" => custom.clone(),
                    "browser-place-1" => view.browser.home.clone(),
                    _ => view.browser.music.clone(),
                };
                (
                    target,
                    view.sample_roots(),
                    view.state.as_ref().unwrap().clone(),
                )
            });
            click(visual, row);
            click(visual, "browser-section-devices");
            if forward {
                click(visual, "browser-history-back");
                click(visual, "browser-history-back");
                editor.read_with(visual, |view, _| {
                    assert_eq!(view.browser.section, BrowserSection::Samples)
                });
            }
            // Remove the retained history target through its actual row menu.
            // The current visible Samples/Devices source is deliberately unchanged.
            let bounds = visual.debug_bounds(row).unwrap();
            visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::default());
            let remove = visual.debug_bounds("context-menu-choice-8").unwrap();
            visual.simulate_click(remove.center(), Modifiers::default());
            visual.run_until_parked();
            editor.read_with(visual, |view, _| {
                assert!(!view.browser_place_visible(&target));
                assert!(view.browser.places.contains(&target));
                assert_eq!(view.sample_roots(), roots);
                assert_eq!(
                    view.browser.section,
                    if forward {
                        BrowserSection::Samples
                    } else {
                        BrowserSection::Instruments
                    }
                );
            });
            assert!(visual.debug_bounds(row).is_none());
            click(
                visual,
                if forward {
                    "browser-history-forward"
                } else {
                    "browser-history-back"
                },
            );
            editor.read_with(visual, |view, _| {
                assert_eq!(
                    view.browser.section,
                    BrowserSection::All,
                    "hidden target {row} forward={forward}"
                );
                assert!(!view.browser_place_visible(&target));
                assert!(view.browser.history.can_forward());
                assert!(view.browser.playback_path.is_none());
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
            });
            assert_eq!(std::fs::read(&wav).unwrap(), bytes);
            assert_eq!(query(&editor, visual), "");
            click(visual, "browser-history-forward");
            editor.read_with(visual, |view, _| {
                assert_eq!(view.browser.section, BrowserSection::Instruments);
                assert!(
                    !view.browser.history.can_forward(),
                    "fallback must not add a history step"
                );
            });
            click(visual, "browser-history-back");
            editor.read_with(visual, |view, _| {
                assert_eq!(view.browser.section, BrowserSection::All)
            });
            click(visual, "browser-history-back");
            editor.read_with(visual, |view, _| {
                assert_eq!(view.browser.section, BrowserSection::Samples);
                assert!(!view.browser.history.can_back());
                assert!(view.browser.history.can_forward());
            });
            eprintln!("ACTUAL_HIDDEN_HISTORY {row} forward={forward}");
        }
    }
}

fn builtin_history_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    PreferencesTestDirectory,
) {
    cx.update(crate::editor::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let home = directory.0.join("history-home");
    let music = home.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(music.join("loop.wav"), mono_pcm_wav(24)).unwrap();
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_with_browser_paths(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Err("offline history source test".into()),
                    (
                        Default::default(),
                        Default::default(),
                        Default::default(),
                        Default::default(),
                    ),
                    crate::editor::test_environment::Inputs {
                        existing_environment: Some(std::sync::Arc::clone(&directory.1)),
                        home: Some(home.clone()),
                        defaults: [true; 4],
                        ..Default::default()
                    },
                )
            })
        })
        .unwrap();
    editor.read_with(cx, |view, _| {
        assert!(std::sync::Arc::ptr_eq(&directory.1, &view.test_environment));
        assert_eq!(view.browser.home, home);
        assert_eq!(view.browser.music, music);
        assert_eq!(view.browser.places, vec![music, home]);
        for (actual, name) in [
            (view.browser.test_preferences_path(), "browser.json"),
            (view.recent_projects.test_path(), "recent-projects.json"),
            (view.theme_settings.test_path(), "theme.json"),
            (view.project_directory.test_path(), "project-directory.json"),
        ] {
            assert_eq!(actual, directory.0.join("config").join(name));
        }
    });
    (handle, editor, directory)
}

#[gpui::test]
fn review_history_disabled_forward_preserves_the_live_query_transaction(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(visual, "browser-search");
    visual.simulate_input("l");
    visual.run_until_parked();
    click(visual, "browser-history-forward");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(
                view.browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window),
                "disabled arrow changed focus"
            );
            assert!(!view.browser.history.can_forward());
        })
    });
    visual.simulate_input("o");
    visual.run_until_parked();
    assert_eq!(query(&editor, visual), "lo");
    click(visual, "browser-history-back");
    assert_eq!(
        query(&editor, visual),
        "",
        "disabled click must not split typing into two history edits"
    );
    editor.read_with(visual, |view, _| {
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty())
    });
}

#[gpui::test]
fn review_history_type_delete_collapses_adjacent_location_before_next_edit(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(visual, "browser-search");
    visual.simulate_input("l");
    visual.run_until_parked();
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    assert_eq!(query(&editor, visual), "");
    editor.read_with(visual, |view, _| {
        assert!(
            !view.browser.history.can_back(),
            "type/delete retained a duplicate predecessor"
        )
    });
    visual.simulate_input("x");
    visual.run_until_parked();
    click(visual, "browser-history-back");
    assert_eq!(query(&editor, visual), "");
    editor.read_with(visual, |view, _| {
        assert!(!view.browser.history.can_back());
        assert!(view.browser.history.can_forward());
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn review_history_enabled_arrow_leaves_search_before_following_typing(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(visual, "browser-search");
    visual.simulate_input("l");
    visual.run_until_parked();
    click(visual, "browser-history-back");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.browser.results_focus.is_focused(window));
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
    visual.simulate_input("z");
    visual.run_until_parked();
    assert_eq!(
        query(&editor, visual),
        "",
        "results navigation must not type into the restored query"
    );
    editor.read_with(visual, |view, _| {
        assert!(view.browser.playback_path.is_none());
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn review_history_restores_realized_top_even_when_the_anchor_is_already_visible(
    cx: &mut TestAppContext,
) {
    let (handle, editor, directory) = fixture(cx);
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
        view.browser.auto_audition = false;
        view.browser
            .scroll
            .scroll_to_item_strict(10, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    let anchor = editor.read_with(visual, |view, cx| {
        let scroll = view.browser.scroll.0.borrow();
        assert!(scroll.deferred_scroll_to_item.is_none());
        let top = scroll.base_handle.logical_scroll_top().0;
        assert_eq!(top, 10);
        view.browser_visible_entries(cx)[top].path.clone()
    });
    click(visual, "browser-section-devices");
    click(visual, "browser-history-back");
    editor.read_with(visual, |view, cx| {
        let scroll = view.browser.scroll.0.borrow();
        assert!(
            scroll.deferred_scroll_to_item.is_none(),
            "inspect painted layout, not deferred intention"
        );
        let top = scroll.base_handle.logical_scroll_top().0;
        assert_eq!(view.browser_visible_entries(cx)[top].path, anchor);
        assert_eq!(top, 10);
    });
    assert!(
        visual.debug_bounds("browser-entry-10").is_some(),
        "actual anchor row must be painted"
    );
}

#[gpui::test]
fn review_history_waits_for_the_used_catalog_not_an_unrelated_scan(cx: &mut TestAppContext) {
    for indexed in [false, true] {
        let (handle, editor, _directory) = fixture(cx);
        let visual = &mut VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        if indexed {
            click(visual, "browser-search");
            visual.simulate_input("loop");
            visual.run_until_parked();
        }
        editor.update(visual, |view, _| view.browser.auto_audition = false);
        click(visual, "browser-entry-0");
        let selected = editor.read_with(visual, |view, cx| {
            let Some(crate::editor::browser::BrowserItemKey::File(path)) =
                view.browser_selected_item(cx)
            else {
                panic!("expected actual file-provider selection")
            };
            path
        });
        click(visual, "browser-section-devices");
        editor.update(visual, |view, cx| {
            view.move_browser_history(false, cx);
            // Run the ordinary provider scheduling path before checking readiness.
            view.reconcile_browser_tree_requests(cx);
            view.refresh_sample_catalog(cx);
            assert!(
                view.browser.samples.history_loading(),
                "real catalog work was not started"
            );
            view.restore_browser_history_position(cx);
            assert_eq!(view.browser_selected_item(cx).is_none(), indexed, "indexed={indexed}");
            if !indexed { assert!(matches!(view.browser_selected_item(cx), Some(crate::editor::browser::BrowserItemKey::File(path)) if path == selected)); }
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(visual, |view, cx| {
            assert!(matches!(view.browser_selected_item(cx), Some(crate::editor::browser::BrowserItemKey::File(path)) if path == selected));
            assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
            assert!(view.browser.playback_path.is_none());
        });
    }
}
