use super::{
    BrowserSection, EditorView,
    drag::{BrowserAudioDrag, BrowserDropDestination},
};
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
    let (handle, editor, tracks, directory, path) =
        browser_drag_fixture_with_tracks(cx, &["target"]);
    (handle, editor, tracks[0], directory, path)
}

fn browser_drag_fixture_with_tracks(
    cx: &mut TestAppContext,
    names: &[&str],
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    Vec<TrackId>,
    PreferencesTestDirectory,
    std::path::PathBuf,
) {
    let directory = PreferencesTestDirectory::new();
    let path = directory.0.join("loop.WAV");
    std::fs::write(&path, mono_pcm_wav(4_800)).expect("wav fixture");
    let mut model = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
    let mut tracks = Vec::with_capacity(names.len());
    for name in names {
        let Created::Track(track) = model
            .execute(Command::AddTrack {
                name: (*name).into(),
            })
            .expect("add track")
        else {
            panic!("track result")
        };
        tracks.push(track);
    }
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
    (handle, editor, tracks, directory, path)
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
        .debug_bounds("browser-audio-floating-preview")
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
        assert_eq!(
            drag.destination.get(),
            Some(BrowserDropDestination::ExistingTrack {
                track,
                start: preview.start_frame,
            })
        );
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
            Some((path.clone(), track, expected, true)),
            "release imports at the previewed start with overwrite semantics"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_between_tracks_shows_only_insertion_line_and_keeps_snapped_start(
    cx: &mut TestAppContext,
) {
    let (handle, editor, tracks, _directory, path) =
        browser_drag_fixture_with_tracks(cx, &["first", "second"]);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let first_selector =
        Box::leak(format!("arrangement-lane-{}", tracks[0].value()).into_boxed_str());
    let second_selector =
        Box::leak(format!("arrangement-lane-{}", tracks[1].value()).into_boxed_str());
    let first = visual
        .debug_bounds(first_selector)
        .expect("first track lane");
    let second = visual
        .debug_bounds(second_selector)
        .expect("second track lane");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("arrangement body");
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
    let target = point(
        first.origin.x + first.size.width * 0.73,
        first.origin.y + first.size.height,
    );
    assert_eq!(first.origin.x, body.origin.x);
    assert_eq!(
        first.size.width,
        body.size.width - px(super::super::tracks::TRACK_HEADER_WIDTH)
    );
    let expected_pointer_start = (((f32::from(target.x) - f32::from(first.origin.x))
        / f32::from(first.size.width))
        * 384_000.0)
        .floor() as u64;
    assert!((f32::from(target.y) - f32::from(second.origin.y)).abs() <= 1.0);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let expected = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track,
            start,
        }) = drag.destination.get()
        else {
            panic!("gap should target a newly inserted track");
        };
        assert!(drag.lane_hovered.get());
        assert_eq!(before_track, Some(tracks[1]));
        assert_eq!(
            start, expected_pointer_start,
            "drop start follows the painted lane's pointer position"
        );
        assert!(drag.insertion_line_y.get().is_some());
        start
    });
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("full-width insertion line");
    assert_eq!(line.size.width, body.size.width);
    assert!(visual.debug_bounds("clip-drop-preview").is_none());
    assert!(
        visual.debug_bounds("append-track-drop-highlight").is_none(),
        "between-track insertion must not highlight the rows below"
    );
    let ghost = visual
        .debug_bounds("browser-audio-floating-preview-hidden")
        .expect("drag ghost node remains mounted but collapsed");
    assert_eq!(ghost.size.width, px(0.0));
    assert_eq!(ghost.size.height, px(0.0));

    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_new_track,
            Some((path.clone(), Some(tracks[1]), expected)),
            "release uses the same stable anchor and snapped start as the preview"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_below_last_track_appends_with_line_and_floating_ghost(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_selector).expect("last track lane");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("arrangement body");
    assert!(
        body.size.height > lane.size.height,
        "Arrangement has blank space below last row: body={:?}, lane={:?}",
        body,
        lane
    );
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
    let arrangement = editor.read_with(cx, |view, _| {
        view.arrangement_bounds.expect("arrangement bounds")
    });
    let target = point(
        lane.origin.x + lane.size.width * 0.62,
        px(arrangement.top + arrangement.height - 8.0),
    );
    assert!(
        editor.read_with(cx, |view, _| view
            .painted_track_insertion_target(f32::from(target.x), f32::from(target.y))
            .is_some()),
        "last painted track should expose the blank area below it: target={target:?}, body={body:?}, lane={lane:?}, bounds={:?}/{}, lane_epoch={:?}/{}, lanes={:?}",
        editor.read_with(cx, |view, _| view.arrangement_bounds),
        editor.read_with(cx, |view, _| view.arrangement_bounds_epoch),
        editor.read_with(cx, |view, _| view.drag_lane_epochs.get(&track).copied()),
        editor.read_with(cx, |view, _| view.drag_layout_generation),
        editor.read_with(cx, |view, _| view.drag_lanes.clone()),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let start = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(!drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track: None,
            start,
        }) = drag.destination.get()
        else {
            panic!("blank area under the final track should append");
        };
        assert!(view.browser_audio_drag_is_current(drag));
        assert!(!drag.lane_hovered.get());
        start
    });
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("append insertion line");
    assert_eq!(line.size.width, body.size.width);
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(matches!(
            drag.destination.get(),
            Some(BrowserDropDestination::NewTrack {
                before_track: None,
                ..
            })
        ));
        assert!(view.browser_audio_drag_is_current(drag));
        assert!(
            view.active_browser_audio_drag
                .as_ref()
                .filter(|drag| view.browser_audio_drag_is_current(drag))
                .is_some_and(|drag| matches!(
                    drag.destination.get(),
                    Some(BrowserDropDestination::NewTrack {
                        before_track: None,
                        ..
                    })
                ))
        );
        assert!(
            view.state()
                .is_some_and(|state| !state.project.tracks().is_empty())
        );
        let last = view.state().unwrap().project.tracks().last().unwrap().id();
        assert_eq!(
            view.drag_lane_epochs.get(&last),
            Some(&view.drag_layout_generation)
        );
        let lane = view.drag_lanes[&last];
        let bounds = view.arrangement_bounds.unwrap();
        assert!(
            lane.top + lane.height < bounds.top + bounds.height,
            "lane={lane:?} body={bounds:?}"
        );
        assert_eq!(drag.insertion_line_y.get(), Some(lane.top + lane.height));
    });
    let highlight = visual
        .debug_bounds("append-track-drop-highlight")
        .expect("blank append area is highlighted");
    assert_eq!(highlight.origin.x, body.origin.x);
    assert_eq!(highlight.size.width, body.size.width);
    assert!((highlight.origin.y - (lane.origin.y + lane.size.height)).abs() <= px(1.0));
    assert!(
        (highlight.origin.y + highlight.size.height - (body.origin.y + body.size.height)).abs()
            <= px(1.0)
    );
    let ghost = visual
        .debug_bounds("browser-audio-floating-preview")
        .expect("blank-space append keeps cursor ghost");
    assert!(ghost.size.width > px(100.0));
    let body_after = visual
        .debug_bounds("arrangement-content")
        .expect("Arrangement body remains mounted");
    assert_eq!(
        body_after.origin, body.origin,
        "drag must not shift the workspace"
    );
    assert_eq!(
        body_after.size, body.size,
        "drag must not resize the workspace"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_new_track,
            Some((path.clone(), None, start))
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_in_empty_arrangement_creates_bottom_track(cx: &mut TestAppContext) {
    let (handle, editor, tracks, _directory, path) = browser_drag_fixture_with_tracks(cx, &[]);
    assert!(tracks.is_empty());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("empty arrangement body");
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
    let target = point(
        body.origin.x + body.size.width * 0.4,
        body.origin.y + body.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let start = editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(!drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track: None,
            start,
        }) = drag.destination.get()
        else {
            panic!("empty Arrangement should target a bottom track");
        };
        start
    });
    assert!(
        visual.debug_bounds("track-insert-preview").is_none(),
        "empty Arrangement feedback highlights the section without a cursor-following line"
    );
    let highlight = visual
        .debug_bounds("empty-track-drop-highlight")
        .expect("empty Arrangement section is highlighted");
    assert_eq!(highlight.origin, body.origin);
    assert_eq!(highlight.size, body.size);
    let ghost = visual
        .debug_bounds("browser-audio-floating-preview")
        .expect("empty Arrangement keeps cursor ghost while showing insertion line");
    assert!(ghost.size.width > px(100.0));
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_import_to_new_track,
            Some((path.clone(), None, start))
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_gap_target_uses_only_currently_painted_rows_after_scroll(
    cx: &mut TestAppContext,
) {
    let names = (0..24)
        .map(|index| format!("track {index}"))
        .collect::<Vec<_>>();
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let (handle, editor, tracks, _directory, _) = browser_drag_fixture_with_tracks(cx, &name_refs);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(px(500.0), px(250.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-1100.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let (first_track, second_track, target) = editor.update(cx, |view, _| {
        let generation = view.drag_layout_generation;
        let tracks = &view.state.as_ref().expect("project").project.tracks();
        let pair = tracks.windows(2).find(|pair| {
            view.drag_lane_epochs.get(&pair[0].id()) == Some(&generation)
                && view.drag_lane_epochs.get(&pair[1].id()) == Some(&generation)
        });
        let pair = pair.expect("at least two adjacent rows are painted");
        let first_lane = view.drag_lanes[&pair[0].id()];
        let second_lane = view.drag_lanes[&pair[1].id()];
        let pointer_x = first_lane.left + first_lane.width * 0.62;
        let pointer_y = first_lane.top + first_lane.height;
        assert!(
            view.drag_lane_epochs[&tracks[0].id()] < generation,
            "first row should be offscreen after scroll; epochs={:?}, generation={generation}, lanes={:?}, arrangement={:?}",
            view.drag_lane_epochs,
            view.drag_lanes,
            view.arrangement_bounds
        );
        view.drag_lanes.insert(tracks[0].id(), second_lane);
        let insertion = view
            .painted_track_insertion_target(pointer_x, pointer_y)
            .expect("visible gap remains a valid insertion target");
        assert_eq!(insertion.0, Some(pair[1].id()));
        (
            pair[0].id(),
            pair[1].id(),
            point(px(pointer_x), px(pointer_y)),
        )
    });
    assert_ne!(first_track, tracks[0]);

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
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_browser_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(matches!(
            drag.destination.get(),
            Some(BrowserDropDestination::NewTrack {
                before_track: Some(track),
                ..
            }) if track == second_track
        ));
        assert!(drag.insertion_line_y.get().is_some());
    });
    assert!(visual.debug_bounds("track-insert-preview").is_some());
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
            destination: Rc::new(std::cell::Cell::new(None)),
            insertion_line_y: Rc::new(std::cell::Cell::new(None)),
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
