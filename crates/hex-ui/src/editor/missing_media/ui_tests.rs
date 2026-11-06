//! Real Service open of a saved project whose media moved; disposable files only.
use super::*;
use crate::editor::test_environment;
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
use hex_session::service::Service;
use std::time::Instant;

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.state.is_some()
                && view.pending_open_path.is_none()
        });
        visual.run_until_parked();
        if done {
            return;
        }
        assert!(Instant::now() < until, "real Service did not settle");
        std::thread::yield_now();
    }
}

fn wav(path: &std::path::Path, frames: u32) {
    let mut bytes = Vec::from(*b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 2).to_le_bytes());
    bytes.resize(bytes.len() + frames as usize * 2, 0);
    std::fs::write(path, bytes).unwrap();
}

#[gpui::test]
fn moved_media_shows_panel_instead_of_failing_and_opens_with_relink(cx: &mut TestAppContext) {
    let environment = test_environment::Environment::new().unwrap();
    let root = environment.root().to_path_buf();
    std::fs::create_dir(root.join("samples")).unwrap();
    std::fs::create_dir(root.join("moved")).unwrap();
    let original = root.join("samples/kick.wav");
    wav(&original, 64);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    pump(&editor, &mut visual);
    let song = root.join("song.hex");
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Import(original.clone())));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(song.clone())));
    });
    pump(&editor, &mut visual);
    let moved = root.join("moved/kick.wav");
    std::fs::rename(&original, &moved).unwrap();
    let generation = editor.read_with(&visual, |view, _| view.project_generation);

    editor.update(&mut visual, |view, cx| {
        view.choose_project(song.clone(), DocumentAudioMode::Resident);
        cx.notify();
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.opening_project);
        assert_eq!(view.project_generation, generation);
        let panel = view.missing_media.as_ref().expect("missing-media panel");
        assert_eq!(panel.rows.len(), 1);
        assert_eq!(panel.rows[0].media.expected_path, original);
        assert_eq!(panel.rows[0].media.usage_count, 1);
    });
    assert!(visual.debug_bounds("missing-media-overlay").is_some());

    // The open stays disabled until every file is resolved.
    editor.update(&mut visual, |view, cx| view.missing_media_open(cx));
    assert!(editor.read_with(&visual, |view, _| view.missing_media.is_some()));
    editor.update(&mut visual, |view, cx| {
        let panel = view.missing_media.as_mut().unwrap();
        let media = panel.rows[0].media.clone();
        let fit = missing_media::check_candidate(&media, &moved);
        apply_locate(&mut panel.rows, 0, moved.clone(), fit, Vec::new());
        view.missing_media_open(cx);
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.missing_media.is_none() && !view.opening_project);
        assert_eq!(view.project_generation, generation + 1);
        let state = view.state.as_ref().unwrap();
        assert!(state.unsaved);
        assert_eq!(state.project.assets().next().unwrap().path(), moved);
    });
    assert!(visual.debug_bounds("missing-media-overlay").is_none());
}

#[gpui::test]
fn sample_catalog_matches_are_offered_then_relinked_all_at_once(cx: &mut TestAppContext) {
    let environment = test_environment::Environment::new().unwrap();
    let root = environment.root().to_path_buf();
    for folder in ["samples", "moved/a", "moved/b"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    let kick = root.join("samples/kick.wav");
    let snare = root.join("samples/snare.wav");
    let hat = root.join("samples/hat.wav");
    wav(&kick, 64);
    wav(&snare, 32);
    wav(&hat, 16);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    pump(&editor, &mut visual);
    let song = root.join("song.hex");
    for path in [&kick, &snare, &hat] {
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Import(path.clone())));
        });
        pump(&editor, &mut visual);
    }
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(song.clone())));
    });
    pump(&editor, &mut visual);
    // kick moves once; snare has two equal copies; hat is gone entirely.
    let moved_kick = root.join("moved/a/kick.wav");
    std::fs::rename(&kick, &moved_kick).unwrap();
    std::fs::copy(&snare, root.join("moved/b/snare.wav")).unwrap();
    std::fs::rename(&snare, root.join("moved/a/snare.wav")).unwrap();
    std::fs::remove_file(&hat).unwrap();
    let catalog =
        hex_library::catalog::scan_samples(std::slice::from_ref(&root.join("moved"))).catalog;
    let until = Instant::now() + std::time::Duration::from_secs(30);
    while !editor.update(&mut visual, |view, _| {
        view.test_replace_sample_catalog(catalog.clone())
    }) {
        visual.run_until_parked();
        assert!(Instant::now() < until, "sample catalog did not settle");
        std::thread::yield_now();
    }

    editor.update(&mut visual, |view, cx| {
        view.choose_project(song.clone(), DocumentAudioMode::Resident);
        cx.notify();
    });
    let until = crate::test_wait::hang_deadline();
    loop {
        pump(&editor, &mut visual);
        if editor.read_with(&visual, |view, _| {
            view.missing_media
                .as_ref()
                .is_some_and(|panel| panel.catalog.is_some())
        }) {
            break;
        }
        assert!(Instant::now() < until, "catalog lookup did not finish");
        std::thread::yield_now();
    }
    editor.read_with(&visual, |view, _| {
        let panel = view.missing_media.as_ref().unwrap();
        assert_eq!(panel.rows.len(), 3);
        // Offered, not applied.
        assert!(
            panel
                .rows
                .iter()
                .all(|row| row.resolution == Resolution::Missing)
        );
        assert_eq!(panel.catalog.as_ref().unwrap().counts(&panel.rows), (1, 1));
    });
    assert!(visual.debug_bounds("missing-media-overlay").is_some());
    editor.update(&mut visual, |view, cx| {
        view.missing_media_relink_catalog(cx)
    });
    editor.update(&mut visual, |view, cx| {
        let panel = view.missing_media.as_ref().unwrap();
        let by_name = |name: &str| {
            &panel
                .rows
                .iter()
                .find(|row| row.media.file_name() == name)
                .unwrap()
                .resolution
        };
        assert_eq!(
            *by_name("kick.wav"),
            Resolution::Found(moved_kick.clone(), CandidateFit::Matches)
        );
        assert!(matches!(by_name("snare.wav"), Resolution::Choices(paths) if paths.len() == 2));
        assert_eq!(*by_name("hat.wav"), Resolution::Missing);
        assert_eq!(panel.catalog.as_ref().unwrap().counts(&panel.rows), (0, 0));
        // Not complete: the project still waits for the remaining files.
        view.missing_media_open(cx);
        assert!(view.missing_media.is_some());
        view.missing_media_dismiss(cx);
    });
}

#[gpui::test]
fn missing_media_can_open_offline_and_shows_the_missing_count(cx: &mut TestAppContext) {
    let environment = test_environment::Environment::new().unwrap();
    let root = environment.root().to_path_buf();
    std::fs::create_dir(root.join("samples")).unwrap();
    let kick = root.join("samples/kick.wav");
    let snare = root.join("samples/snare.wav");
    wav(&kick, 64);
    wav(&snare, 32);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    pump(&editor, &mut visual);
    let song = root.join("song.hex");
    for path in [&kick, &snare] {
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Import(path.clone())));
        });
        pump(&editor, &mut visual);
    }
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(song.clone())));
    });
    pump(&editor, &mut visual);
    std::fs::remove_file(&kick).unwrap();
    let generation = editor.read_with(&visual, |view, _| view.project_generation);
    assert!(visual.debug_bounds("missing-media-indicator").is_none());

    editor.update(&mut visual, |view, cx| {
        view.choose_project(song.clone(), DocumentAudioMode::Resident);
        cx.notify();
    });
    pump(&editor, &mut visual);
    assert!(editor.read_with(&visual, |view, _| view.missing_media.is_some()));
    editor.update(&mut visual, |view, cx| {
        view.missing_media_open_with(true, cx)
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.missing_media.is_none() && !view.opening_project);
        assert_eq!(view.project_generation, generation + 1);
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.offline_assets().count(), 1);
        assert_eq!(
            project
                .tracks()
                .iter()
                .flat_map(|track| track.clips())
                .count(),
            2
        );
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("missing-media-indicator").is_some());
}

#[gpui::test]
fn offline_media_relinks_in_place_from_the_indicator_and_undoes(cx: &mut TestAppContext) {
    let environment = test_environment::Environment::new().unwrap();
    let root = environment.root().to_path_buf();
    for folder in ["samples", "moved"] {
        std::fs::create_dir(root.join(folder)).unwrap();
    }
    let kick = root.join("samples/kick.wav");
    wav(&kick, 64);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    pump(&editor, &mut visual);
    let song = root.join("song.hex");
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Import(kick.clone())));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(song.clone())));
    });
    pump(&editor, &mut visual);
    let moved = root.join("moved/kick.wav");
    std::fs::rename(&kick, &moved).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.choose_project(song.clone(), DocumentAudioMode::Resident);
        cx.notify();
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, cx| {
        view.missing_media_open_with(true, cx)
    });
    pump(&editor, &mut visual);
    visual.run_until_parked();
    let indicator = visual.debug_bounds("missing-media-indicator").unwrap();
    visual.simulate_click(indicator.center(), gpui::Modifiers::default());
    editor.update(&mut visual, |view, cx| {
        let panel = view.missing_media.as_mut().expect("relink panel");
        assert!(matches!(panel.target, Target::Relink));
        assert_eq!(panel.rows.len(), 1);
        assert_eq!(panel.rows[0].media.expected_path, kick);
        assert!(!view.opening_project, "an open project stays editable");
        let media = panel.rows[0].media.clone();
        let fit = missing_media::check_candidate(&media, &moved);
        apply_locate(&mut panel.rows, 0, moved.clone(), fit, Vec::new());
        view.close_prompt_open = true;
        view.missing_media_relink(cx);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(
            view.missing_media
                .as_ref()
                .is_some_and(|panel| panel.resolved().values().eq([&moved]))
        );
        view.close_prompt_open = false;
        view.missing_media_relink(cx);
        assert!(view.missing_media.is_none());
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.offline_assets().count(), 0);
        assert_eq!(project.assets().next().unwrap().path(), moved);
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("missing-media-indicator").is_none());
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Undo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.offline_assets().count(), 1);
    });
}
