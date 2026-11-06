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
    let deadline = crate::test_wait::hang_deadline();
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
        editor.update(cx, |view, _| {
            assert!(view.enqueue(request), "request refused: {:?}", view.error)
        });
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
    editor.update(cx, |view, _| {
        assert!(view.enqueue(request), "request refused: {:?}", view.error)
    });
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

fn arrangement_midi_source(
    cx: &mut TestAppContext,
    editor: &gpui::Entity<EditorView>,
    track: TrackId,
) -> hex_project::ClipId {
    use hex_project::BeatTime;
    history(
        editor,
        cx,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).unwrap(),
        }),
    );
    let clip = project(editor, cx).track(track).unwrap().midi_clips()[0].id();
    for (start, pitch, channel, velocity) in
        [(BeatTime::ZERO, 60, 3, 99), (BeatTime::ONE, 67, 2, 71)]
    {
        history(
            editor,
            cx,
            Request::Edit(Command::AddMidiNote {
                clip,
                start,
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                pitch,
                channel,
                velocity,
            }),
        );
    }
    clip
}

fn begin_arrangement_clip_in_console(
    cx: &mut TestAppContext,
    handle: gpui::AnyWindowHandle,
    editor: &gpui::Entity<EditorView>,
    source: hex_project::ClipId,
) -> VisualTestContext {
    editor.update(cx, |view, cx| {
        view.set_mode(EditorMode::Arrangement);
        view.last_enqueued_edit = None;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let source_bounds = bounds(&mut visual, format!("clip-drag-surface-{}", source.value()));
    let down = source_bounds.center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let drag = view
            .active_clip_drag
            .as_ref()
            .expect("real Arrangement pointer capture");
        assert_eq!(drag.clip, source);
        assert!(view.drag_start_is_current(drag));
    });
    assert!(visual.update(|_, app| app.has_active_drag()));
    // The pinned420 source predates419's Tab binding. Use the SAME production
    // mode setter as its toolbar, retaining the real native drag payload; do not
    // manufacture a ClipDrag or claim this source has a keyboard binding.
    editor.update(&mut visual, |view, cx| {
        view.set_mode(EditorMode::Session);
        cx.notify();
    });
    visual.run_until_parked();
    visual
}

fn drop_actual_arrangement_midi(
    cx: &mut TestAppContext,
    handle: gpui::AnyWindowHandle,
    editor: &gpui::Entity<EditorView>,
    source: hex_project::ClipId,
    track: TrackId,
    scene: SceneId,
) -> bool {
    let mut visual = begin_arrangement_clip_in_console(cx, handle, editor, source);
    let target = bounds(
        &mut visual,
        format!("console-cell-{}-{}", track.value(), scene.value()),
    )
    .center();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let submitted = editor.read_with(&visual, |view, _| {
        assert!(view.active_clip_drag.is_none(), "drop retires the visible clip ghost");
        matches!(view.last_enqueued_edit, Some(Command::CopyArrangementMidiToSession { source: id, scene: target }) if id == source && target == scene)
    });
    assert!(
        !visual.update(|_, app| app.has_active_drag()),
        "actual GPUI drag ended"
    );
    receive_worker(editor, cx);
    submitted
}

#[gpui::test]
fn console_arrangement_midi_stop_cell_refuses_before_worker_and_preserves_stop(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, scene, _) = fixture(cx);
    let source = arrangement_midi_source(cx, &editor, track);
    history(
        &editor,
        cx,
        Request::Edit(Command::SetSessionStopCell { track, scene }),
    );
    let before = project(&editor, cx);
    let submitted = drop_actual_arrangement_midi(cx, handle, &editor, source, track, scene);
    editor.read_with(cx, |view, _| {
        eprintln!("actual Stop-cell copy submitted={submitted}, canonical={:?}, worker/UI error={:?}", view.state.as_ref().unwrap().project.session_cell(track, scene), view.error);
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), before.as_ref());
        assert!(matches!(view.state.as_ref().unwrap().project.session_cell(track, scene), Some(hex_project::SessionCellContent::Stop)));
        assert!(!submitted, "an explicit Stop cell is occupied: UI must refuse before enqueue, matching canonical ensure_empty");
        assert!(view.error.as_ref().unwrap().contains("existing empty console cell"));
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn console_arrangement_midi_empty_cell_copies_real_source_once_and_protects_occupied_cells(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, scene, path) = fixture(cx);
    let source = arrangement_midi_source(cx, &editor, track);
    let before = project(&editor, cx);
    assert!(drop_actual_arrangement_midi(
        cx, handle, &editor, source, track, scene
    ));
    let after = project(&editor, cx);
    let original = before.midi_clip(source).unwrap();
    let Some(hex_project::SessionCellContent::Midi(copy)) = after.session_cell(track, scene) else {
        panic!("actual canonical Session MIDI clip")
    };
    assert_ne!(copy.id(), source);
    assert_eq!(copy.name(), original.name());
    assert_eq!(copy.flavor(), original.flavor());
    let a = original.source();
    let b = copy.source();
    assert_eq!(
        (
            a.source_length(),
            a.source_start(),
            a.loop_start(),
            a.loop_end(),
            a.looping()
        ),
        (
            b.source_length(),
            b.source_start(),
            b.loop_start(),
            b.loop_end(),
            b.looping()
        )
    );
    assert_eq!(a.notes().len(), 2);
    assert_eq!(a.notes().len(), b.notes().len());
    for (a, b) in a.notes().iter().zip(b.notes()) {
        assert_ne!(a.id(), b.id());
        assert_eq!(
            (
                a.start(),
                a.duration(),
                a.pitch(),
                a.channel(),
                a.velocity()
            ),
            (
                b.start(),
                b.duration(),
                b.pitch(),
                b.channel(),
                b.velocity()
            )
        );
    }
    assert_eq!(after.midi_clip(source), before.midi_clip(source));
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &before, after.next_identity());
    history(&editor, cx, Request::Redo);
    assert_eq!(project(&editor, cx), after);
    // A real MIDI cell stays protected, then a real prepared Audio cell does too.
    assert!(!drop_actual_arrangement_midi(
        cx, handle, &editor, source, track, scene
    ));
    assert_eq!(project(&editor, cx), after);
    history(
        &editor,
        cx,
        Request::Edit(Command::ClearSessionCell { track, scene }),
    );
    history(&editor, cx, Request::ImportToSession { path, track, scene });
    let audio_before = project(&editor, cx);
    assert!(matches!(
        audio_before.session_cell(track, scene),
        Some(hex_project::SessionCellContent::Audio(_))
    ));
    assert!(!drop_actual_arrangement_midi(
        cx, handle, &editor, source, track, scene
    ));
    assert_eq!(project(&editor, cx), audio_before);
}

#[gpui::test]
fn console_column_sample_boundaries_create_atomic_tracks_and_preserve_cells(
    cx: &mut TestAppContext,
) {
    for placement in 0..3 {
        let (handle, editor, first, scene, path) = fixture(cx);
        history(
            &editor,
            cx,
            Request::Edit(Command::AddTrack {
                name: "second".into(),
            }),
        );
        history(
            &editor,
            cx,
            Request::Edit(Command::SetSessionStopCell {
                track: first,
                scene,
            }),
        );
        let before = project(&editor, cx);
        let second = before.tracks()[1].id();
        let mut visual = VisualTestContext::from_window(handle, cx);
        let drag = begin_browser(&mut visual, &editor);
        let selector = match placement {
            0 => format!("console-column-before-{}-{}", first.value(), scene.value()),
            1 => format!("console-column-before-{}-{}", second.value(), scene.value()),
            _ => format!("console-column-tail-{}", scene.value()),
        };
        let at = bounds(&mut visual, selector).center();
        visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(project(&editor, cx), before, "hover is read-only");
        let anchor = match placement {
            0 => Some(first),
            1 => Some(second),
            _ => None,
        };
        assert_eq!(
            drag.destination.get(),
            Some(super::super::drag::BrowserDropDestination::Console(
                hex_session::import_batch::ConsoleDestination::NewTrack {
                    before_track: anchor,
                    scene: hex_project::SessionSceneTarget::Existing(scene)
                }
            ))
        );
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        assert_retired(&drag);
        receive_worker(&editor, cx);
        let after = project(&editor, cx);
        assert_eq!(after.tracks().len(), 3);
        let born = &after.tracks()[placement];
        assert!(!before.tracks().iter().any(|track| track.id() == born.id()));
        assert!(born.instrument().is_none() && born.devices().is_empty());
        assert!((born.gain() - 0.251_188_64).abs() < 1e-7);
        assert_eq!(
            after.track(first).unwrap().gain(),
            before.track(first).unwrap().gain()
        );
        assert_eq!(
            after.track(second).unwrap().gain(),
            before.track(second).unwrap().gain()
        );
        let Some(hex_project::SessionCellContent::Audio(clip)) =
            after.session_cell(born.id(), scene)
        else {
            panic!("new column contains prepared sample")
        };
        assert_eq!(after.asset(clip.asset()).unwrap().path(), path);
        assert_eq!(
            after.session_cell(first, scene),
            before.session_cell(first, scene)
        );
        history(&editor, cx, Request::Undo);
        restored(&project(&editor, cx), &before, after.next_identity());
        history(&editor, cx, Request::Redo);
        assert_eq!(project(&editor, cx), after);
    }
}

#[gpui::test]
fn console_column_empty_and_append_row_birth_track_scene_clip_together(cx: &mut TestAppContext) {
    for empty in [false, true] {
        let (handle, editor, track, scene, _) = fixture(cx);
        if empty {
            history(&editor, cx, Request::Edit(Command::RemoveTrack(track)));
            history(&editor, cx, Request::Edit(Command::RemoveScene(scene)));
        }
        let before = project(&editor, cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let drag = begin_browser(&mut visual, &editor);
        let at = bounds(&mut visual, "console-column-append-tail".into()).center();
        release(&mut visual, at);
        assert_retired(&drag);
        receive_worker(&editor, cx);
        let after = project(&editor, cx);
        assert_eq!(after.tracks().len(), before.tracks().len() + 1);
        assert_eq!(after.scenes().len(), before.scenes().len() + 1);
        let track = after.tracks().last().unwrap();
        let scene = after.scenes().last().unwrap();
        assert!(matches!(
            after.session_cell(track.id(), scene.id()),
            Some(hex_project::SessionCellContent::Audio(_))
        ));
        history(&editor, cx, Request::Undo);
        restored(&project(&editor, cx), &before, after.next_identity());
    }
}

#[gpui::test]
fn console_column_paged_narrow_target_uses_painted_anchor_and_rejects_stale_source(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, scene, _) = fixture(cx);
    for i in 0..6 {
        history(
            &editor,
            cx,
            Request::Edit(Command::AddTrack {
                name: format!("column {i}"),
            }),
        );
    }
    editor.update(cx, |view, cx| {
        view.session_track_start = 3;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(900.0), px(600.0)));
    visual.run_until_parked();
    let before = project(&editor, cx);
    let anchor = before.tracks()[3].id();
    let drag = begin_browser(&mut visual, &editor);
    let at = bounds(
        &mut visual,
        format!("console-column-before-{}-{}", anchor.value(), scene.value()),
    )
    .center();
    release(&mut visual, at);
    assert_retired(&drag);
    receive_worker(&editor, cx);
    let after = project(&editor, cx);
    assert_eq!(after.tracks()[4].id(), anchor);
    assert!(matches!(
        after.session_cell(after.tracks()[3].id(), scene),
        Some(hex_project::SessionCellContent::Audio(_))
    ));
    let stale = begin_browser(&mut visual, &editor);
    history(
        &editor,
        cx,
        Request::Edit(Command::AddScene {
            name: "intervening".into(),
        }),
    );
    let changed = project(&editor, cx);
    visual.run_until_parked();
    let at = bounds(
        &mut visual,
        format!("console-column-before-{}-{}", anchor.value(), scene.value()),
    )
    .center();
    release(&mut visual, at);
    receive_worker(&editor, cx);
    assert_retired(&stale);
    assert_eq!(project(&editor, cx), changed);
}

#[gpui::test]
fn console_clip_new_track_refuses_unavailable_defaults_but_existing_cell_still_works(
    cx: &mut TestAppContext,
) {
    for loading in [true, false] {
        let (handle, editor, track, scene, _) = fixture(cx);
        let source = arrangement_midi_source(cx, &editor, track);
        let before = project(&editor, cx);
        let mut visual = begin_arrangement_clip_in_console(cx, handle, &editor, source);
        editor.update(&mut visual, |view, _| {
            view.track_defaults.set_test_state(0.5, loading, false)
        });
        let at = bounds(
            &mut visual,
            format!("console-column-before-{}-{}", track.value(), scene.value()),
        )
        .center();
        release(&mut visual, at);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.worker_requests_pending, 0);
            assert!(view.error.as_deref().unwrap().contains(if loading {
                "loading"
            } else {
                "open settings"
            }));
            assert!(view.active_clip_drag.is_none());
        });
        assert_eq!(project(&editor, cx), before);
        editor.update(cx, |view, _| view.error = None);
        assert!(drop_actual_arrangement_midi(
            cx, handle, &editor, source, track, scene
        ));
        assert_eq!(
            project(&editor, cx).track(track).unwrap().gain(),
            before.track(track).unwrap().gain()
        );
    }
}

#[gpui::test]
fn console_column_real_arrangement_midi_copies_source_with_one_history_step(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, scene, _) = fixture(cx);
    let source = arrangement_midi_source(cx, &editor, track);
    let before = project(&editor, cx);
    let mut visual = begin_arrangement_clip_in_console(cx, handle, &editor, source);
    editor.update(&mut visual, |view, _| {
        view.track_defaults.set_test_state(0.5, false, true);
        view.error = Some("previous preference error".into());
    });
    let at = bounds(
        &mut visual,
        format!("console-column-before-{}-{}", track.value(), scene.value()),
    )
    .center();
    release(&mut visual, at);
    editor.read_with(&visual, |view, _| assert!(view.error.is_none()));
    editor.update(&mut visual, |view, _| {
        view.track_defaults.set_test_state(0.25, false, true)
    });
    editor.read_with(&visual, |view, _| assert!(view.active_clip_drag.is_none()));
    receive_worker(&editor, cx);
    let after = project(&editor, cx);
    assert_eq!(after.tracks().len(), before.tracks().len() + 1);
    let born = &after.tracks()[0];
    assert_eq!(born.gain(), 0.5);
    assert!(born.instrument().is_none());
    let Some(hex_project::SessionCellContent::Midi(copy)) = after.session_cell(born.id(), scene)
    else {
        panic!("actual MIDI copy in new column")
    };
    assert_ne!(copy.id(), source);
    assert_eq!(
        copy.source().source_length(),
        before.midi_clip(source).unwrap().source().source_length()
    );
    assert_eq!(after.midi_clip(source), before.midi_clip(source));
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &before, after.next_identity());
    history(&editor, cx, Request::Redo);
    assert_eq!(project(&editor, cx), after);
}

#[gpui::test]
fn console_column_browser_devices_create_only_explicit_device_and_one_atomic_track(
    cx: &mut TestAppContext,
) {
    for effect in [false, true] {
        let (handle, editor, track, _, _) = fixture(cx);
        editor.update(cx, |view, cx| {
            view.browser.section = BrowserSection::Instruments;
            cx.notify();
        });
        let before = project(&editor, cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let selector = if effect {
            "browser-device-effect-hex.utility"
        } else {
            "browser-device-instrument-hex.instrument.fm4"
        };
        let source = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(source.x + px(12.0), source.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.active_native_device_drag.is_some())
        });
        let target = if effect {
            "console-column-header-tail".to_string()
        } else {
            format!("console-column-header-before-{}", track.value())
        };
        let at = bounds(&mut visual, target).center();
        release(&mut visual, at);
        editor.read_with(&visual, |view, _| {
            assert!(view.active_native_device_drag.is_none())
        });
        receive_worker(&editor, cx);
        let after = project(&editor, cx);
        assert_eq!(after.tracks().len(), 2);
        assert_eq!(
            after.session(),
            before.session(),
            "device creates no clips or scenes"
        );
        let born = &after.tracks()[usize::from(effect)];
        assert!((born.gain() - 0.251_188_64).abs() < 1e-7);
        if effect {
            assert!(born.instrument().is_none());
            assert_eq!(born.devices().len(), 1);
            assert_eq!(
                born.devices()[0].state().type_id().as_str().unwrap(),
                "hex.utility"
            );
        } else {
            assert_eq!(
                born.instrument()
                    .unwrap()
                    .state()
                    .type_id()
                    .as_str()
                    .unwrap(),
                "hex.instrument.fm4"
            );
            assert!(born.devices().is_empty());
        }
        history(&editor, cx, Request::Undo);
        restored(&project(&editor, cx), &before, after.next_identity());
        history(&editor, cx, Request::Redo);
        assert_eq!(project(&editor, cx), after);
    }
}

#[gpui::test]
fn console_column_real_arrangement_audio_copies_plain_source_and_refuses_processing(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, scene, path) = fixture(cx);
    // Let the real asynchronous import-preference load finish before admission.
    VisualTestContext::from_window(handle, cx).run_until_parked();
    history(
        &editor,
        cx,
        Request::ImportToTrack {
            path,
            track,
            start: 0,
        },
    );
    let before = project(&editor, cx);
    let source = before.track(track).unwrap().clips()[0].id();
    // The fixture is 512 frames: zoom to expose its real body rather than a
    // subpixel edge at the default whole-song scale.
    editor.update(cx, |view, cx| {
        view.viewport = Some(super::super::viewport::Viewport {
            start: 0,
            length: 4096,
        });
        cx.notify();
    });
    let mut visual = begin_arrangement_clip_in_console(cx, handle, &editor, source);
    editor.update(&mut visual, |view, _| {
        view.track_defaults.set_test_state(0.5, false, true)
    });
    let at = bounds(
        &mut visual,
        format!("console-column-tail-{}", scene.value()),
    )
    .center();
    release(&mut visual, at);
    editor.update(&mut visual, |view, _| {
        view.track_defaults.set_test_state(0.25, false, true)
    });
    receive_worker(&editor, cx);
    let after = project(&editor, cx);
    assert_eq!(after.tracks().len(), 2);
    assert_eq!(after.tracks()[1].gain(), 0.5);
    let Some(hex_project::SessionCellContent::Audio(copy)) =
        after.session_cell(after.tracks()[1].id(), scene)
    else {
        panic!("actual plain audio copy")
    };
    assert_eq!(copy.asset(), before.clip(source).unwrap().asset());
    assert_eq!(after.clip(source), before.clip(source));
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &before, after.next_identity());
    history(
        &editor,
        cx,
        Request::Edit(Command::SetAudioClipPitch {
            clip: source,
            cents: 100,
        }),
    );
    let processed = project(&editor, cx);
    let mut visual = begin_arrangement_clip_in_console(cx, handle, &editor, source);
    let at = bounds(
        &mut visual,
        format!("console-column-tail-{}", scene.value()),
    )
    .center();
    release(&mut visual, at);
    receive_worker(&editor, cx);
    assert_eq!(
        project(&editor, cx),
        processed,
        "processed source is refused with no empty column"
    );
    editor.read_with(cx, |view, _| {
        assert!(view.active_clip_drag.is_none());
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("processed arrangement audio")
        );
    });
}

#[gpui::test]
fn console_column_stale_clip_release_preserves_a_new_capture_of_the_same_source(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, scene, _) = fixture(cx);
    let source = arrangement_midi_source(cx, &editor, track);
    let before = project(&editor, cx);
    let mut visual = begin_arrangement_clip_in_console(cx, handle, &editor, source);
    let old = editor.read_with(&visual, |view, _| view.active_clip_drag.clone().unwrap());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(
        point(px(8.0), px(8.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    let mut visual = begin_arrangement_clip_in_console(cx, handle, &editor, source);
    let fresh = editor.read_with(&visual, |view, _| view.active_clip_drag.clone().unwrap());
    assert!(!std::rc::Rc::ptr_eq(&old.group, &fresh.group));
    // Deliver an older receiver's payload after a new real pointer capture.
    // Same clip/generation/revision does not transfer ownership to that receiver.
    editor.update(cx, |view, cx| {
        view.drop_console_arrangement_clip(
            &old,
            hex_session::import_batch::ConsoleDestination::NewTrack {
                before_track: Some(track),
                scene: hex_project::SessionSceneTarget::Existing(scene),
            },
            cx,
        )
    });
    editor.read_with(cx, |view, _| {
        let active = view
            .active_clip_drag
            .as_ref()
            .expect("stale Console release must preserve newer pointer capture");
        assert!(std::rc::Rc::ptr_eq(&active.group, &fresh.group));
        assert_eq!(
            view.worker_requests_pending, 0,
            "stale receiver must not admit a duplicate birth"
        );
    });
    assert_eq!(project(&editor, cx), before);
    let at = bounds(
        &mut visual,
        format!("console-column-before-{}-{}", track.value(), scene.value()),
    )
    .center();
    release(&mut visual, at);
    receive_worker(&editor, cx);
    let after = project(&editor, cx);
    assert_eq!(after.tracks().len(), 2);
    history(&editor, cx, Request::Undo);
    restored(&project(&editor, cx), &before, after.next_identity());
}

#[gpui::test]
fn recorded_take_range_drag_imports_exact_pcm_in_arrangement_and_session(cx: &mut TestAppContext) {
    use hex_audio::sample_recording::{Source, Take};
    use hex_project::{Project, SessionCellContent};
    let selection =
        |project: &Project, mode, track, scene| -> Option<(hex_project::AssetId, u64)> {
            if mode == EditorMode::Arrangement {
                let clip = project.track(track)?.clips().first()?;
                Some((clip.asset(), clip.source_length()))
            } else {
                match project.session_cell(track, scene)? {
                    SessionCellContent::Audio(clip) => Some((clip.asset(), clip.source_end())),
                    _ => None,
                }
            }
        };
    for mode in [EditorMode::Arrangement, EditorMode::Session] {
        let (handle, editor, track, scene, path) = fixture(cx);
        let mut recorded = std::fs::read(&path).unwrap();
        for (index, bytes) in recorded[44..].chunks_exact_mut(2).enumerate() {
            bytes.copy_from_slice(&(8192 + index as i16).to_le_bytes());
        }
        std::fs::write(&path, recorded).unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(cx, |view, cx| {
            view.mode = mode;
            view.show_take_fixture(Take {
                source: Source::Main,
                path: path.clone(),
                frames: 512,
                sample_rate: 48_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let take_row = visual.debug_bounds("recorded-take-0").unwrap().center();
        visual.simulate_click(take_row, Modifiers::default());
        visual.run_until_parked();
        let waveform = visual.debug_bounds("take-waveform").unwrap();
        let from = point(
            waveform.left() + waveform.size.width * 0.25,
            waveform.center().y,
        );
        let to = point(
            waveform.left() + waveform.size.width * 0.75,
            waveform.center().y,
        );
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let deadline = crate::test_wait::hang_deadline();
        let source = loop {
            visual.run_until_parked();
            if let Some(source) = visual.debug_bounds("take-excerpt-drag") {
                break source.center();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "excerpt was not ready"
            );
            std::thread::yield_now();
        };
        let target = if mode == EditorMode::Arrangement {
            bounds(&mut visual, format!("arrangement-lane-{}", track.value())).center()
        } else {
            let column = bounds(&mut visual, format!("session-track-{}", track.value()));
            let row = visual.debug_bounds("session-scene-column").unwrap();
            point(column.center().x, row.center().y)
        };
        visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(source.x + px(14.0), source.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        receive_worker(&editor, cx);
        editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let (id, frames) =
                selection(project, mode, track, scene).expect("drop created an audio clip");
            assert_eq!(frames, 256);
            let media = project.asset(id).unwrap().path();
            assert_ne!(media, path);
            let wav = hex_media::decode_wav_file(media, hex_media::Limits::new(4096, 512)).unwrap();
            let expected: Vec<_> = (128..384)
                .map(|index| [(8192 + index) as f32 / 32768.0; 2])
                .collect();
            assert_eq!(wav.audio().frames(), expected);
        });
        for (request, present) in [(Request::Undo, false), (Request::Redo, true)] {
            editor.update(cx, |view, _| assert!(view.enqueue(request)));
            receive_worker(&editor, cx);
            editor.read_with(cx, |view, _| {
                let project = &view.state.as_ref().unwrap().project;
                assert_eq!(selection(project, mode, track, scene).is_some(), present);
            });
        }
        let document = path.with_extension("hex");
        for request in [
            Request::SaveNew(document.clone()),
            Request::Open(document.clone()),
        ] {
            editor.update(cx, |view, _| assert!(view.enqueue(request)));
            receive_worker(&editor, cx);
        }
        editor.read_with(cx, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(state.document_path.as_deref(), Some(document.as_path()));
            let (asset, frames) = selection(&state.project, mode, track, scene).unwrap();
            assert_eq!(frames, 256);
            assert!(state.project.asset(asset).unwrap().path().is_file());
        });
    }
}

#[gpui::test]
fn released_sample_drop_keeps_target_through_delayed_worker_repaints(cx: &mut TestAppContext) {
    for (mode, refuse) in [
        (EditorMode::Arrangement, false),
        (EditorMode::Session, false),
        (EditorMode::Session, true),
    ] {
        let (handle, editor, track, scene, path) = fixture(cx);
        editor.update(cx, |view, cx| {
            view.mode = mode;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        begin_browser(&mut visual, &editor);
        let (target, selector) = if mode == EditorMode::Arrangement {
            (
                bounds(&mut visual, format!("arrangement-lane-{}", track.value())).center(),
                "clip-drop-preview",
            )
        } else {
            let column = bounds(&mut visual, format!("session-track-{}", track.value()));
            let row = visual.debug_bounds("session-scene-column").unwrap();
            (
                point(column.center().x, row.center().y),
                "console-sample-drop-preview",
            )
        };
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let before = visual.debug_bounds(selector).expect("held target paint");
        if refuse {
            std::fs::remove_file(&path).unwrap();
        }
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(&mut visual, |view, cx| {
            assert!(view.active_browser_audio_drag.is_none());
            assert!(view.pending_sample_drop.is_some());
            let deadline = crate::test_wait::hang_deadline();
            loop {
                if let Ok(event) = view.service.try_event() {
                    if event.request_complete {
                        assert_eq!(event.error.is_some(), refuse);
                        cx.notify();
                        break event;
                    }
                    view.receive(event);
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "sample import terminal"
                );
                std::thread::yield_now();
            }
        });
        for _ in 0..2 {
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds(selector), Some(before));
            visual.simulate_keystrokes("escape");
        }
        editor.update(&mut visual, |view, cx| {
            view.receive(terminal);
            assert!(view.pending_sample_drop.is_none());
            let project = &view.state.as_ref().unwrap().project;
            if mode == EditorMode::Arrangement {
                assert_eq!(project.track(track).unwrap().clips().len(), 1);
            } else {
                assert_eq!(project.session_cell(track, scene).is_some(), !refuse);
            }
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds(selector).is_none());
    }
}
