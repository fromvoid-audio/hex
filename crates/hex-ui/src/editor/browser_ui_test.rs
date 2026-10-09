use super::{BrowserSection, EditorView, drag::BrowserAudioDrag};
use gpui::{
    AppContext, Empty, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_library::{DirectoryListing, EntryKind, LibraryEntry};
use hex_project::{Command, Created, Editor, Project, TrackId};
use hex_session::service::{Service, ViewState};
use std::{cell::RefCell, rc::Rc, sync::Arc};

struct PreferencesTestDirectory(std::path::PathBuf);

impl PreferencesTestDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-ui-browser-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("preferences test directory");
        Self(path)
    }
}

impl Drop for PreferencesTestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mono_pcm_wav(frame_count: usize) -> Vec<u8> {
    let samples = vec![4_000_i16; frame_count];
    let data = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes
}

fn browser_drag_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    TrackId,
    PreferencesTestDirectory,
    std::path::PathBuf,
) {
    let directory = PreferencesTestDirectory::new();
    let path = directory.0.join("loop.WAV");
    std::fs::write(&path, mono_pcm_wav(4_800)).expect("wav fixture");
    let mut model = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
    let Created::Track(track) = model
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .expect("add track")
    else {
        panic!("track result")
    };
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service =
                Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES).expect("test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(Arc::new(ViewState {
                    project: Arc::new(model.project().clone()),
                    waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser.scans.generation = view.browser.scans.generation.saturating_add(1);
                view.browser.directory = directory.0.clone();
                view.browser.listing = Some(DirectoryListing {
                    directory: directory.0.clone(),
                    entries: vec![LibraryEntry {
                        name: "loop.WAV".into(),
                        path: path.clone(),
                        kind: EntryKind::WaveAudio,
                    }],
                    enumerated_entries: 1,
                    truncated: false,
                });
                view.browser.section = BrowserSection::Samples;
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor, track, directory, path)
}

#[gpui::test]
fn browser_search_owns_typing_and_blocks_editor_shortcuts(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES)
                .expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Editor::new(Project::new(48_000).expect("test project"), 4)
                    .expect("test editor")
                    .project()
                    .clone();
                view.state = Some(Arc::new(ViewState {
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    timeline_frames: 0,
                    can_undo: false,
                    can_redo: false,
                    revision: 0,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser
                    .search
                    .update(cx, |input, _| input.focus(window));
                cx.notify();
            });
            editor
        })
        .expect("editor window");

    cx.simulate_keystrokes(handle, "shift-a space cmd-shift-t");

    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "a ");
        assert!(
            view.pending_requests.is_empty(),
            "search focus must suppress add-track"
        );
        assert!(
            view.pending_audio_commands.is_empty(),
            "search focus must suppress transport actions"
        );
        assert_eq!(
            view.state
                .as_ref()
                .map(|state| state.project.tracks().len()),
            Some(0)
        );
    });
}

#[gpui::test]
fn browser_project_open_preserves_dirty_work_until_same_item_is_confirmed(cx: &mut TestAppContext) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES)
                .expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Editor::new(Project::new(48_000).expect("test project"), 4)
                    .expect("test editor")
                    .project()
                    .clone();
                view.state = Some(Arc::new(ViewState {
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    timeline_frames: 0,
                    can_undo: true,
                    can_redo: false,
                    revision: 1,
                    unsaved: true,
                    document_path: None,
                }));
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    let first = std::path::PathBuf::from("/tmp/first.hex");
    let second = std::path::PathBuf::from("/tmp/second.hex");

    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| view.open_browser_project(first.clone(), cx));
    })
    .expect("editor window");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.pending_open_path.as_ref(), Some(&first));
        assert!(view.pending_requests.is_empty());
    });

    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| view.open_browser_project(second.clone(), cx));
    })
    .expect("editor window");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.pending_open_path.as_ref(), Some(&second));
        assert!(view.pending_requests.is_empty());
    });

    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| view.open_browser_project(second.clone(), cx));
    })
    .expect("editor window");
    editor.read_with(cx, |view, _| {
        assert!(view.opening_project);
        assert!(view.pending_open_path.is_none());
    });
}

#[gpui::test]
fn browser_right_edge_drag_resizes_and_release_stops_resizing(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, VisualTestContext, point, px};
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service =
                Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            })
        })
        .expect("editor window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Default panel ends at x=280; grab its four-pixel separator midway down.
    visual.simulate_mouse_down(
        point(px(282.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| {
        assert!(view.browser.resizing, "separator must receive mouse down")
    });
    visual.simulate_mouse_move(
        point(px(310.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| assert_eq!(view.browser.width, 310.0));
    visual.simulate_mouse_move(
        point(px(240.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| assert_eq!(view.browser.width, 240.0));
    visual.simulate_mouse_up(
        point(px(240.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(point(px(300.0), px(200.0)), None, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(!view.browser.resizing);
        assert_eq!(view.browser.width, 240.0);
    });
}

#[gpui::test]
fn browser_audio_drag_floats_outside_lanes_locks_preview_and_imports_exact_candidate(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual
        .debug_bounds(lane_selector)
        .expect("rendered arrangement target lane");
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
    assert!(cx.update(|app| app.has_active_drag()));
    let floating = visual
        .debug_bounds("browser-audio-drag-ghost")
        .expect("outside-lane drag has cursor ghost");
    assert!(f32::from(floating.size.width) > 100.0);
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(!drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
    });

    let outside = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("drag remains active");
        assert!(!drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
    });

    let target = point(
        lane.origin.x + lane.size.width * 0.58,
        lane.origin.y + lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    for _ in 0..8 {
        if editor.read_with(cx, |view, _| {
            view.active_browser_audio_drag
                .as_ref()
                .is_some_and(|drag| drag.preview.get().is_some())
        }) {
            break;
        }
        visual.run_until_parked();
    }
    let expected = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("drag remains active");
        assert!(drag.lane_hovered.get());
        let preview = drag
            .preview
            .get()
            .expect("prepared preview is exact on lane");
        assert_eq!(preview.target_track, track);
        assert_eq!(preview.end_frame - preview.start_frame, 4_800);
        assert_eq!(drag.pending_start.get(), Some((track, preview.start_frame)));
        preview.start_frame
    });
    assert!(visual.debug_bounds("clip-drop-preview").is_some());

    // Moving back out of the lane restores the cursor ghost and clears the target.
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("drag remains active");
        assert!(!drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
    });
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_track,
            Some((path.clone(), track, expected)),
            "release imports the same snapped start shown by the preview"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_audio_drag_release_outside_arrangement_cancels_import(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
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
    let outside = point(source.origin.x + px(24.0), source.origin.y + px(8.0));
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.last_enqueued_import_to_track.is_none());
        assert!(view.active_browser_audio_drag.is_none());
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_audio_drag_rejects_changed_snap_or_viewport(cx: &mut TestAppContext) {
    let (_handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    editor.update(cx, |view, _| {
        let state = view.state.as_ref().expect("project state");
        let viewport = view.visible_viewport(state.timeline_frames);
        view.snap_grid = super::super::musical_time::SnapGrid::Quarter;
        let payload = BrowserAudioDrag {
            project_generation: view.project_generation,
            revision: state.revision,
            browser_generation: view.browser.scan_generation(),
            sample_rate: state.project.sample_rate(),
            path,
            label: "loop.wav".into(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            snap_grid: view.snap_grid,
            metadata: Rc::new(RefCell::new(None)),
            metadata_error: Rc::new(RefCell::new(None)),
            preview: Rc::new(std::cell::Cell::new(None)),
            pending_start: Rc::new(std::cell::Cell::new(None)),
            lane_hovered: Rc::new(std::cell::Cell::new(false)),
            last_pointer: Rc::new(std::cell::Cell::new(None)),
        };
        view.begin_browser_audio_drag(&payload);
        assert!(view.browser_audio_drag_is_current(&payload));
        view.snap_grid = super::super::musical_time::SnapGrid::Off;
        assert!(!view.browser_audio_drag_is_current(&payload));
        view.snap_grid = payload.snap_grid;
        view.viewport = Some(super::super::viewport::Viewport {
            start: 0,
            length: viewport.length / 2,
        });
        assert!(!view.browser_audio_drag_is_current(&payload));
    });
}

#[gpui::test]
fn browser_restores_missing_user_folder_without_touching_real_preferences(cx: &mut TestAppContext) {
    use gpui::VisualTestContext;
    let directory = PreferencesTestDirectory::new();
    let preferences_path = directory.0.join("browser.json");
    let saved =
        hex_preferences::BrowserPlaces::new([directory.0.join("folder-that-is-currently-missing")])
            .expect("saved missing folder");
    hex_preferences::save(&preferences_path, &saved).expect("write isolated preferences");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service =
                Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test(
                    window,
                    cx,
                    service,
                    Err("offline test".into()),
                    preferences_path.clone(),
                )
            })
        })
        .expect("editor window");
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.browser.places.contains(&saved.additional()[0]));
        assert!(view.browser.preferences_writable);
    });
    assert_eq!(
        hex_preferences::load(&preferences_path).expect("preserved saved places"),
        saved
    );
}

#[gpui::test]
fn browser_preferences_coalesce_background_saves_and_preserve_corrupt_files(
    cx: &mut TestAppContext,
) {
    use gpui::VisualTestContext;
    let directory = PreferencesTestDirectory::new();
    let preferences_path = directory.0.join("browser.json");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service =
                Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test(
                    window,
                    cx,
                    service,
                    Err("offline test".into()),
                    preferences_path.clone(),
                )
            })
        })
        .expect("editor window");
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let first = directory.0.join("first-added-folder");
    let second = directory.0.join("second-added-folder");
    editor.update(cx, |view, cx| {
        view.browser.places.push(first.clone());
        view.queue_browser_places_save(cx);
        view.browser.places.push(second.clone());
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    let saved = hex_preferences::load(&preferences_path).expect("coalesced save");
    assert_eq!(saved.additional(), &[first.clone(), second.clone()]);

    let corrupt = b"not valid browser preferences";
    std::fs::write(&preferences_path, corrupt).expect("replace with corrupt fixture");
    editor.update(cx, |view, cx| {
        view.browser.places.push(directory.0.join("third-folder"));
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.browser
                .places_status
                .as_deref()
                .is_some_and(|status| status.contains("could not save"))
        );
    });
    assert_eq!(
        std::fs::read(&preferences_path).expect("corrupt file remains"),
        corrupt
    );
}
