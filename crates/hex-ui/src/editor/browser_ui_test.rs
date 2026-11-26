use super::{
    BrowserSection, EditorView,
    drag::{BrowserAudioDrag, BrowserDropDestination, ClipDrag, ClipDropDestination, ClipPosition},
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
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                // Fixed geometry for pointer/drop tests; the normal navigation
                // allowance is tested by the arranger navigation regressions.
                view.viewport = Some(super::super::viewport::Viewport::fit(384_000));
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(model.project().clone()),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser.scans.generation = view.browser.scans.generation.saturating_add(1);
                view.browser.places = vec![directory.0.clone()];
                view.browser.directory = directory.0.clone();
                // This fixture installs a validated snapshot, replacing startup work.
                view.browser.loading = false;
                view.browser.error = None;
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
                view.browser.samples.replace(
                    hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
                );
                view.browser.section = BrowserSection::Place(directory.0.clone());
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor, tracks, directory, path)
}

fn windowed_audition_matches_resident_pcm(cx: &mut TestAppContext, rate: u32, fraction: f64) {
    use hex_audio::sample_preview::Phase;
    use std::sync::atomic::AtomicBool;

    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(rate, false).unwrap();
    let (handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    std::fs::write(&path, mono_pcm_wav(20_000)).unwrap();
    let expected = hex_session::service::PreviewSources::default()
        .prepare_playback(&path, rate, &AtomicBool::new(false))
        .unwrap();
    let start = super::audition_geometry::frame_at_fraction(expected.frames().len(), fraction);
    let mut visual = VisualTestContext::from_window(handle, cx);
    // Finish startup preference/scan callbacks before installing the test output.
    visual.run_until_parked();
    let client = output.take_client().unwrap();
    let replay_path = path.clone();
    editor.update(&mut visual, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: rate,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audio_pub.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.browser.section = BrowserSection::Samples;
        view.browser.loading = false;
        view.browser.error = None;
        view.play_browser_sample_from(path, fraction, cx);
        assert!(
            view.browser.playback_in_flight.is_some(),
            "request not started: {} {:?}",
            view.status,
            view.error
        );
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let first = loop {
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        backend.render(&mut frame).unwrap();
        let playing = editor.read_with(&visual, |view, _| {
            view.audio
                .as_ref()
                .unwrap()
                .control
                .preview_snapshot()
                .unwrap()
                .phase
                == Phase::Playing
        });
        if playing {
            break frame[0];
        }
        if std::time::Instant::now() >= deadline {
            let detail = editor.read_with(&visual, |view, _| {
                format!(
                    "status={} error={:?} in_flight={} snapshot={:?}",
                    view.status,
                    view.error,
                    view.browser.playback_in_flight.is_some(),
                    view.audio
                        .as_ref()
                        .and_then(|audio| audio.control.preview_snapshot())
                )
            });
            editor.update(&mut visual, |view, _| {
                view.cancel_browser_sample_playback();
                view.audio.take().unwrap().control.close();
            });
            backend.close();
            drop(output);
            panic!("first window did not play: {detail}");
        }
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    };
    assert_eq!(first, expected.frames()[start]);
    assert!(
        editor.read_with(&visual, |view, _| view.browser.playback_in_flight.is_some()),
        "continuation remains active after the first window starts"
    );
    let mut position = start + 1;
    while position < expected.frames().len() {
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
        visual.run_until_parked();
        let count = (expected.frames().len() - position).min(128);
        let mut rendered = vec![[0.0; 2]; count];
        backend.render(&mut rendered).unwrap();
        assert_eq!(
            rendered,
            expected.frames()[position..position + count],
            "gap at {position}"
        );
        position += count;
    }
    let snapshot = editor.read_with(&visual, |view, _| {
        view.audio
            .as_ref()
            .unwrap()
            .control
            .preview_snapshot()
            .unwrap()
    });
    assert_eq!(
        (snapshot.phase, snapshot.frame),
        (Phase::Finished, expected.frames().len() as u64)
    );
    editor.update(&mut visual, |view, cx| {
        view.play_browser_sample_from(replay_path, fraction, cx);
    });
    let replay_deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        backend.render(&mut frame).unwrap();
        let playing = editor.read_with(&visual, |view, _| {
            view.audio
                .as_ref()
                .unwrap()
                .control
                .preview_snapshot()
                .unwrap()
                .phase
                == Phase::Playing
        });
        if playing {
            assert_eq!(frame[0], expected.frames()[start]);
            break;
        }
        assert!(
            std::time::Instant::now() < replay_deadline,
            "warm replay did not start"
        );
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    editor.update(&mut visual, |view, _| {
        view.cancel_browser_sample_playback();
        view.audio.take().unwrap().control.close();
    });
    backend.close();
    drop(output);
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
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).unwrap();
            window.replace_root(cx, |window, cx| {
                EditorView::new_with_browser_paths(
                    window,
                    cx,
                    service,
                    Err("offline icon test".into()),
                    (
                        config.to_owned(),
                        recent.to_owned(),
                        directory.0.join("icon-theme.json"),
                        directory.0.join("project-directory.json"),
                    ),
                    None,
                    crate::editor::test_environment::Inputs {
                        home: Some(home),
                        ..Default::default()
                    },
                )
            })
        })
        .unwrap();
    (handle, editor)
}

#[path = "browser_search_selection_ui_test.rs"]
mod search_selection;

#[path = "browser_focus_ui_test.rs"]
mod explicit_focus;

#[path = "browser_insert_ui_test.rs"]
mod insertion;

#[path = "browser_history_ui_test.rs"]
mod browser_history_ui_test;

#[path = "browser_builtin_source_ui_test.rs"]
mod builtin_sources;

#[path = "browser_resize_feedback_ui_test.rs"]
mod resize_feedback;

#[path = "browser_tag_editor_geometry_ui_test.rs"]
mod tag_editor_geometry;

#[path = "editor_test_environment_ui_test.rs"]
mod test_environment;

#[path = "browser_picker_compact_ui_test.rs"]
mod compact_picker;

#[path = "browser_arrow_fixture_ui_test.rs"]
mod arrow_fixture;

use arrow_fixture::arrow_row_fixture;

fn unhighlighted_arrow_action(cx: &mut TestAppContext, sample: bool, escape: bool, left: bool) {
    let (handle, editor, _directory, samples) = arrow_row_fixture(cx, sample);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let category = if sample {
        "browser-place-0"
    } else {
        "browser-section-projects"
    };
    let point = visual.debug_bounds(category).unwrap().center();
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, cx| {
        let entries = v.browser_visible_entries(cx);
        assert!(!entries.is_empty(), "actual async entries available");
        assert_eq!(
            entries[0].path,
            if sample {
                samples.join("loop.wav")
            } else {
                samples.join("a")
            }
        );
    });
    if escape || left {
        let row = visual.debug_bounds("browser-entry-0").unwrap().center();
        visual.simulate_click(row, Modifiers::default());
        editor.read_with(&visual, |v, _| assert!(v.browser.results_focused));
        if left {
            visual.simulate_keystrokes("right");
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(
                    (v.browser.tree.disclosure(&samples.join("a"))
                        == super::tree::Disclosure::Expanded)
                )
            });
        }
        visual.simulate_keystrokes("escape");
    }
    visual.update(|window, cx| {
        editor.read_with(cx, |v, cx| {
            assert!(!v.browser.results_focused);
            assert!(v.browser.results_focus.is_focused(window));
            assert_eq!(v.browser_key_context(window, cx), "hex_browser");
        })
    });
    let canonical = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    let before = editor.read_with(&visual, |v, _| {
        (
            v.browser.playback_generation,
            v.browser.tree.disclosure(&samples.join("a")),
            v.browser.tree.requests_started,
        )
    });
    visual.simulate_keystrokes(if left { "left" } else { "right" });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let after = (v.browser.playback_generation, v.browser.tree.disclosure(&samples.join("a")), v.browser.tree.requests_started);
        eprintln!("ACTUAL_UNHIGHLIGHTED_ROW sample={sample} escape={escape} left={left} before={before:?} after={after:?}");
        assert_eq!(after, before, "unhighlighted navigation must not dispatch a row action");
        assert!(v.pending_requests.is_empty() && v.audio_pub.pending_audio_commands.is_empty());
        let state = v.state.as_ref().unwrap();
        assert_eq!(state.project, canonical.project);
        assert_eq!((state.revision, state.can_undo, state.can_redo, state.unsaved),
                   (canonical.revision, canonical.can_undo, canonical.can_redo, canonical.unsaved));
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[path = "browser_divider_paint_ui_test.rs"]
mod divider_paint;

#[path = "browser_instrument_enter_ui_test.rs"]
mod instrument_enter;

#[path = "browser_sampler_drop_ui_test.rs"]
mod sampler_drop;

#[path = "browser_plugin_ui_test.rs"]
mod plugin_browser;

#[path = "browser_reveal_ui_test.rs"]
mod reveal;

#[path = "browser_random_ui_test.rs"]
mod random_ui_tests;

#[path = "browser_scrollbar_ui_test.rs"]
mod scrollbar_ui_tests;

#[gpui::test]
fn browser_right_edge_drag_resizes_and_release_stops_resizing(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, VisualTestContext, point, px};
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            })
        })
        .expect("editor window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Grab the actual separator; the browser may use a compact or expanded width.
    let separator = visual
        .debug_bounds("browser-resize-handle")
        .expect("browser separator");
    visual.simulate_mouse_down(separator.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.browser.resizing, "separator must receive mouse down")
    });
    visual.simulate_mouse_move(
        point(px(310.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| assert_eq!(view.browser.width, 306.0));
    visual.simulate_mouse_move(
        point(px(240.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| assert_eq!(view.browser.width, 236.0));
    visual.simulate_mouse_up(
        point(px(240.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(point(px(300.0), px(200.0)), None, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(!view.browser.resizing);
        assert_eq!(view.browser.width, 236.0);
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
    crate::editor::clip_boundary::tests::assert_open_upper_right(
        &mut visual,
        "browser-audio-floating-preview",
        "browser-audio-floating-preview-trailing",
        Some("browser-drag-name"),
    );
    let title = visual.debug_bounds("browser-drag-name").unwrap();
    assert_eq!(
        title.size.height,
        px(crate::editor::clip_name::CLIP_NAME_HEIGHT)
    );

    editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
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
            .browser
            .active_audio_drag
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
            view.browser
                .active_audio_drag
                .as_ref()
                .is_some_and(|drag| drag.preview.get().is_some())
        }) {
            break;
        }
        visual.run_until_parked();
    }
    let expected = editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
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
    crate::editor::clip_boundary::tests::assert_open_upper_right(
        &mut visual,
        "clip-drop-preview",
        "clip-drop-preview-trailing",
        None,
    );

    // Moving back out of the lane restores the cursor ghost and clears the target.
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
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
            view.test_probes.last_enqueued_import_to_track,
            Some((path.clone(), track, expected, true)),
            "release imports at the previewed start with overwrite semantics"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn arrangement_wheel_zoom_is_pointer_anchored_and_does_not_escape_its_timeline(
    cx: &mut TestAppContext,
) {
    let names = (0..24)
        .map(|index| format!("track {index}"))
        .collect::<Vec<_>>();
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let (handle, editor, _, _directory, _) = browser_drag_fixture_with_tracks(cx, &name_refs);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let ruler = visual
        .debug_bounds("timeline-ruler")
        .expect("painted arrangement ruler");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("painted arrangement body");
    let grid = visual
        .debug_bounds("arrangement-grid")
        .expect("painted Arrangement grid");
    let first_grid_line = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("grid starts at the visible timeline origin");
    assert_eq!(grid.origin.x, body.origin.x);
    assert_eq!(
        grid.size.width + editor.read_with(cx, |view, _| px(view.track_header_width())),
        body.size.width
    );
    assert_eq!(first_grid_line.origin.x, ruler.origin.x);
    let pointer = point(
        ruler.origin.x + ruler.size.width * 0.37,
        ruler.origin.y + ruler.size.height / 2.0,
    );
    let before = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let after = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    let anchor = super::super::seek::pointer_to_frame(
        f32::from(pointer.x),
        f32::from(ruler.origin.x),
        f32::from(ruler.size.width),
        before,
    )
    .expect("anchor inside ruler");
    assert_eq!(after, before.zoom_at(384_000, anchor, true));
    let old_fraction = (anchor - before.start) as f64 / before.length as f64;
    let new_fraction = (anchor - after.start) as f64 / after.length as f64;
    assert!((old_fraction - new_fraction).abs() < 0.002);

    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        direction_inverted_from_device: true,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let natural_down = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        natural_down.length, before.length,
        "physical wheel down zooms out"
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        direction_inverted_from_device: true,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let natural_up = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        natural_up, after,
        "physical wheel up zooms in with inversion enabled"
    );

    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        delta: gpui::ScrollDelta::Lines(point(0.0, -1.0)),
        ..Default::default()
    });
    visual.run_until_parked();
    let line_out = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        line_out.length, 384_000,
        "one physical wheel-down line zooms out once"
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        delta: gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        ..Default::default()
    });
    visual.run_until_parked();
    let line_in = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        line_in.length, after.length,
        "one reverse wheel line zooms in once"
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: pointer,
        delta: gpui::ScrollDelta::Pixels(point(px(160.0), px(0.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let horizontal_only = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(
        horizontal_only, line_in,
        "horizontal-only wheel input does not zoom"
    );

    let (list_before, scroll_before) = editor.read_with(cx, |view, _| {
        (
            view.track_scroll.logical_scroll_top_index(),
            view.track_scroll.offset(),
        )
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(body.origin.x + px(100.0), body.origin.y + px(80.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-12.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let (list_after, scroll_after) = editor.read_with(cx, |view, _| {
        (
            view.track_scroll.logical_scroll_top_index(),
            view.track_scroll.offset(),
        )
    });
    assert_eq!(
        list_after, list_before,
        "small timeline wheel does not scroll track rows"
    );
    assert_eq!(
        scroll_after, scroll_before,
        "partial wheel deltas are consumed"
    );

    editor.update(cx, |view, _| {
        view.viewport = Some(super::super::viewport::Viewport {
            start: 0,
            length: 1,
        });
    });
    visual.run_until_parked();
    let at_zoom_limit = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.track_scroll.offset(),
        )
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(body.origin.x + px(100.0), body.origin.y + px(80.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let after_limit_wheel = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.track_scroll.offset(),
        )
    });
    assert_eq!(after_limit_wheel, at_zoom_limit);

    let browser_handle = visual
        .debug_bounds("browser-resize-handle")
        .expect("visible browser");
    let browser_pointer = point(
        browser_handle.origin.x - px(60.0),
        browser_handle.origin.y + browser_handle.size.height / 2.0,
    );
    let before_browser_wheel = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: browser_pointer,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-160.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let after_browser_wheel = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    assert_eq!(after_browser_wheel, before_browser_wheel);
}

#[gpui::test]
fn samples_catalog_combines_roots_and_active_place_indexed_search(cx: &mut TestAppContext) {
    let (handle, editor, _, first, original) = browser_drag_fixture(cx);
    let second = PreferencesTestDirectory::new();
    let nested = second.0.join("drums");
    std::fs::create_dir_all(&nested).unwrap();
    let other = nested.join("loop.WAV");
    std::fs::write(&other, mono_pcm_wav(16)).unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.browser.places = vec![first.0.clone(), second.0.clone()];
        view.browser.section = BrowserSection::Samples;
        view.browser.search.update(cx, |input, cx| {
            input.set_text("loop", cx);
            cx.notify();
        });
        view.refresh_sample_catalog(cx);
        // Rapid refreshes must coalesce; the latest roots must win.
        view.refresh_sample_catalog(cx);
    });
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|entry| entry.kind == EntryKind::WaveAudio)
        );
        assert!(entries.iter().any(|entry| entry.path == original));
        assert!(entries.iter().any(|entry| entry.path == other));
        view.select_browser_source(BrowserSection::Place(second.0.clone()), cx);
        view.browser.search.update(cx, |input, cx| {
            input.set_text("DRUMS loop", cx);
            cx.notify();
        });
        assert!(view.browser_visible_entries(cx).is_empty());
        assert!(view.browser.loading);
        assert_eq!(view.browser.directory, second.0);
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let filtered = view.browser_visible_entries(cx);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, other);
    });
    std::fs::remove_file(&other).unwrap();
    editor.update(cx, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert!(view.browser_visible_entries(cx).is_empty());
        assert_eq!(view.browser.samples.catalog.entries().len(), 1);
        let database = view
            .browser
            .preferences_path
            .as_ref()
            .unwrap()
            .with_extension("samples-v1.db");
        assert_eq!(
            hex_library::catalog::SampleCatalog::load(&database).unwrap(),
            view.browser.samples.catalog
        );
    });
}

#[gpui::test]
fn all_search_unions_samples_folder_and_devices_and_preserves_identity_on_refresh(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, indexed, _) = browser_drag_fixture(cx);
    let nested = indexed.0.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let indexed_wav = nested.join("mix-index.wav");
    std::fs::write(&indexed_wav, mono_pcm_wav(16)).unwrap();
    let outside = PreferencesTestDirectory::new();
    let outside_wav = outside.0.join("mix-outside.wav");
    std::fs::write(&outside_wav, mono_pcm_wav(16)).unwrap();
    let project = outside.0.join("mix.hex");
    std::fs::write(&project, []).unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.browser.directory = outside.0.clone();
        view.browser.listing = Some(hex_library::scan_directory(&outside.0).unwrap());
        view.browser.samples.invalidate_listing();
        // Global search must ignore an unrelated previous Place source.
        view.browser.section = BrowserSection::Place(indexed.0.join("unmatched-source"));
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&indexed.0)).catalog,
            cx,
        );
        cx.notify();
    });
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "mix");
    visual.run_until_parked();
    let selected_device = editor.update(cx, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::All);
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 3);
        for path in [&indexed_wav, &outside_wav, &project] {
            assert_eq!(
                entries.iter().filter(|entry| &entry.path == path).count(),
                1
            );
        }
        assert!(Arc::ptr_eq(&entries, &view.browser_visible_entries(cx)));
        let devices = view.browser_visible_devices(cx);
        let device_index = devices
            .iter()
            .position(|(_, entry)| entry.name.to_lowercase() == "mix")
            .unwrap();
        let identity = (devices[device_index].0, devices[device_index].1.type_id);
        view.browser.selected_index = view
            .browser_history_index(&super::BrowserItemKey::Device(identity.0, identity.1), cx)
            .unwrap();
        view.browser.results_focused = true;
        cx.notify();
        identity
    });
    std::fs::write(nested.join("mix-second.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(cx, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx).len(), 4);
        assert!(matches!(view.browser_selected_item(cx), Some(super::BrowserItemKey::Device(class, id)) if (class, id) == selected_device));
        assert_eq!(view.browser_selected_label(cx).to_lowercase(), "mix");
    });
    let unchanged = editor.read_with(cx, |view, cx| view.browser_visible_entries(cx));
    editor.update(cx, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert!(Arc::ptr_eq(&unchanged, &view.browser_visible_entries(cx)))
    });
    std::fs::write(outside.0.join("mix-third.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(cx, |view, cx| {
        view.request_browser_scan(outside.0.clone(), cx)
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx).len(), 5);
        assert!(matches!(view.browser_selected_item(cx), Some(super::BrowserItemKey::Device(class, id)) if (class, id) == selected_device));
    });
    cx.simulate_keystrokes(handle, "enter");
    editor.read_with(cx, |view, _| {
        assert!(view.status.contains("drag a browser device"));
        assert!(view.test_probes.last_enqueued_import_to_track.is_none());
    });
    editor.update(cx, |view, cx| view.request_browser_scan(nested.clone(), cx));
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(
            entries.len(),
            2,
            "overlapping folder and catalog WAV paths appear once"
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.kind == EntryKind::WaveAudio)
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.path == indexed_wav)
                .count(),
            1
        );
    });
}

#[gpui::test]
fn browser_audition_row_observes_pcm_completion_and_stop_without_project_transport(
    cx: &mut TestAppContext,
) {
    use hex_audio::sample_preview::Phase;
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (mut control, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
    let generation = control.sample_preview.request().unwrap();
    control
        .sample_preview
        .publish(
            generation,
            hex_engine::Audio::new(48_000, vec![[0.25, -0.25]; 3]).unwrap(),
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    editor.update(&mut visual, |view, cx| {
        assert!(view.accept_browser_sample_phase(control.sample_preview.snapshot().phase));
        assert_eq!(view.browser.playback_phase, Phase::Playing);
        assert_eq!(view.status.as_str(), "sample preview playing");
        cx.notify();
    });
    visual.run_until_parked();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    assert_eq!(control.sample_preview.snapshot().phase, Phase::Finished);
    editor.update(&mut visual, |view, cx| {
        assert!(view.accept_browser_sample_phase(control.sample_preview.snapshot().phase));
        assert_eq!(view.status.as_str(), "sample preview finished");
        assert!(
            !view.accept_browser_sample_phase(Phase::Finished),
            "no idle timer repaint"
        );
        cx.notify();
    });
    // A later audition has its own real callback start observation.
    let next = control.sample_preview.request().unwrap();
    control
        .sample_preview
        .publish(
            next,
            hex_engine::Audio::new(48_000, vec![[0.2; 2]; 8]).unwrap(),
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2]).unwrap();
    editor.update(&mut visual, |view, cx| {
        assert!(view.accept_browser_sample_phase(control.sample_preview.snapshot().phase));
        cx.notify();
    });
    visual.run_until_parked();
    let stop = visual.debug_bounds("browser-audition-transport").unwrap();
    visual.simulate_mouse_down(stop.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(stop.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_phase, Phase::Idle);
        assert_eq!(view.status.as_str(), "sample preview stopped");
        assert_eq!(view.browser.playback_generation, 1);
        assert!(view.browser.playback_output.is_none());
        assert!(view.browser.playback_pending.is_none());
        assert!(view.audio_pub.pending_audio_commands.is_empty());
    });
    editor.update(&mut visual, |view, cx| {
        assert!(
            !view.poll_browser_sample_playback(),
            "Stop removes the observed output token"
        );
        assert!(view.accept_browser_sample_phase(Phase::Unavailable));
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("sample preview"))
        );
        cx.notify();
    });
}

#[gpui::test]
fn browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, directory, path) = browser_drag_fixture(cx);
    for i in 0..40 {
        std::fs::write(
            directory.0.join(format!("sample-{i:02}.wav")),
            mono_pcm_wav(32),
        )
        .unwrap();
    }
    let metadata = Rc::new(
        hex_session::service::PreviewSources::default()
            .prepare_waveform(&path, 48_000, &std::sync::atomic::AtomicBool::new(false))
            .unwrap(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.listing = Some(hex_library::scan_directory(&directory.0).unwrap());
        view.browser.samples.invalidate_listing();
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
            cx,
        );
        view.browser.results_focused = true;
        view.browser.selected_index = 0;
        let slot = view.browser_preview_cell(path.clone(), 48_000);
        *slot.metadata.borrow_mut() = Some(metadata.clone());
        // Uniform-list rendering may compose more rows than cache capacity.
        let entries = view.browser_visible_entries(cx);
        let cached_before = view.browser.preview_cache.len();
        for (index, entry) in entries.iter().cloned().enumerate() {
            let _ = view.browser_row(index, entry, cx);
        }
        assert_eq!(
            view.browser.preview_cache.len(),
            cached_before,
            "rendering visible rows cannot mutate the preparation cache"
        );
        let cached = view.browser_preview_cell(path.clone(), 48_000);
        assert!(
            cached
                .metadata
                .borrow()
                .as_ref()
                .is_some_and(|value| Rc::ptr_eq(value, &metadata)),
            "rendering other rows must not evict/reprepare the selected waveform"
        );
        cx.notify();
    });
    visual.run_until_parked();
    let lane = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let bounds = visual.debug_bounds(lane).unwrap();
    let color = visual.rendered_background(lane);
    let canonical = editor.read_with(&visual, |view, _| view.state.clone().unwrap());
    for index in 0..3 {
        let row = visual
            .debug_bounds(Box::leak(format!("browser-entry-{index}").into_boxed_str()))
            .unwrap();
        visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let waveform = visual.debug_bounds("browser-audition-waveform").unwrap();
        let slot = editor.read_with(&visual, |view, cx| {
            let selected = view.browser_visible_entries(cx)[index].path.clone();
            view.browser.preview_cache[&super::PreviewKey {
                path: selected,
                sample_rate: 48_000,
            }]
                .clone()
        });
        let preparations = editor.read_with(&visual, |view, _| view.browser.preview_preparations);
        for _ in 0..4 {
            visual.simulate_mouse_move(waveform.center(), None, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert!(slot.metadata.borrow().is_some());
                assert_eq!(
                    view.browser.preview_preparations, preparations,
                    "hover cannot reprepare"
                );
                let key = view.browser.preview_selected.as_ref().unwrap();
                assert!(Rc::ptr_eq(
                    &view.browser.preview_cache[key].metadata,
                    &slot.metadata
                ));
                assert!(view.browser.preview_in_flight.is_none());
                assert!(view.browser.preview_pending.is_none());
                let source = super::super::overview_waveform::Source::Browser(Arc::clone(
                    &slot.metadata.borrow().as_ref().unwrap().waveform,
                ));
                let expected = super::super::waveform::waveform_column_count(
                    f32::from(waveform.size.width),
                    2.0,
                );
                assert!(expected > 48, "browser paint must use detailed columns");
                assert_eq!(
                    view.timeline_caches
                        .overview_waveform_cache
                        .borrow()
                        .prepared_columns(&source, waveform, 2.0),
                    Some(expected),
                    "actual browser canvas must populate its geometry cache at rendered bounds"
                );
                assert_eq!(view.browser.playback_generation, 0, "hover cannot audition");
                assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &canonical));
                assert!(view.audio_pub.pending_audio_commands.is_empty());
            });
            assert_eq!(visual.debug_bounds(lane).unwrap(), bounds);
            assert_eq!(visual.rendered_background(lane), color);
            assert_eq!(
                visual.debug_bounds("browser-audition-waveform").unwrap(),
                waveform
            );
        }
    }
    // Actual GPUI TestWindow is fixed at 2x; 1x geometry has a separate oracle,
    // while native 1x visual acceptance remains a distinct check.
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(scale, 2.0);
    for width in [960.0, 1280.0] {
        visual.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.0)));
        visual.run_until_parked();
        let waveform = visual.debug_bounds("browser-audition-waveform").unwrap();
        editor.read_with(&visual, |view, _| {
            let key = view.browser.preview_selected.as_ref().unwrap();
            let slot = &view.browser.preview_cache[key];
            let metadata = slot.metadata.borrow();
            let source = super::super::overview_waveform::Source::Browser(Arc::clone(
                &metadata.as_ref().unwrap().waveform,
            ));
            let expected = super::super::waveform::waveform_column_count(
                f32::from(waveform.size.width),
                scale,
            );
            assert_eq!(
                view.timeline_caches
                    .overview_waveform_cache
                    .borrow()
                    .prepared_columns(&source, waveform, scale),
                Some(expected),
                "browser resize must repaint the detailed waveform at its actual new bounds"
            );
            assert_eq!(
                view.browser.playback_generation, 0,
                "resize cannot audition"
            );
        });
    }
}

#[gpui::test]
fn browser_waveform_click_is_manual_positioned_audition_and_retains_focus(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.results_focused = true;
        cx.notify();
    });
    visual.run_until_parked();
    let bounds = visual.debug_bounds("browser-audition-waveform").unwrap();
    let mut generation = 0;
    for fraction in [0.0, 0.5, 0.9, 0.25] {
        let position = point(
            bounds.left() + bounds.size.width * fraction,
            bounds.center().y,
        );
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                view.browser.search.read(cx).focus(window);
            })
        });
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        generation += 1;
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                assert!(
                    !view
                        .browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window)
                );
                assert_eq!(view.browser_key_context(window, cx), "hex_browser");
            })
        });
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.browser.playback_generation, generation);
            assert!((view.browser.playback_fraction - fraction).abs() < 0.02);
            assert_eq!(view.browser.playback_path.as_ref(), Some(&path));
            assert_eq!(
                view.browser.playback_phase,
                hex_audio::sample_preview::Phase::Unavailable
            );
            assert!(view.browser.results_focused);
            assert!(!view.browser.auto_audition);
            assert!(view.audio_pub.pending_audio_commands.is_empty());
            assert!(view.pending_requests.is_empty());
        });
    }
    visual.simulate_mouse_move(bounds.center(), None, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.playback_generation, generation)
    });
}

#[gpui::test]
fn browser_source_transition_discards_late_scan_and_keeps_query_and_keyboard_scope(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.search.update(cx, |input, cx| {
            input.set_text("loop", cx);
            cx.notify();
        });
        view.browser.scans = Default::default();
        let old = view.browser.scans.request(directory.0.clone()).unwrap();
        view.select_browser_source(BrowserSection::Collection(0), cx);
        view.browser_scan_finished(
            old,
            Ok(hex_library::scan_directory(&directory.0).unwrap()),
            cx,
        );
        assert!(view.browser_visible_entries(cx).is_empty());
        assert_eq!(view.browser.section, BrowserSection::Collection(0));
        view.select_browser_source(BrowserSection::Samples, cx);
        assert_eq!(view.browser.search.read(cx).text(), "loop");
        cx.notify();
    });
    visual.run_until_parked();
    let place = visual.debug_bounds("browser-place-0").unwrap();
    visual.simulate_mouse_down(place.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(place.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("down");
    editor.read_with(&visual, |view, cx| {
        assert!(view.browser.results_focused);
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
        assert_eq!(view.browser.search.read(cx).text(), "loop");
        let a = view.browser_visible_entries(cx);
        let b = view.browser_visible_entries(cx);
        assert!(
            Arc::ptr_eq(&a, &b),
            "unchanged source/query must retain rows"
        );
    });
    visual.simulate_keystrokes("cmd-f");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::All);
        assert_eq!(view.browser.search.read(cx).text(), "loop");
    });
}

#[gpui::test]
fn browser_folder_expansion_preserves_its_position_when_an_earlier_branch_closes(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let previous = directory.0.join("a");
    let target = directory.0.join("c");
    for folder in [&previous, &target] {
        std::fs::create_dir(folder).unwrap();
        for index in 0..80 {
            std::fs::write(folder.join(format!("{index:02}.hex")), []).unwrap();
        }
    }
    for index in 0..12 {
        std::fs::create_dir(directory.0.join(format!("b{index:02}"))).unwrap();
    }
    for index in 0..60 {
        std::fs::write(directory.0.join(format!("tail-{index:02}.hex")), []).unwrap();
    }
    editor.update(cx, |view, cx| {
        view.browser.auto_audition = false;
        view.request_browser_scan(directory.0.clone(), cx);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let disclosure = |index| {
        let selector = format!("browser-disclosure-{index}");
        Box::leak(selector.into_boxed_str()) as &'static str
    };
    let at = visual.debug_bounds(disclosure(0)).unwrap().center();
    visual.simulate_click(at, Modifiers::default());
    visual.run_until_parked();
    let target_index = |visual: &VisualTestContext| {
        editor.read_with(visual, |view, cx| {
            view.browser_visible_entries(cx)
                .iter()
                .position(|entry| entry.path == target)
                .unwrap()
        })
    };
    let old_index = target_index(&visual);
    assert_eq!(old_index, 93);
    editor.update(&mut visual, |view, cx| {
        view.browser.scroll.0.borrow().base_handle.set_offset(point(
            px(0.0),
            -px((old_index - 3) as f32 * super::ROW_HEIGHT + 5.0),
        ));
        cx.notify();
    });
    visual.run_until_parked();
    let before = visual.debug_bounds(disclosure(old_index)).unwrap();
    let gate = editor.update(&mut visual, |view, _| {
        view.browser.tree.test_gate_next_scan()
    });
    visual.simulate_click(before.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(gate.reached.load(std::sync::atomic::Ordering::Acquire));
    let new_index = target_index(&visual);
    assert_eq!(
        new_index, 13,
        "the earlier branch closed on normal expansion"
    );
    let loading = visual.debug_bounds(disclosure(new_index));
    gate.release();
    visual.run_until_parked();

    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries[new_index + 1].path, target.join("00.hex"));
        assert!(!view.browser.tree.test_worker_busy());
    });
    let expanded = visual.debug_bounds(disclosure(new_index));
    assert_eq!(
        expanded.map(|bounds| bounds.top()),
        Some(before.top()),
        "expanding a long folder must keep its disclosure in view"
    );
    assert_eq!(
        loading.map(|bounds| bounds.top()),
        Some(before.top()),
        "the disclosure must also stay put while its children are loading"
    );
    assert_eq!(
        visual.debug_bounds("browser-entry-14").unwrap().top(),
        before.bottom(),
        "the first child is painted immediately below its folder"
    );
}

#[gpui::test]
fn browser_inline_tree_nested_keyboard_refresh_and_hover_retain_identity(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let folder = directory.0.join("a");
    let nested = folder.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let sample = nested.join("kick.wav");
    std::fs::write(&sample, mono_pcm_wav(32)).unwrap();
    editor.update(cx, |view, cx| {
        view.browser.auto_audition = false;
        view.request_browser_scan(directory.0.clone(), cx);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let disclosure = visual.debug_bounds("browser-disclosure-0").unwrap();
    visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    // Click the nested folder label, then actual Right/Left actions in result focus.
    let row = visual.debug_bounds("browser-entry-1").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    let (requests, entries, state) = editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries[2].path, sample);
        assert_eq!(view.browser.tree.depth(&sample), 2);
        assert_eq!(view.browser.selected_index, 1);
        (
            view.browser.tree.requests_started,
            entries,
            view.state.clone().unwrap(),
        )
    });
    let sample_bounds = visual.debug_bounds("browser-entry-2").unwrap();
    for x in [0.2, 0.6, 0.9, 0.4] {
        visual.simulate_mouse_move(
            point(
                sample_bounds.left() + sample_bounds.size.width * x,
                sample_bounds.center().y,
            ),
            None,
            Modifiers::default(),
        );
        visual.run_until_parked();
    }
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser.tree.requests_started, requests,
            "hover never re-enumerates folders"
        );
        assert!(
            Arc::ptr_eq(&entries, &view.browser_visible_entries(cx)),
            "unchanged rows retain their snapshot"
        );
        assert!(Arc::ptr_eq(&state, view.state.as_ref().unwrap()));
    });
    visual.simulate_keystrokes("down");
    let added = nested.join("after.wav");
    std::fs::write(&added, mono_pcm_wav(64)).unwrap();
    editor.update(&mut visual, |view, cx| view.refresh_browser_tree(cx));
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(
            entries[view.browser.selected_index].path, sample,
            "refresh retains selected native path despite inserted row"
        );
        assert!(entries.iter().any(|entry| entry.path == added));
        assert_eq!(view.browser.directory, directory.0);
    });
    visual.simulate_keystrokes("left");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            nested
        )
    });
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("down");
    editor.update(&mut visual, |view, cx| {
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            sample
        );
        view.browser.playback_generation = 10;
        view.browser.playback_path = Some(sample.clone());
        view.browser.playback_output = Some((1, 1));
        view.browser.playback_phase = hex_audio::sample_preview::Phase::Preparing;
        view.browser.playback_pending = Some(super::PlaybackPreviewRequest {
            generation: 10,
            output_generation: 1,
            output_identity: 1,
            start_fraction: 0.5,
            source: super::BrowserPlaybackSource::File(sample.clone()),
            sample_rate: 48_000,
        });
    });
    let disclosure = visual.debug_bounds("browser-disclosure-1").unwrap();
    visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert!(
            !view
                .browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.path == sample)
        );
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            nested
        );
        assert_eq!(view.browser.playback_generation, 11);
        assert!(view.browser.playback_pending.is_none());
        assert!(view.browser.playback_output.is_none());
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Idle
        );
    });
    visual.simulate_keystrokes("left");
    visual.simulate_keystrokes("left");
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert!(
            !view
                .browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.path == nested)
        )
    });
}

#[gpui::test]
fn browser_tree_same_name_files_keep_distinct_preview_paths_and_bounded_labels(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let a = directory.0.join("a");
    let b = directory.0.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let first = a.join("kick.wav");
    let second = b.join("kick.wav");
    std::fs::write(&first, mono_pcm_wav(32)).unwrap();
    std::fs::write(&second, mono_pcm_wav(64)).unwrap();
    editor.update(cx, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.width = 220.0;
        view.request_browser_scan(directory.0.clone(), cx);
        view.refresh_sample_catalog(cx);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (index, keep) in [(0, false), (2, true)] {
        let selector = Box::leak(format!("browser-disclosure-{index}").into_boxed_str());
        let disclosure = visual.debug_bounds(selector).unwrap();
        let modifiers = Modifiers {
            platform: keep,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, modifiers);
        visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, modifiers);
        visual.run_until_parked();
    }
    for (index, path, frames) in [(1, first, 32), (3, second, 64)] {
        let selector = Box::leak(format!("browser-entry-{index}").into_boxed_str());
        let label_selector = Box::leak(format!("browser-entry-label-{index}").into_boxed_str());
        let row = visual.debug_bounds(selector).unwrap();
        let label = visual.debug_bounds(label_selector).unwrap();
        assert!(label.left() >= row.left() && label.right() <= row.right());
        assert!(label.size.height <= row.size.height);
        visual.simulate_mouse_down(label.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(label.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("right");
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert_eq!(view.browser.playback_path.as_ref(), Some(&path));
            let entries = view.browser_visible_entries(cx);
            assert_eq!(entries[index].name, "kick.wav");
            assert_eq!(entries[index].path, path);
            let key = super::PreviewKey {
                path: path.clone(),
                sample_rate: 48_000,
            };
            assert_eq!(
                view.browser.preview_cache[&key]
                    .metadata
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .playback_frames,
                frames
            );
            assert!(view.test_probes.last_enqueued_import_to_track.is_none());
        });
    }
    editor.update(&mut visual, |view, cx| {
        view.select_browser_source(BrowserSection::Samples, cx);
        view.browser.search.update(cx, |input, cx| {
            input.set_text("kick", cx);
            cx.notify();
        });
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(
            entries.len(),
            2,
            "indexed Samples search retains both native paths"
        );
        assert_eq!(entries[0].name, entries[1].name);
        assert_ne!(entries[0].path, entries[1].path);
    });
}

#[gpui::test]
fn browser_samples_count_tracks_hierarchy_items_and_flat_sample_matches(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let first = PreferencesTestDirectory::new();
    let second = PreferencesTestDirectory::new();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.places = vec![first.0.clone(), second.0.clone()];
        view.browser.directory = first.0.clone();
        view.browser.section = BrowserSection::Samples;
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("", cx));
        view.browser.samples.replace(Default::default());
        view.browser.tree.source_changed();
        assert_eq!(view.browser_visible_entries(cx).len(), 2);
        assert_eq!(view.sample_results_count_text(cx), "2 items");
        cx.notify();
    });
    let nested = first.0.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("kick.wav"), mono_pcm_wav(16)).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.expand_browser_folder(first.0.clone(), cx)
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .all(|entry| entry.kind == EntryKind::Directory)
        );
        assert_eq!(view.sample_results_count_text(cx), "3 items");
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| {
            input.set_text("kick", cx);
            cx.notify();
        });
        view.refresh_sample_catalog(cx);
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
        assert_eq!(view.sample_results_count_text(cx), "1 samples");
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("missing", cx));
        assert!(view.browser_visible_entries(cx).is_empty());
        assert_eq!(view.sample_results_count_text(cx), "0 samples");
    });
}

#[gpui::test]
fn place_correctness_reselect_gates_cached_rows_and_old_audition_callbacks(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.browser.section = BrowserSection::Place(directory.0.clone());
        view.browser.loading = false;
        view.browser.error = None;
        view.browser.scans = Default::default();
        // Hold the worker lane so the fresh request remains deterministically pending.
        let old = view.browser.scans.request(directory.0.clone()).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&directory.0).unwrap();
        view.select_browser_source(BrowserSection::Place(directory.0.clone()), cx);
        assert!(view.browser.loading);
        assert!(
            view.browser_visible_entries(cx).is_empty(),
            "same-root reselect exposes retained offline listing before availability returns"
        );
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("loop", cx));
        assert!(
            view.browser_visible_entries(cx).is_empty(),
            "query exposes cached offline index before availability returns"
        );
        let generation = view.browser.playback_generation;
        // A closure painted before reselect may still carry the former path.
        view.play_browser_sample_from(path.clone(), 0.5, cx);
        assert_eq!(
            view.browser.playback_generation, generation,
            "old waveform callback admitted stale preview"
        );
        view.insert_browser_audio(path.clone(), cx);
        assert!(
            view.test_probes.last_enqueued_import_to_track.is_none(),
            "old result callback admitted stale import"
        );
        let (current, fresh) = view.browser.scans.complete(old.generation);
        assert!(!current);
        let fresh = fresh.unwrap();
        view.browser_scan_finished(fresh, hex_library::scan_directory(&directory.0), cx);
        assert!(!view.browser.loading);
        assert!(view.browser.error.is_some());
        assert!(view.browser_visible_entries(cx).is_empty());
        let generation = view.browser.playback_generation;
        view.play_browser_sample_from(path, 0.5, cx);
        assert_eq!(view.browser.playback_generation, generation);
        cx.notify();
    });
}

#[gpui::test]
fn place_correctness_refresh_retains_identity_only_after_current_success(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.section = BrowserSection::Place(directory.0.clone());
        view.browser.loading = false;
        view.browser.error = None;
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("loop", cx));
        view.browser.results_focused = true;
        view.browser.selected_index = 0;
        view.browser.scans = Default::default();
        let old = view.browser.scans.request(directory.0.clone()).unwrap();
        view.request_browser_scan(directory.0.clone(), cx);
        assert!(view.browser_visible_entries(cx).is_empty());
        // A concurrent catalog refresh must not discard the privately saved identity.
        std::fs::write(directory.0.join("another.wav"), mono_pcm_wav(16)).unwrap();
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
            cx,
        );
        // The superseded worker must not reauthorize rows.
        let (current, fresh) = view.browser.scans.complete(old.generation);
        assert!(!current);
        assert!(view.browser_visible_entries(cx).is_empty());
        view.browser_scan_finished(
            fresh.unwrap(),
            hex_library::scan_directory(&directory.0),
            cx,
        );
        assert!(!view.browser.loading);
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
        assert!(view.browser.results_focused);
        assert_eq!(
            view.browser_visible_entries(cx)[view.browser.selected_index].path,
            path
        );
        // An available, freshly validated row still admits manual offset preview.
        let generation = view.browser.playback_generation;
        view.play_browser_sample_from(path, 0.5, cx);
        assert_eq!(view.browser.playback_generation, generation + 1);
        cx.notify();
    });
}

#[gpui::test]
fn browser_tree_refresh_updates_expanded_branch_hidden_by_indexed_query(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, visible_sample) = browser_drag_fixture(cx);
    let branch = directory.0.join("branch");
    std::fs::create_dir(&branch).unwrap();
    let old = branch.join("old.wav");
    let fresh = branch.join("fresh.wav");
    std::fs::write(&old, mono_pcm_wav(16)).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.select_browser_source(BrowserSection::Samples, cx);
        cx.notify();
    });
    visual.run_until_parked();
    for row in [0, 1] {
        let selector = Box::leak(format!("browser-disclosure-{row}").into_boxed_str());
        let disclosure = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_down(disclosure.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(disclosure.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    let before = editor.read_with(&visual, |view, cx| {
        assert!(
            view.browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.path == old)
        );
        view.browser.tree.requests_started
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| {
            input.set_text("loop", cx);
            cx.notify();
        });
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, visible_sample);
    });
    std::fs::remove_file(&old).unwrap();
    std::fs::write(&fresh, mono_pcm_wav(32)).unwrap();
    editor.update(&mut visual, |view, cx| view.refresh_browser_tree(cx));
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.browser
                .samples
                .catalog
                .entries()
                .iter()
                .any(|entry| entry.path == fresh)
        );
        println!(
            "hidden-branch refresh started {} folder requests",
            view.browser.tree.requests_started - before
        );
    });
    editor.update(&mut visual, |view, cx| {
        view.browser.search.update(cx, |input, cx| input.clear(cx));
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-label-2").is_some(),
        "expanded child row must be rendered after clearing query"
    );
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert!(
            entries.iter().any(|entry| entry.path == fresh),
            "clearing query restored stale cached contents despite refresh"
        );
        assert!(!entries.iter().any(|entry| entry.path == old));
        assert_eq!(
            view.browser.tree.requests_started - before,
            2,
            "only the two already-expanded folders need enumeration"
        );
        assert!(view.audio_pub.pending_audio_commands.is_empty());
        assert!(view.test_probes.last_enqueued_import_to_track.is_none());
    });
}

#[path = "browser_ui_test/collections_and_layout_tests.rs"]
mod collections_and_layout_tests;
#[path = "browser_ui_test/collections_persistence_tests.rs"]
mod collections_persistence_tests;
#[path = "browser_ui_test/device_and_drop_tests.rs"]
mod device_and_drop_tests;
#[path = "browser_ui_test/preview_and_import_tests.rs"]
mod preview_and_import_tests;
#[path = "browser_ui_test/search_and_navigation_tests.rs"]
mod search_and_navigation_tests;
#[path = "browser_ui_test/tag_editor_tests.rs"]
mod tag_editor_tests;
#[path = "browser_ui_test/tag_filter_tests.rs"]
mod tag_filter_tests;
