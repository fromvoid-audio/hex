//! Real Enter routing into the canonical worker, using bounded private startup roots.
use super::*;
use crate::editor::browser::BrowserItemKey;
use gpui::size;
use hex_session::service::Request;

fn receive_worker(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext, initial: bool) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(cx, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            if initial {
                view.state.is_some()
            } else {
                view.worker_requests_pending == 0
                    && view.pending_requests.is_empty()
                    && view.audio_batch.is_none()
            }
        });
        if done {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "sample insertion worker timed out"
        );
        std::thread::yield_now();
    }
}

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    std::sync::Arc<crate::editor::test_environment::Environment>,
    std::path::PathBuf,
) {
    cx.update(crate::editor::bind_editor_keys);
    let directory = crate::editor::test_environment::Environment::new().unwrap();
    let samples = directory.root().join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    let path = samples.join("Loop.WAV");
    std::fs::write(&path, mono_pcm_wav(24)).unwrap();
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    std::sync::Arc::clone(&directory),
                )
            })
        })
        .unwrap();
    editor.read_with(cx, |view, _| {
        assert!(std::sync::Arc::ptr_eq(&directory, &view.test_environment));
        for (actual, name) in [
            (view.browser.test_preferences_path(), "browser.json"),
            (view.recent_projects.test_path(), "recent-projects.json"),
            (view.theme_settings.test_path(), "theme.json"),
            (view.project_directory.test_path(), "project-directory.json"),
        ] {
            assert_eq!(actual, directory.root().join("config").join(name));
        }
        assert_eq!(view.browser.directory, samples);
        assert_eq!(view.browser.places, vec![samples.clone()]);
        assert_eq!(view.sample_roots(), vec![samples.clone()]);
    });
    receive_worker(&editor, cx, true);
    for name in ["selected", "other"] {
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddTrack { name: name.into() })))
        });
        receive_worker(&editor, cx, false);
    }
    editor.update(cx, |view, cx| {
        view.browser.auto_audition = false;
        view.select_browser_source(BrowserSection::Place(samples), cx);
    });
    (handle, editor, directory, path)
}

#[gpui::test]
fn browser_insert_no_selected_track_refuses_without_new_track(cx: &mut TestAppContext) {
    let (handle, editor, _directory, path) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_click(row.center(), Modifiers::default());
    let before = editor.update(visual, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx)[0].path, path);
        view.selected_track = None;
        view.state.as_ref().unwrap().clone()
    });
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        let after = view.state.as_ref().unwrap();
        assert_eq!(
            after.project.tracks().len(),
            before.project.tracks().len(),
            "Enter without selected track must not create a fallback track"
        );
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.project, before.project);
        assert!(
            view.error
                .as_deref()
                .is_some_and(|error| error.contains("select"))
        );
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
    });
}

fn select_sample(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
) -> hex_project::TrackId {
    visual.run_until_parked();
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_click(row.center(), Modifiers::default());
    editor.update(visual, |view, _| {
        let track = view.state.as_ref().unwrap().project.tracks()[0].id();
        view.selected_track = Some(track);
        track
    })
}

#[gpui::test]
fn project_sample_enter_reuses_asset_and_undoes_placement(cx: &mut TestAppContext) {
    let (handle, editor, _directory, path) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1400.0), px(900.0)));
    let track = select_sample(&editor, visual);
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx, false);
    let (asset, revision, generation) = editor.update(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        let asset = state.project.assets().next().unwrap().id();
        assert_eq!(state.project.assets().count(), 1);
        let revision = state.revision;
        let generation = view.project_generation;
        view.offline_cursor = 100;
        (asset, revision, generation)
    });
    editor.update(cx, |view, _| {
        view.set_auto_warp_imports_for_test(true);
        let captured = view.capture_audio_import_policy(Request::InsertProjectSample {
            project_generation: generation,
            revision,
            asset,
            track,
            start: 100,
            warp: hex_project::AudioImportWarp::Off,
            audition_timing: None,
        });
        assert!(matches!(
            captured,
            Ok(Request::InsertProjectSample {
                warp: hex_project::AudioImportWarp::Beats,
                ..
            })
        ));
        view.set_auto_warp_imports_for_test(false);
    });
    let source = visual.debug_bounds("browser-project-samples").unwrap();
    visual.simulate_click(source.center(), Modifiers::default());
    visual.run_until_parked();
    let search = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(search.center(), Modifiers::default());
    visual.simulate_input("absent");
    visual.run_until_parked();
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::ProjectSamples);
        assert!(view.browser_project_samples(cx).is_empty());
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("Loop");
    visual.run_until_parked();
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::ProjectSamples);
        let rows = view.browser_project_samples(cx);
        assert_eq!(
            rows.len(),
            1,
            "project sample search should retain the asset"
        );
        assert_eq!(rows[0].name, "Loop.WAV");
        assert_eq!(rows[0].asset, asset);
    });
    let key = BrowserItemKey::ProjectSample(generation, revision, asset);
    let row = visual.debug_bounds("browser-project-sample-0").unwrap();
    visual.simulate_click(row.center(), Modifiers::default());
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser_selected_item(cx).as_ref(), Some(&key));
        assert!(view.project_sample_waveform(&key).is_some());
    });
    for selector in ["browser-section-devices", "browser-history-back"] {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
    }
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::ProjectSamples);
        assert_eq!(view.browser_selected_item(cx).as_ref(), Some(&key));
    });
    let row = visual.debug_bounds("browser-project-sample-0").unwrap();
    visual.simulate_click(row.center(), Modifiers::default());
    // Placement owns the retained project asset and must not reopen the source path.
    std::fs::remove_file(&path).unwrap();
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.assets().count(), 1);
        let clips = project.track(track).unwrap().clips();
        assert_eq!(clips.len(), 2);
        assert!(clips.iter().all(|clip| clip.asset() == asset));
        assert_eq!(clips[1].start(), 100);
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    receive_worker(&editor, cx, false);
    let before = editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser_selected_item(cx).as_ref(), Some(&key));
        view.state.as_ref().unwrap().clone()
    });
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        let after = view.state.as_ref().unwrap();
        assert_eq!(after.project, before.project);
        assert_eq!(after.revision, before.revision);
        assert!(
            view.error
                .as_deref()
                .is_some_and(|error| error.contains("project changed"))
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .clips()
                .len(),
            2
        );
        assert_eq!(view.state.as_ref().unwrap().project.assets().count(), 1);
    });
}

#[gpui::test]
fn browser_insert_captures_selected_track_frame_native_path_and_atomic_overwrite(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _directory, path) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let track = select_sample(&editor, visual);
    editor.update(visual, |view, _| view.offline_cursor = 100);
    visual.simulate_keystrokes("enter");
    // No worker response/tick has been consumed. Changing UI selection/cursor must
    // not retarget the captured metadata even if the service has already decoded.
    editor.update(visual, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_track,
            Some((path.clone(), track, 100, true))
        );
        view.selected_track = Some(view.state.as_ref().unwrap().project.tracks()[1].id());
        view.offline_cursor = 999;
    });
    receive_worker(&editor, cx, false);
    let before = editor.update(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project.track(track).unwrap().clips()[0].start(), 100);
        assert!(state.project.tracks()[1].clips().is_empty());
        let clip = &state.project.track(track).unwrap().clips()[0];
        assert_eq!(
            state
                .project
                .assets()
                .find(|asset| asset.id() == clip.asset())
                .unwrap()
                .path(),
            path
        );
        view.selected_track = Some(track);
        view.offline_cursor = 108;
        state.project.clone()
    });
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx, false);
    let after = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        let clips = state.project.track(track).unwrap().clips();
        assert_eq!(clips.len(), 2);
        assert_eq!((clips[0].start(), clips[0].duration()), (100, 8));
        assert_eq!((clips[1].start(), clips[1].duration()), (108, 24));
        assert_eq!(clips[0].id(), before.track(track).unwrap().clips()[0].id());
        assert_eq!(state.project.tracks()[1], before.tracks()[1]);
        state.project.clone()
    });
    // One Undo restores the complete original source clip; Redo needs no file IO.
    std::fs::remove_file(&path).unwrap();
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        let restored = &view.state.as_ref().unwrap().project;
        // Canonical Undo deliberately keeps ID allocation monotonic.
        assert_eq!(restored.tracks(), before.tracks());
        assert_eq!(
            restored.assets().collect::<Vec<_>>(),
            before.assets().collect::<Vec<_>>()
        );
        assert_eq!(restored.tempo_map(), before.tempo_map());
        assert_eq!(restored.meter_map(), before.meter_map());
        assert!(view.state.as_ref().unwrap().can_redo);
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, after)
    });
}

#[gpui::test]
fn browser_insert_quantized_toggle_uses_nearest_canonical_grid_and_snapshots_mode(
    cx: &mut TestAppContext,
) {
    use crate::editor::musical_time::SnapGrid;
    for (quantized, candidate, expected) in [
        (false, 25_000, 25_000),
        (true, 25_000, 24_000),
        (true, 37_000, 48_000),
    ] {
        let (handle, editor, _directory, _path) = fixture(cx);
        let visual = &mut VisualTestContext::from_window(handle, cx);
        let track = select_sample(&editor, visual);
        if quantized {
            let toggle = visual.debug_bounds("browser-insert-grid").unwrap();
            visual.simulate_click(toggle.center(), Modifiers::default());
            select_sample(&editor, visual);
        }
        editor.update(visual, |view, _| {
            assert_eq!(
                view.input_quantization != hex_project::InputQuantization::Off,
                quantized
            );
            view.snap_grid = SnapGrid::Quarter;
            view.offline_cursor = candidate;
        });
        visual.simulate_keystrokes("enter");
        editor.update(visual, |view, _| {
            assert_eq!(
                view.last_enqueued_import_to_track.as_ref().unwrap().2,
                expected
            );
            view.snap_grid = SnapGrid::Off;
            view.input_quantization = if !quantized {
                hex_project::InputQuantization::Adaptive
            } else {
                hex_project::InputQuantization::Off
            };
            view.offline_cursor = 123_456;
        });
        receive_worker(&editor, cx, false);
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .clips()[0]
                    .start(),
                expected
            )
        });
    }
}

#[gpui::test]
fn browser_insert_adaptive_grid_uses_tempo_meter_and_current_viewport(cx: &mut TestAppContext) {
    use crate::editor::musical_time::SnapGrid;
    for zoom in [48_000, 1_500_000] {
        let (handle, editor, _directory, _path) = fixture(cx);
        for command in [
            Command::SetTempoPoint {
                beat: hex_project::BeatTime::ZERO,
                tempo: hex_project::Tempo::new(60_000_000).unwrap(),
            },
            Command::SetMeterPoint {
                beat: hex_project::BeatTime::ZERO,
                meter: hex_project::Meter::new(3, 4).unwrap(),
            },
        ] {
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Edit(command))));
            receive_worker(&editor, cx, false);
        }
        let visual = &mut VisualTestContext::from_window(handle, cx);
        let track = select_sample(&editor, visual);
        let expected = editor.update(visual, |view, _| {
            view.input_quantization = hex_project::InputQuantization::Adaptive;
            view.snap_grid = SnapGrid::Adaptive;
            view.viewport = Some(crate::editor::Viewport {
                start: 0,
                length: zoom,
            });
            view.offline_cursor = 49_000;
            let state = view.state.as_ref().unwrap();
            let lane = view.main_track_lane_bounds.unwrap();
            view.snap_grid
                .snap_frame(
                    &state.project,
                    49_000,
                    view.visible_viewport(state.timeline_frames),
                    lane.width,
                )
                .unwrap()
        });
        assert_ne!(expected, 49_000);
        visual.simulate_keystrokes("enter");
        receive_worker(&editor, cx, false);
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .clips()[0]
                    .start(),
                expected
            )
        });
    }
}

#[gpui::test]
fn browser_insert_refuses_stale_target_busy_session_grid_and_missing_file_atomically(
    cx: &mut TestAppContext,
) {
    use crate::editor::musical_time::SnapGrid;
    for refusal in 0..8 {
        let (handle, editor, _directory, path) = fixture(cx);
        let visual = &mut VisualTestContext::from_window(handle, cx);
        let track = select_sample(&editor, visual);
        if refusal == 0 {
            editor.update(visual, |view, _| {
                assert!(view.enqueue(Request::Edit(Command::RemoveTrack(track))))
            });
            receive_worker(&editor, cx, false);
        }
        let before = editor.update(visual, |view, _| {
            let before = view.state.as_ref().unwrap().clone();
            match refusal {
                0 => view.selected_track = Some(track),
                1 => view.selected_track = None,
                2 => view.opening_project = true,
                3 => view.mode = crate::editor::EditorMode::Session,
                4 => {
                    view.input_quantization = hex_project::InputQuantization::Adaptive;
                    view.snap_grid = SnapGrid::Off;
                }
                5 => {
                    view.input_quantization = hex_project::InputQuantization::Adaptive;
                    view.drag_layout_generation = view.drag_layout_generation.wrapping_add(1);
                }
                6 => std::fs::remove_file(&path).unwrap(),
                7 => view.project_generation = view.project_generation.wrapping_add(1),
                _ => unreachable!(),
            }
            before
        });
        visual.simulate_keystrokes("enter");
        receive_worker(&editor, cx, false);
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project,
                before.project,
                "refusal {refusal}"
            );
            assert_eq!(
                view.state.as_ref().unwrap().revision,
                before.revision,
                "refusal {refusal}"
            );
            assert!(view.error.is_some(), "refusal {refusal}");
        });
    }
}

#[gpui::test]
fn browser_insert_enter_in_query_cannot_enqueue_sample_or_commit_ime(cx: &mut TestAppContext) {
    let (handle, editor, _directory, _path) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    select_sample(&editor, visual);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("Loop");
    visual.run_until_parked();
    let before = editor.read_with(visual, |view, cx| {
        assert!(
            view.browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.kind == hex_library::EntryKind::WaveAudio)
        );
        view.state.as_ref().unwrap().clone()
    });
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        assert!(view.last_enqueued_import_to_track.is_none())
    });
    use gpui::EntityInputHandler;
    visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(Some(0..4), "Loop", Some(1..1), window, cx);
            assert_eq!(input.marked_text_range(window, cx), Some(0..4));
        });
    });
    visual.run_until_parked();
    editor.read_with(visual, |view, cx| {
        assert!(
            view.browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.kind == hex_library::EntryKind::WaveAudio)
        )
    });
    visual.simulate_keystrokes("enter");
    visual.update(|window, cx| {
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            assert_eq!(input.marked_text_range(window, cx), Some(0..4))
        });
    });
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before.project);
        assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
        assert!(view.last_enqueued_import_to_track.is_none());
    });
}

#[gpui::test]
fn browser_insert_all_disabled_densities_refuse_actual_enter(cx: &mut TestAppContext) {
    use crate::editor::musical_time::{AdaptiveGridDensity, SnapGrid};
    let (handle, editor, _directory, _path) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    select_sample(&editor, visual);
    let before = editor.read_with(visual, |view, _| view.state.as_ref().unwrap().clone());
    for grid in
        std::iter::once(SnapGrid::Off).chain(AdaptiveGridDensity::ALL.map(SnapGrid::OffDensity))
    {
        editor.update(visual, |view, _| {
            view.input_quantization = hex_project::InputQuantization::Adaptive;
            view.snap_grid = grid;
            view.offline_cursor = 25_000;
            view.error = None;
        });
        visual.simulate_keystrokes("enter");
        editor.read_with(visual, |view, _| {
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|error| error.contains("enable the arrangement grid")),
                "{grid:?} must visibly refuse"
            );
            assert_eq!(view.worker_requests_pending, 0, "{grid:?}");
            assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
            assert!(view.audio_batch.is_none() && view.last_enqueued_import_to_track.is_none());
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
            assert_eq!(view.snap_grid, grid);
        });
        eprintln!("ACTUAL_DISABLED_GRID_REFUSAL {grid:?}");
    }
}

#[gpui::test]
fn browser_insert_active_densities_snap_and_disabled_unquantized_keeps_frame(
    cx: &mut TestAppContext,
) {
    use crate::editor::musical_time::{AdaptiveGridDensity, SnapGrid};
    let (handle, editor, _directory, path) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let track = select_sample(&editor, visual);
    for density in AdaptiveGridDensity::ALL {
        for quantized in [true, false] {
            let expected = editor.update(visual, |view, _| {
                view.input_quantization = if quantized {
                    hex_project::InputQuantization::Adaptive
                } else {
                    hex_project::InputQuantization::Off
                };
                view.snap_grid = if quantized {
                    SnapGrid::AdaptiveDensity(density)
                } else {
                    SnapGrid::OffDensity(density)
                };
                view.offline_cursor = 25_000;
                assert!(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .track(track)
                        .unwrap()
                        .clips()
                        .is_empty()
                );
                if quantized {
                    let state = view.state.as_ref().unwrap();
                    let lane = view.main_track_lane_bounds.unwrap();
                    let expected = view
                        .snap_grid
                        .snap_frame(
                            &state.project,
                            25_000,
                            view.visible_viewport(state.timeline_frames),
                            lane.width,
                        )
                        .unwrap();
                    assert_ne!(expected, 25_000, "actual nearest-grid control must snap");
                    expected
                } else {
                    25_000
                }
            });
            visual.simulate_keystrokes("enter");
            editor.read_with(visual, |view, _| {
                assert_eq!(
                    view.last_enqueued_import_to_track,
                    Some((path.clone(), track, expected, true))
                );
            });
            receive_worker(&editor, cx, false);
            editor.read_with(cx, |view, _| {
                let clips = view
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .clips();
                assert_eq!(clips.len(), 1);
                assert_eq!((clips[0].start(), clips[0].duration()), (expected, 24));
                assert!(view.error.is_none());
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
            receive_worker(&editor, cx, false);
            editor.read_with(cx, |view, _| {
                assert!(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .track(track)
                        .unwrap()
                        .clips()
                        .is_empty()
                );
                assert_eq!(view.worker_requests_pending, 0);
                assert!(view.pending_requests.is_empty() && view.audio_batch.is_none());
            });
            eprintln!("ACTUAL_DENSITY_INPUT {density:?} quantized={quantized} frame={expected}");
        }
    }
}

#[gpui::test]
fn input_quantization_menu_controls_fixed_browser_insertion_and_freezes_pending_policy(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _directory, _path) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.set_input_quantization(hex_project::InputQuantization::Adaptive);
        cx.notify();
    });
    for width in [880.0, 1_000.0, 1_280.0] {
        visual.simulate_resize(gpui::size(gpui::px(width), gpui::px(540.0)));
        visual.run_until_parked();
        for id in ["input-quantization", "arrange-zoom-back"] {
            let bounds = visual.debug_bounds(id).unwrap();
            assert!(
                f32::from(bounds.origin.x) >= 0.0
                    && f32::from(bounds.origin.x + bounds.size.width) <= width,
                "{id} must remain inside the toolbar at {width}: {bounds:?}"
            );
        }
    }
    editor.update(visual, |view, cx| {
        view.set_input_quantization(hex_project::InputQuantization::Off);
        cx.notify();
    });
    visual.run_until_parked();
    let control = visual.debug_bounds("input-quantization").unwrap();
    visual.simulate_mouse_down(control.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(control.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert_eq!(
            menu.items.iter().map(|item| item.label).collect::<Vec<_>>(),
            ["off", "1/4", "1/8", "1/16", "adaptive"]
        );
        assert_eq!(menu.items.iter().filter(|item| item.checked).count(), 1);
    });
    visual.simulate_keystrokes("down down enter");
    let track = select_sample(&editor, visual);
    editor.update(visual, |view, _| {
        assert_eq!(
            view.input_quantization,
            hex_project::InputQuantization::Eighth
        );
        view.snap_grid = crate::editor::musical_time::SnapGrid::Off;
        view.offline_cursor = 15_600;
    });
    visual.simulate_keystrokes("enter");
    editor.update(visual, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_track.as_ref().unwrap().2,
            12_000
        );
        view.set_input_quantization(hex_project::InputQuantization::Sixteenth);
        view.offline_cursor = 99_999;
    });
    receive_worker(&editor, visual, false);
    editor.read_with(visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .clips()[0]
                .start(),
            12_000
        );
    });
}
