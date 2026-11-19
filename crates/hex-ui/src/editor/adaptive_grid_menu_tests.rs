use super::super::super::clip_drag_ui_test::audio_project;
use super::super::super::musical_time::{SnapGrid, grid_lines};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, px};
use std::sync::Arc;

// One fixture root is leased by the view and its real catalog jobs.
struct GridTestDirectory(
    std::path::PathBuf,
    Arc<super::super::super::test_environment::Environment>,
);

impl GridTestDirectory {
    fn with_environment(
        environment: Arc<super::super::super::test_environment::Environment>,
    ) -> Self {
        let root = environment.root().to_owned();
        std::fs::create_dir_all(root.join("samples")).unwrap();
        Self(root, environment)
    }
}

fn editor_window(
    cx: &mut TestAppContext,
    project: hex_project::Project,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    GridTestDirectory,
) {
    editor_window_with_environment(
        cx,
        project,
        midi_preview,
        super::super::super::test_environment::Environment::new().unwrap(),
    )
}

fn editor_window_with_environment(
    cx: &mut TestAppContext,
    project: hex_project::Project,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    environment: Arc<super::super::super::test_environment::Environment>,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    GridTestDirectory,
) {
    use gpui::AppContext;
    let directory = GridTestDirectory::with_environment(environment);
    let bootstrap = cx.add_window(|_, _| gpui::Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&directory.1),
                )
            });
            super::super::super::install_close_guard(window, cx, editor.downgrade());
            editor.update(cx, |view, cx| {
                assert!(Arc::ptr_eq(&directory.1, &view.test_environment));
                assert_eq!(view.browser.directory, directory.0.join("samples"));
                assert_eq!(view.browser.places, vec![directory.0.join("samples")]);
                view.state = Some(Arc::new(hex_session::service::ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview,
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, directory)
}

#[gpui::test]
fn adaptive_grid_menu_right_click_keyboard_choice_and_cancel_preserve_project_and_focus(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let (project, track, _, _) = audio_project();
    let original = project.clone();
    let (handle, editor, _directory) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let bounds = visual.debug_bounds(lane).unwrap();
    let position = gpui::point(bounds.right() - px(20.0), bounds.origin.y + px(24.0));
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let fine_index = editor.read_with(cx, |view, _| {
        let menu = view
            .context_menu
            .as_ref()
            .expect("actual arranger right click opens menu");
        assert_eq!(menu.items[0].label, "insert track");
        let choices = menu
            .items
            .iter()
            .filter(|item| matches!(item.action, EditorContextAction::AdaptiveGrid { .. }))
            .collect::<Vec<_>>();
        assert_eq!(choices.len(), 3);
        assert_eq!(
            choices.iter().map(|item| item.checked).collect::<Vec<_>>(),
            [false, true, false]
        );
        menu.items
            .iter()
            .position(|item| {
                matches!(
                    item.action,
                    EditorContextAction::AdaptiveGrid {
                        density: AdaptiveGridDensity::Fine,
                        ..
                    }
                )
            })
            .unwrap()
    });
    for _ in 0..fine_index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    editor.read_with(cx, |view, app| {
        assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Fine);
        assert!(view.context_menu.is_none());
        assert_eq!(*view.state.as_ref().unwrap().project, original);
        assert!(view.last_enqueued_edit.is_none());
        let _ = app;
    });
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| assert!(view.focus.is_focused(window)))
    });
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("down down escape");
    assert_eq!(
        editor.read_with(cx, |view, _| view.snap_grid.density()),
        AdaptiveGridDensity::Fine
    );
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    let index = editor.read_with(cx, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        let checked = menu
            .items
            .iter()
            .filter(|item| {
                matches!(item.action, EditorContextAction::AdaptiveGrid { .. }) && item.checked
            })
            .collect::<Vec<_>>();
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].label, AdaptiveGridDensity::Fine.label());
        menu.items
            .iter()
            .position(|item| {
                matches!(
                    item.action,
                    EditorContextAction::AdaptiveGrid {
                        density: AdaptiveGridDensity::Coarse,
                        ..
                    }
                )
            })
            .unwrap()
    });
    let panel = visual.debug_bounds("context-menu-panel").unwrap();
    visual.simulate_click(
        gpui::point(
            panel.origin.x + px(16.0),
            panel.origin.y + px(2.0 + 24.0 * index as f32 + 12.0),
        ),
        Modifiers::default(),
    );
    assert_eq!(
        editor.read_with(cx, |view, _| view.snap_grid.density()),
        AdaptiveGridDensity::Coarse
    );
}

#[gpui::test]
fn arrangement_waveform_scale_toggle_is_view_local_and_checked(cx: &mut TestAppContext) {
    use super::super::super::waveform::WaveformAmplitudeScale;

    let (project, _, _, _) = audio_project();
    let (handle, editor, _directory) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let position = gpui::point(px(400.0), px(100.0));
            let generation = view.project_generation;
            assert_eq!(
                view.waveform_geometry_cache.amplitude_scale(),
                WaveformAmplitudeScale::Linear
            );
            view.open_arranger_grid_context_menu(position, window, cx);
            let choice = view
                .context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .find(|item| matches!(item.action, EditorContextAction::WaveformDecibels { .. }))
                .unwrap();
            assert!(!choice.checked);
            view.select_context_item(
                EditorContextAction::WaveformDecibels {
                    generation,
                    enabled: true,
                },
                window,
                cx,
            );
            assert_eq!(
                view.waveform_geometry_cache.amplitude_scale(),
                WaveformAmplitudeScale::Decibel
            );
            assert_eq!(
                view.clip_waveform_geometry_cache.amplitude_scale(),
                WaveformAmplitudeScale::Linear
            );
            view.open_arranger_grid_context_menu(position, window, cx);
            let choice = view
                .context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .find(|item| matches!(item.action, EditorContextAction::WaveformDecibels { .. }))
                .unwrap();
            assert!(choice.checked);
            view.select_context_item(
                EditorContextAction::WaveformDecibels {
                    generation,
                    enabled: false,
                },
                window,
                cx,
            );
            assert_eq!(
                view.waveform_geometry_cache.amplitude_scale(),
                WaveformAmplitudeScale::Linear
            );
        });
    });
}

#[gpui::test]
fn adaptive_grid_menu_disabled_choice_and_stale_project_refusal_are_ui_only(
    cx: &mut TestAppContext,
) {
    let (project, _, _, _) = audio_project();
    let (handle, editor, _directory) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.snap_grid = SnapGrid::Off;
            view.open_arranger_grid_context_menu(gpui::point(px(400.0), px(100.0)), window, cx);
            let generation = view.project_generation;
            view.select_context_item(
                EditorContextAction::AdaptiveGrid {
                    generation,
                    density: AdaptiveGridDensity::Coarse,
                },
                window,
                cx,
            );
            assert!(view.snap_grid.is_off());
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
            view.toggle_snap_grid();
            assert!(!view.snap_grid.is_off());
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
            view.select_context_item(
                EditorContextAction::AdaptiveGrid {
                    generation: generation.wrapping_sub(1),
                    density: AdaptiveGridDensity::Fine,
                },
                window,
                cx,
            );
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
            assert!(view.last_enqueued_edit.is_none());
        })
    });
}

#[gpui::test]
fn adaptive_grid_menu_choice_renders_the_same_landmark_used_by_keyboard_edit(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    for density in AdaptiveGridDensity::ALL {
        let (project, track, _, clip) = audio_project();
        let (handle, editor, _directory) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.selected_track = Some(track);
                view.selected_clip = Some(clip);
                view.focus.focus(window);
                view.open_arranger_grid_context_menu(gpui::point(px(400.0), px(100.0)), window, cx);
                view.select_context_item(
                    EditorContextAction::AdaptiveGrid {
                        generation: view.project_generation,
                        density,
                    },
                    window,
                    cx,
                );
            })
        });
        visual.run_until_parked();
        let next = editor.read_with(cx, |view, _| {
            let (grid, viewport, width) = view.nudge_grid_context(false).unwrap();
            let project = &view.state.as_ref().unwrap().project;
            let next = grid
                .adjacent_frame(project, 0, viewport, width, true)
                .unwrap();
            assert!(
                grid_lines(project, viewport, width, grid)
                    .iter()
                    .any(|line| line.frame == next)
            );
            next
        });
        let selector = Box::leak(format!("arrangement-grid-line-{next}").into_boxed_str());
        assert!(
            visual.debug_bounds(selector).is_some(),
            "selected subdivision is actually rendered"
        );
        visual.simulate_keystrokes("right");
        editor.read_with(cx, |view, _| {
            assert!(matches!(view.last_enqueued_edit,
            Some(hex_project::Command::MoveClip {start,..}) if start==next))
        });
    }
}

#[gpui::test]
fn adaptive_grid_menu_empty_arranger_choice_survives_keyboard_snap_toggle(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let project = hex_project::Project::new(48_000).unwrap();
    let original = project.clone();
    let (handle, editor, _directory) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual.debug_bounds("arrangement-content").unwrap();
    visual.simulate_mouse_down(body.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(body.center(), MouseButton::Right, Modifiers::default());
    editor.read_with(cx, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert_eq!(menu.items.len(), 7);
        assert_eq!(menu.items[0].label, "insert track");
        assert!(matches!(
            menu.items.last().map(|item| &item.action),
            Some(EditorContextAction::WaveformDecibels { enabled: true, .. })
        ));
    });
    visual.simulate_keystrokes("up up enter");
    assert_eq!(
        editor.read_with(cx, |view, _| view.snap_grid.density()),
        AdaptiveGridDensity::Coarse
    );
    for off in [true, false] {
        visual.simulate_keystrokes("cmd-4");
        editor.read_with(cx, |view, _| {
            assert_eq!(view.snap_grid.is_off(), off);
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
            assert_eq!(*view.state.as_ref().unwrap().project, original);
            assert!(view.last_enqueued_edit.is_none());
        });
    }
}

#[gpui::test]
fn grid_fixture_real_catalog_retains_one_root_after_view_and_caller_drop(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let environment = super::super::super::test_environment::Environment::new().unwrap();
    let root = environment.root().to_owned();
    std::fs::create_dir(root.join("samples")).unwrap();
    let wav = root.join("samples/grid.wav");
    let bytes = [
        b"RIFF".as_slice(),
        &44_u32.to_le_bytes(),
        b"WAVEfmt ",
        &16_u32.to_le_bytes(),
        &1_u16.to_le_bytes(),
        &1_u16.to_le_bytes(),
        &48_000_u32.to_le_bytes(),
        &96_000_u32.to_le_bytes(),
        &2_u16.to_le_bytes(),
        &16_u16.to_le_bytes(),
        b"data",
        &8_u32.to_le_bytes(),
        &[1, 0, 1, 0, 1, 0, 1, 0],
    ]
    .concat();
    std::fs::write(&wav, &bytes).unwrap();
    let weak = Arc::downgrade(&environment);
    let gate = environment.gate_next_catalog_save();
    let (project, _, _, _) = audio_project();
    let (handle, editor, owner) =
        editor_window_with_environment(cx, project, Arc::default(), environment);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(gate.reached.load(Ordering::Acquire));
    assert!(!gate.finished.load(Ordering::Acquire));
    let paths = editor.read_with(&visual, |view, _| {
        assert!(Arc::ptr_eq(&owner.1, &view.test_environment));
        assert_eq!(view.browser.directory, owner.0.join("samples"));
        assert_eq!(view.browser.places, vec![owner.0.join("samples")]);
        [
            view.browser.test_preferences_path().to_owned(),
            view.recent_projects.test_path().to_owned(),
            view.theme_settings.test_path().to_owned(),
            view.project_directory.test_path().to_owned(),
        ]
    });
    for (path, name) in paths.iter().zip([
        "browser.json",
        "recent-projects.json",
        "theme.json",
        "project-directory.json",
    ]) {
        assert_eq!(*path, root.join("config").join(name));
    }
    let database = paths[0].with_extension("samples-v1.db");
    assert_eq!(database, root.join("config/browser.samples-v1.db"));
    assert!(!database.exists());
    let editor_weak = editor.downgrade();
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| gpui::Empty);
    });
    drop(editor);
    drop(owner);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(editor_weak.upgrade().is_none());
    assert_eq!(
        weak.strong_count(),
        1,
        "actual held catalog job retains its path owner"
    );
    assert_eq!(std::fs::read(&wav).unwrap(), bytes);
    for path in &paths {
        assert!(path.parent().unwrap().is_dir());
    }
    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(weak.upgrade().is_none());
    let deadline = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "owned grid root cleanup"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(
        !root.exists(),
        "retired catalog job cannot recreate grid root"
    );
    eprintln!(
        "grid fixture SAME Environment/WAV/fourprefs/database, actual held save, view+caller retired, cleanup verified"
    );
}

#[gpui::test]
fn plus_minus_adjust_adaptive_density_with_clamped_limits_and_visible_snap_landmarks(
    cx: &mut TestAppContext,
) {
    use super::super::super::viewport::Viewport;
    cx.update(super::super::super::bind_editor_keys);
    for length in [96_000, 1_152_000] {
        let (project, track, _, clip) = audio_project();
        let original = project.snapshot();
        let (handle, editor, _directory) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.selected_track = Some(track);
                view.selected_clip = Some(clip);
                view.viewport = Some(Viewport::fit(length));
                view.focus.focus(window);
                cx.notify();
            })
        });
        visual.run_until_parked();
        for (key, density) in [
            ("+", AdaptiveGridDensity::Coarse),
            ("+", AdaptiveGridDensity::Coarse),
            ("-", AdaptiveGridDensity::Medium),
            ("-", AdaptiveGridDensity::Fine),
            ("-", AdaptiveGridDensity::Fine),
            ("+", AdaptiveGridDensity::Medium),
        ] {
            visual.simulate_keystrokes(key);
            visual.run_until_parked();
            let next = editor.read_with(&visual, |view, _| {
                assert_eq!(view.snap_grid.density(), density);
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original);
                assert_eq!(view.viewport, Some(Viewport::fit(length)));
                assert!(view.last_enqueued_edit.is_none());
                let (grid, viewport, width) = view.nudge_grid_context(false).unwrap();
                let project = &view.state.as_ref().unwrap().project;
                let next = grid
                    .adjacent_frame(project, 0, viewport, width, true)
                    .unwrap();
                assert!(
                    grid_lines(project, viewport, width, grid)
                        .iter()
                        .any(|line| line.frame == next)
                );
                assert_eq!(grid.snap_frame(project, next, viewport, width), Some(next));
                next
            });
            let selector = Box::leak(format!("arrangement-grid-line-{next}").into_boxed_str());
            assert!(
                visual.debug_bounds(selector).is_some(),
                "new snap interval is rendered"
            );
        }
        editor.update(&mut visual, |view, cx| {
            view.toggle_snap_grid();
            cx.notify();
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("+");
        editor.read_with(&visual, |view, _| {
            assert!(view.snap_grid.is_off());
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
        });
        editor.update(&mut visual, |view, cx| {
            view.toggle_snap_grid();
            cx.notify();
        });
        visual.run_until_parked();
        let next = editor.read_with(&visual, |view, _| {
            let (grid, viewport, width) = view.nudge_grid_context(false).unwrap();
            grid.adjacent_frame(
                &view.state.as_ref().unwrap().project,
                0,
                viewport,
                width,
                true,
            )
            .unwrap()
        });
        visual.simulate_keystrokes("right");
        editor.read_with(&visual, |view, _| {
            assert!(matches!(view.last_enqueued_edit, Some(hex_project::Command::MoveClip { start, .. }) if start == next));
        });
    }
}

#[gpui::test]
fn plus_minus_preserve_text_numeric_piano_and_session_input_and_command_zoom(
    cx: &mut TestAppContext,
) {
    use super::super::super::{EditorMode, clip_drag_ui_test, viewport::Viewport};
    cx.update(super::super::super::bind_editor_keys);
    for target in 0..5 {
        let (project, track, _, clip) = if target == 3 {
            clip_drag_ui_test::midi_project()
        } else {
            audio_project()
        };
        let (handle, editor, _directory) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.selected_track = Some(track);
                view.selected_clip = Some(clip);
                view.clip_view_open = true;
                match target {
                    0 => view.browser.search.read(cx).focus(window),
                    1 => view.begin_clip_name(window, cx),
                    3 => view.midi_focus.focus(window),
                    4 => {
                        view.mode = EditorMode::Session;
                        view.focus.focus(window);
                    }
                    _ => view.focus.focus(window),
                }
                cx.notify();
            })
        });
        visual.run_until_parked();
        if target == 2 {
            let numeric = visual.debug_bounds("project-tempo-value").unwrap();
            visual.simulate_click(numeric.center(), Modifiers::default());
            visual.run_until_parked();
            editor.update(&mut visual, |view, cx| {
                assert!(view.project_tempo_editing(cx))
            });
        }
        let piano_grid = editor.read_with(&visual, |view, _| view.midi_snap_grid);
        visual.simulate_keystrokes("+ -");
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert_eq!(view.snap_grid, SnapGrid::Adaptive, "input target {target}");
            assert_eq!(view.midi_snap_grid, piano_grid);
            assert!(view.last_enqueued_edit.is_none());
            if target == 0 {
                assert_eq!(view.browser.search.read(cx).text(), "+-");
            }
        });
        if target == 1 {
            visual.simulate_keystrokes("enter");
            editor.read_with(&visual, |view, _| {
                assert!(matches!(&view.last_enqueued_edit, Some(hex_project::Command::RenameClip { name, .. }) if name == "+-"));
            });
        } else if target == 2 {
            editor.update(&mut visual, |view, cx| {
                assert!(view.project_tempo_editing(cx))
            });
        }
    }
    let (project, _, _, _) = audio_project();
    let (handle, editor, _directory) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.viewport = Some(Viewport::fit(384_000));
            view.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    let before = editor.read_with(&visual, |view, _| view.viewport);
    visual.simulate_keystrokes("cmd-+");
    visual.run_until_parked();
    let zoomed = editor.read_with(&visual, |view, _| {
        assert_eq!(view.snap_grid, SnapGrid::Adaptive);
        assert_ne!(view.viewport, before);
        view.viewport
    });
    visual.simulate_keystrokes("cmd--");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.snap_grid, SnapGrid::Adaptive);
        assert_ne!(view.viewport, zoomed);
    });
}
