//! Console drop regressions use real events, the real worker and retained private roots.
use super::{BrowserSection, EditorMode, EditorView};
use gpui::{
    AppContext, Empty, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_library::{DirectoryListing, EntryKind, LibraryEntry};
use hex_project::{Command, SceneId, TrackId};
use hex_session::service::{Request, Service};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

fn receive_worker(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let done = editor.update(cx, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.state.is_some() && view.worker_requests_pending == 0 && view.audio_batch.is_none()
        });
        if done {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real Console worker timed out"
        );
        std::thread::yield_now();
    }
}

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    TrackId,
    SceneId,
    PathBuf,
) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    cx.update(super::super::bind_editor_keys);
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                // Since #414 the ordinary test constructor allocates and retains all
                // three preference roots and private home/Music BEFORE startup jobs.
                EditorView::new(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Err("offline test".into()),
                )
            })
        })
        .unwrap();
    receive_worker(&editor, cx);
    for request in [
        Request::Edit(Command::AddTrack {
            name: "target".into(),
        }),
        Request::Edit(Command::AddScene {
            name: "first".into(),
        }),
    ] {
        editor.update(cx, |view, _| assert!(view.enqueue(request)));
        receive_worker(&editor, cx);
    }
    let (track, scene, path) = editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let track = state.project.tracks()[0].id();
        let scene = state.project.scenes()[0].id();
        let directory = view.test_environment.home.join("Music");
        assert!(directory.starts_with(view.test_environment.root()));
        let path = directory.join(format!(
            "ConsoleCase-{}.WAV",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let frames = 512_u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&8192_i16.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        view.browser.scans.generation = view.browser.scans.generation.saturating_add(1);
        view.browser.places = vec![directory.clone()];
        view.browser.directory = directory.clone();
        view.browser.loading = false;
        view.browser.error = None;
        view.browser.listing = Some(DirectoryListing {
            directory: directory.clone(),
            entries: vec![LibraryEntry {
                name: "ConsoleCase.WAV".into(),
                path: path.clone(),
                kind: EntryKind::WaveAudio,
            }],
            enumerated_entries: 1,
            truncated: false,
        });
        view.browser.section = BrowserSection::Place(directory);
        view.mode = EditorMode::Session;
        view.selected_cell = Some((track, scene));
        cx.notify();
        (track, scene, path)
    });
    (handle, editor, track, scene, path)
}

#[gpui::test]
fn console_browser_drop_real_empty_cell_retires_ghost_and_imports_audio(cx: &mut TestAppContext) {
    let (handle, editor, track, scene, path) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("actual browser audio row");
    let column = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let row = visual.debug_bounds("session-scene-column").unwrap();
    let target = point(column.center().x, row.center().y);
    visual.simulate_mouse_down(source.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(source.center().x + px(14.0), source.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.active_browser_audio_drag.is_some(),
            "actual browser drag began"
        )
    });
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(
            view.active_browser_audio_drag.is_none(),
            "Console release must retire the browser ghost"
        )
    });
    receive_worker(&editor, cx);
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        let Some(hex_project::SessionCellContent::Audio(clip)) =
            state.project.session_cell(track, scene)
        else {
            panic!("drop did not create a canonical audio clip");
        };
        assert_eq!(state.project.asset(clip.asset()).unwrap().path(), path);
        assert_eq!(clip.source_end(), 512);
        assert!(state.waveforms.contains_key(&clip.asset()));
    });
}

fn bounds(visual: &mut VisualTestContext, name: String) -> gpui::Bounds<gpui::Pixels> {
    let selector = Box::leak(name.into_boxed_str());
    visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("actual Console hit surface: {selector}"))
}
fn begin_browser(
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
) -> super::super::drag::BrowserAudioDrag {
    visual.run_until_parked();
    let source = visual.debug_bounds("browser-entry-0").unwrap().center();
    visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(source.x + px(14.0), source.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        view.active_browser_audio_drag
            .clone()
            .expect("real browser payload")
    })
}
fn release(visual: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
    visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
}
fn project(
    editor: &gpui::Entity<EditorView>,
    cx: &TestAppContext,
) -> std::sync::Arc<hex_project::Project> {
    editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone())
}
fn assert_retired(drag: &super::super::drag::BrowserAudioDrag) {
    assert!(drag.destination.get().is_none());
    assert!(drag.preview.get().is_none());
    assert!(drag.insertion_line_y.get().is_none());
    assert!(drag.last_pointer.get().is_none());
    assert!(!drag.lane_hovered.get());
    assert!(drag.scroll.get().viewport.is_none());
}
// Canonical Undo deliberately retains next_id at its committed high-water
// mark (history.rs), while every authored field must restore exactly.
fn restored(actual: &hex_project::Project, expected: &hex_project::Project, frontier: u64) {
    assert_eq!(actual.next_identity(), frontier);
    assert_eq!(actual.sample_rate(), expected.sample_rate());
    assert_eq!(actual.tuning(), expected.tuning());
    assert_eq!(actual.authored_routing(), expected.authored_routing());
    assert_eq!(actual.audio_feeds(), expected.audio_feeds());
    assert_eq!(actual.midi_routes(), expected.midi_routes());
    assert_eq!(actual.import_provenance(), expected.import_provenance());
    assert_eq!(
        actual.assets().collect::<Vec<_>>(),
        expected.assets().collect::<Vec<_>>()
    );
    assert_eq!(actual.tracks(), expected.tracks());
    assert_eq!(actual.return_tracks(), expected.return_tracks());
    assert_eq!(actual.session(), expected.session());
    assert_eq!(actual.loop_region(), expected.loop_region());
    assert_eq!(actual.master_gain(), expected.master_gain());
    assert_eq!(actual.tempo_map(), expected.tempo_map());
    assert_eq!(actual.meter_map(), expected.meter_map());
}

fn history(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext, request: Request) {
    editor.update(cx, |view, _| assert!(view.enqueue(request)));
    receive_worker(editor, cx);
}

#[gpui::test]
fn console_browser_between_and_bottom_are_actual_atomic_scene_targets(cx: &mut TestAppContext) {
    let (handle, editor, track, scene, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = project(&editor, cx);
    let at = bounds(
        &mut visual,
        format!("console-insert-before-{}-{}", track.value(), scene.value()),
    )
    .center();
    let drag = begin_browser(&mut visual, &editor);
    visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
    assert_eq!(
        drag.destination.get(),
        Some(super::super::drag::BrowserDropDestination::Console(
            hex_session::import_batch::ConsoleDestination::InsertScene {
                track,
                before: Some(scene)
            }
        ))
    );
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    assert_retired(&drag);
    receive_worker(&editor, cx);
    let inserted = project(&editor, cx);
    assert_eq!(inserted.scenes().len(), 2);
    assert_eq!(inserted.scenes()[1].id(), scene);
    assert!(matches!(
        inserted.session_cell(track, inserted.scenes()[0].id()),
        Some(hex_project::SessionCellContent::Audio(_))
    ));
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &before, inserted.next_identity());
    history(&editor, cx, Request::Redo);
    assert_eq!(*project(&editor, cx), *inserted);
    visual.run_until_parked();
    let at = bounds(
        &mut visual,
        format!("console-append-track-{}", track.value()),
    )
    .center();
    let drag = begin_browser(&mut visual, &editor);
    release(&mut visual, at);
    assert_retired(&drag);
    receive_worker(&editor, cx);
    let bottom = project(&editor, cx);
    assert_eq!(bottom.scenes().len(), 3);
    assert_eq!(&bottom.scenes()[..2], inserted.scenes());
    assert!(matches!(
        bottom.session_cell(track, bottom.scenes()[2].id()),
        Some(hex_project::SessionCellContent::Audio(_))
    ));
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &inserted, bottom.next_identity());
    history(&editor, cx, Request::Redo);
    assert_eq!(*project(&editor, cx), *bottom);
}

#[gpui::test]
fn console_occupied_failure_then_replace_preserves_old_and_one_undo(cx: &mut TestAppContext) {
    let (handle, editor, track, scene, path) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let target = bounds(
        &mut visual,
        format!("console-cell-{}-{}", track.value(), scene.value()),
    )
    .center();
    begin_browser(&mut visual, &editor);
    release(&mut visual, target);
    receive_worker(&editor, cx);
    let before = project(&editor, cx);
    let original_bytes = std::fs::read(&path).unwrap();
    // Own fixture file only: real decoder failure must not replace the old cell.
    std::fs::write(&path, b"not a wave file").unwrap();
    let failed = begin_browser(&mut visual, &editor);
    release(&mut visual, target);
    assert_retired(&failed);
    receive_worker(&editor, cx);
    assert_eq!(*project(&editor, cx), *before);
    editor.read_with(cx, |view, _| assert!(view.error.is_some()));
    std::fs::write(&path, original_bytes).unwrap();
    let drag = begin_browser(&mut visual, &editor);
    // Selecting another cell/scene cannot redirect the actual drop hit surface.
    editor.update(cx, |view, _| {
        view.selected_cell = None;
        view.selected_scene = None;
    });
    release(&mut visual, target);
    assert_retired(&drag);
    receive_worker(&editor, cx);
    let after = project(&editor, cx);
    assert_ne!(*after, *before);
    assert_eq!(after.scenes(), before.scenes());
    assert_eq!(after.session_cells().len(), before.session_cells().len());
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &before, after.next_identity());
    history(&editor, cx, Request::Redo);
    assert_eq!(*project(&editor, cx), *after);
}

#[gpui::test]
fn console_scene_context_actual_row_empty_list_focus_and_history(cx: &mut TestAppContext) {
    let (handle, editor, _, scene, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = project(&editor, cx);
    let target = bounds(&mut visual, format!("console-scene-menu-{}", scene.value())).center();
    visual.simulate_mouse_down(target, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert_eq!(
            menu.items.iter().map(|item| item.label).collect::<Vec<_>>(),
            ["add scene"]
        );
    });
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx);
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
    let inserted = project(&editor, cx);
    assert_eq!(inserted.scenes().len(), 2);
    assert_eq!(
        inserted.scenes()[1].id(),
        scene,
        "actual right-clicked row is the before anchor"
    );
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &before, inserted.next_identity());
    // Empty projects still expose explicit Add Scene without a track or drag.
    editor.update(cx, |view, _| {
        view.begin_open_request(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        })
    });
    receive_worker(&editor, cx);
    visual.run_until_parked();
    let empty = project(&editor, cx);
    assert!(empty.tracks().is_empty() && empty.scenes().is_empty());
    let target = bounds(&mut visual, "console-append-row".into()).center();
    visual.simulate_mouse_down(target, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx);
    let one = project(&editor, cx);
    assert_eq!(one.scenes().len(), 1);
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &empty, one.next_identity());
}

#[gpui::test]
fn console_browser_cancel_outside_refusal_and_stale_scan_retire_only_owned_payload(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, scene, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let target = bounds(
        &mut visual,
        format!("console-cell-{}-{}", track.value(), scene.value()),
    )
    .center();
    let before = project(&editor, cx);
    for case in 0..4 {
        let drag = begin_browser(&mut visual, &editor);
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        match case {
            0 => {
                visual.simulate_keystrokes("escape");
                // GPUI may still carry its zero-sized payload until mouseup;
                // actual late drop must refuse the now-retired token.
                visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            }
            1 => release(&mut visual, point(px(8.0), px(8.0))),
            2 => {
                editor.update(cx, |view, _| view.save_dialog_open = true);
                visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
                editor.update(cx, |view, _| view.save_dialog_open = false);
            }
            _ => {
                editor.update(cx, |view, _| view.browser.scans.generation += 1);
                visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            }
        }
        assert_retired(&drag);
        receive_worker(&editor, cx);
        editor.read_with(cx, |view, _| {
            assert!(view.active_browser_audio_drag.is_none(), "case {case}")
        });
        assert_eq!(
            *project(&editor, cx),
            *before,
            "case {case} must not import"
        );
    }
    let old = begin_browser(&mut visual, &editor);
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let fresh = begin_browser(&mut visual, &editor);
    editor.update(cx, |view, _| view.clear_browser_audio_drag(&old));
    editor.read_with(cx, |view, _| {
        assert!(std::rc::Rc::ptr_eq(
            &view.active_browser_audio_drag.as_ref().unwrap().destination,
            &fresh.destination
        ))
    });
    release(&mut visual, target);
    assert_retired(&fresh);
    receive_worker(&editor, cx);
    assert!(project(&editor, cx).session_cell(track, scene).is_some());
}

#[gpui::test]
fn console_browser_view_project_and_deleted_target_changes_reject_late_drop(
    cx: &mut TestAppContext,
) {
    for case in 0..4 {
        let (handle, editor, track, scene, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let at = bounds(
            &mut visual,
            format!("console-cell-{}-{}", track.value(), scene.value()),
        )
        .center();
        let drag = begin_browser(&mut visual, &editor);
        match case {
            0 => editor.update(cx, |view, cx| {
                view.set_mode(EditorMode::Arrangement);
                cx.notify();
            }),
            1 => {
                editor.update(cx, |view, _| {
                    view.begin_open_request(Request::NewWithAudioMode {
                        mode: hex_session::DocumentAudioMode::Resident,
                        reservation: None,
                    })
                });
                receive_worker(&editor, cx);
            }
            2 => history(&editor, cx, Request::Edit(Command::RemoveScene(scene))),
            _ => history(&editor, cx, Request::Edit(Command::RemoveTrack(track))),
        }
        let before = project(&editor, cx);
        assert_retired(&drag);
        release(&mut visual, at);
        receive_worker(&editor, cx);
        assert_eq!(
            *project(&editor, cx),
            *before,
            "late case {case} cannot apply to changed document/target"
        );
    }
}

#[gpui::test]
fn console_actual_verified_worker_drop_uses_typed_session_clip_and_file_source(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, scene, path) = fixture(cx);
    history(
        &editor,
        cx,
        Request::Edit(Command::RemoveInstrument { track }),
    );
    let saved = editor.read_with(cx, |view, _| {
        view.test_environment.home.join("streaming.hex")
    });
    history(&editor, cx, Request::SaveNew(saved.clone()));
    let reservation = hex_session::SourcePlanReservation {
        reader: hex_media::VerifiedWavReaderLimits {
            reader: hex_media::WavReaderLimits {
                max_file_bytes: 64 * 1024 * 1024,
                max_source_frames: 8_000_000,
                max_header_count: 32,
                max_window_frames: 4096,
            },
            max_manifest_bytes: 1024 * 1024,
        },
        max_retained_sources: 16,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 4 * 1024 * 1024,
    };
    editor.update(cx, |view, _| {
        view.begin_open_request(Request::OpenWithAudioMode {
            source: hex_session::service::ProjectSource::Saved(saved.clone()),
            relinks: Default::default(),
            mode: hex_session::DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation),
        })
    });
    receive_worker(&editor, cx);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.document_audio_mode,
            hex_session::DocumentAudioMode::VerifiedStreaming
        )
    });
    let before = project(&editor, cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let target = bounds(
        &mut visual,
        format!("console-cell-{}-{}", track.value(), scene.value()),
    )
    .center();
    let drag = begin_browser(&mut visual, &editor);
    release(&mut visual, target);
    assert_retired(&drag);
    receive_worker(&editor, cx);
    let after = project(&editor, cx);
    let Some(hex_project::SessionCellContent::Audio(clip)) = after.session_cell(track, scene)
    else {
        panic!("real verified Session import");
    };
    assert_eq!(after.asset(clip.asset()).unwrap().path(), path);
    editor.read_with(cx, |view, _| {
        let source = view
            .state
            .as_ref()
            .unwrap()
            .canonical_audio_source
            .as_ref()
            .unwrap();
        assert_eq!(
            source.audio_mode(),
            hex_session::DocumentAudioMode::VerifiedStreaming
        );
        assert!(matches!(
            source.sources().get(&clip.asset()),
            Some(hex_session::AudioSourceBinding::VerifiedFile(_))
        ));
    });
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &before, after.next_identity());
    history(&editor, cx, Request::Redo);
    assert_eq!(*project(&editor, cx), *after);
    history(&editor, cx, Request::SaveCurrent);
    editor.update(cx, |view, _| {
        view.begin_open_request(Request::OpenWithAudioMode {
            source: hex_session::service::ProjectSource::Saved(saved),
            relinks: Default::default(),
            mode: hex_session::DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation),
        })
    });
    receive_worker(&editor, cx);
    assert_eq!(
        *project(&editor, cx),
        *after,
        "current-format save/reopen retains actual Console clip and scene identity"
    );
}

#[gpui::test]
fn console_release_hit_not_last_move_or_selection_owns_the_drop(cx: &mut TestAppContext) {
    let (handle, editor, track, first, _) = fixture(cx);
    history(
        &editor,
        cx,
        Request::Edit(Command::AddScene {
            name: "second".into(),
        }),
    );
    let second = project(&editor, cx).scenes()[1].id();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let first_at = bounds(
        &mut visual,
        format!("console-cell-{}-{}", track.value(), first.value()),
    )
    .center();
    let second_at = bounds(
        &mut visual,
        format!("console-cell-{}-{}", track.value(), second.value()),
    )
    .center();
    let drag = begin_browser(&mut visual, &editor);
    visual.simulate_mouse_move(first_at, MouseButton::Left, Modifiers::default());
    assert_eq!(
        drag.destination.get(),
        Some(super::super::drag::BrowserDropDestination::Console(
            hex_session::import_batch::ConsoleDestination::Cell {
                track,
                scene: first
            }
        ))
    );
    // A fast release can carry a new position without an intervening Move.
    // The real receiver must run before deferred Browser cleanup and use its hit.
    visual.simulate_mouse_up(second_at, MouseButton::Left, Modifiers::default());
    assert_retired(&drag);
    receive_worker(&editor, cx);
    let state = project(&editor, cx);
    assert!(state.session_cell(track, first).is_none());
    assert!(matches!(
        state.session_cell(track, second),
        Some(hex_project::SessionCellContent::Audio(_))
    ));
    // A stale captured menu cannot append after its actual anchor is removed.
    visual.run_until_parked();
    let menu_at = bounds(&mut visual, format!("console-scene-menu-{}", first.value())).center();
    visual.simulate_mouse_down(menu_at, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(menu_at, MouseButton::Right, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_some()));
    history(&editor, cx, Request::Edit(Command::RemoveScene(first)));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
    let before = project(&editor, cx);
    visual.simulate_keystrokes("enter");
    receive_worker(&editor, cx);
    assert_eq!(*project(&editor, cx), *before);
}

#[gpui::test]
fn console_browser_window_deactivation_cancels_ghost_without_late_import(cx: &mut TestAppContext) {
    let (handle, editor, track, scene, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    visual.update(|window, _| assert!(window.is_window_active(), "real active precondition"));
    let before = project(&editor, cx);
    let target = bounds(
        &mut visual,
        format!("console-cell-{}-{}", track.value(), scene.value()),
    )
    .center();
    let drag = begin_browser(&mut visual, &editor);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.deactivate_window();
    visual.run_until_parked();
    visual.update(|window, _| assert!(!window.is_window_active(), "actual inactive transition"));
    editor.read_with(cx, |view, _| {
        assert!(
            view.active_browser_audio_drag.is_none(),
            "window cancellation must retire Console Browser capture"
        )
    });
    assert_retired(&drag);
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    receive_worker(&editor, cx);
    assert_eq!(*project(&editor, cx), *before);
}
