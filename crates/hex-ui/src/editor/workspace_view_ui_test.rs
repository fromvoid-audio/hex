//! Workspace Tab dispatch uses an isolated browser before worker startup.
use super::{EditorMode, EditorView};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_session::service::{Event, Request, Service};

fn next(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none());
            return event;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
use std::time::{Duration, Instant};

#[gpui::test]
fn workspace_review_tab_preserves_held_clip_paint_until_release_or_escape(cx: &mut TestAppContext) {
    for cancel in [false, true] {
        let (_files, handle, editor) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (track, project) = editor.read_with(&visual, |v, _| {
            (
                v.selected_track.unwrap(),
                (*v.state.as_ref().unwrap().project).clone(),
            )
        });
        let selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
        let lane = visual.debug_bounds(selector).unwrap();
        let start = gpui::point(
            lane.origin.x + lane.size.width * 0.25,
            lane.origin.y + gpui::px(22.0),
        );
        let end = gpui::point(lane.origin.x + lane.size.width * 0.5, start.y);
        visual.simulate_click(start, Modifiers::default());
        visual.simulate_event(gpui::MouseDownEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let preview = visual.debug_bounds("clip-paint-preview").unwrap();
        editor.read_with(&visual, |v, _| {
            assert!(v.clip_paint_gesture.is_some());
            assert!(v.time_selection_gesture.is_none());
            assert!(v.last_enqueued_edit.is_none());
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, EditorMode::Arrangement);
            assert!(v.clip_paint_gesture.is_some());
            assert!(v.last_enqueued_edit.is_none());
            assert_eq!(v.worker_requests_pending, 0);
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("clip-paint-preview"), Some(preview));
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert!(v.clip_paint_gesture.is_none());
            if cancel {
                assert!(v.last_enqueued_edit.is_none());
                assert_eq!(v.worker_requests_pending, 0);
            } else {
                let command = v.last_enqueued_edit.clone().unwrap();
                assert!(
                    matches!(command, hex_project::Command::PlaceMidiClipWithOverwrite {
                    track: target, length, ..
                } if target == track && length.raw() > 0)
                );
                let mut canonical = hex_project::Editor::new(project.clone(), 8).unwrap();
                let hex_project::Created::Clip(clip) = canonical.execute(command).unwrap() else {
                    panic!("painted clip")
                };
                assert!(canonical.project().midi_clip(clip).is_some());
                assert!(canonical.undo());
                assert!(canonical.project().midi_clip(clip).is_none());
                assert!(!canonical.undo());
                assert!(canonical.redo());
                assert!(canonical.project().midi_clip(clip).is_some());
            }
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-paint-preview").is_none());
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
    }
}

#[gpui::test]
fn workspace_review_tab_preserves_active_track_resize(cx: &mut TestAppContext) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let track = editor.read_with(&visual, |v, _| v.selected_track.unwrap());
    let selector = Box::leak(format!("track-height-resize-{}", track.value()).into_boxed_str());
    let point = visual.debug_bounds(selector).unwrap().center();
    let to = point + gpui::point(gpui::px(0.0), gpui::px(12.0));
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| assert!(v.track_resize_active()));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, EditorMode::Arrangement);
        assert!(v.track_resize_active());
        assert_eq!(v.worker_requests_pending, 0);
    });
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| assert!(!v.track_resize_active()));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
}

#[gpui::test]
fn workspace_review_toolbar_console_returns_tab_to_workspace(cx: &mut TestAppContext) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    let track = editor.read_with(&visual, |v, _| v.selected_track.unwrap());
    let selector =
        Box::leak(format!("arrangement-track-header-{}", track.value()).into_boxed_str());
    let header = visual.debug_bounds(selector).unwrap();
    visual.simulate_click(
        header.origin + gpui::point(gpui::px(12.0), gpui::px(9.0)),
        Modifiers::default(),
    );
    visual
        .update(|window, cx| editor.read_with(cx, |v, _| assert!(v.track_header_focused(window))));
    // Header selection precedes the view switch and has its own selection policy.
    let selection = editor.read_with(&visual, |v, _| (v.selected_clip_ids(), v.time_selection));
    let console = visual
        .debug_bounds("show console view · tab")
        .unwrap()
        .center();
    visual.simulate_click(console, Modifiers::default());
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
    visual.update(|window, cx| {
        editor.read_with(cx, |v, _| {
            assert!(v.focus.is_focused(window) || v.track_header_focused(window));
        })
    });
    visual.simulate_keystrokes("tab");
    visual.update(|window, cx| editor.read_with(cx, |v, _| assert!(v.focus.is_focused(window))));
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, EditorMode::Arrangement);
        assert_eq!((v.selected_clip_ids(), v.time_selection), selection);
        assert_eq!(v.state.as_ref().unwrap().project, before.project);
        assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
}

fn fixture(
    cx: &mut TestAppContext,
) -> (
    super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
) {
    cx.update(super::bind_editor_keys);
    let files = super::external_audio_drop_ui_test::BatchFiles::new();
    let path = files.wav("fixture.wav", 48_000, 8);
    let directory = path.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(path.clone())).unwrap();
    next(&service);
    service
        .submit(Request::Edit(hex_project::Command::AddScene {
            name: "test scene".into(),
        }))
        .unwrap();
    let event = next(&service);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                let track = &event.state.project.tracks()[0];
                let track_id = track.id();
                let clip = track.clips()[0].id();
                let scene = event.state.project.scenes()[0].id();
                v.selected_scene = Some(scene);
                v.selected_cell = Some((track_id, scene));
                v.selected_track = Some(track_id);
                v.selected_clip = Some(clip);
                v.clip_selection.only(track_id, clip);
                v.time_selection = Some(super::time_selection::TimeSelection {
                    anchor_track: track_id,
                    end_track: track_id,
                    anchor_frame: 2,
                    end_frame: 6,
                });
                v.state = Some(event.state);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor)
}

#[gpui::test]
fn workspace_tab_switches_both_directions_without_edit_or_view_state_loss(cx: &mut TestAppContext) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    let selection = editor.read_with(&visual, |v, _| (v.selected_clip_ids(), v.time_selection));
    let console_selection = editor.read_with(&visual, |v, _| (v.selected_scene, v.selected_cell));
    editor.update(&mut visual, |v, cx| {
        v.viewport = Some(super::viewport::Viewport {
            start: 1000,
            length: 24_000,
        });
        v.session_track_start = 2;
        v.clip_view_open = false;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrange view").is_some());
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert!(visual.debug_bounds("console view").is_some());
    assert!(visual.debug_bounds("arrange view").is_none());
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, EditorMode::Arrangement);
        assert_eq!(
            v.viewport,
            Some(super::viewport::Viewport {
                start: 1000,
                length: 24_000
            })
        );
        assert_eq!(v.session_track_start, 2);
        assert!(!v.clip_view_open);
        assert_eq!((v.selected_clip_ids(), v.time_selection), selection);
        assert_eq!((v.selected_scene, v.selected_cell), console_selection);
        assert_eq!(v.state.as_ref().unwrap().project, before.project);
        assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
    for width in [880.0, 1280.0] {
        visual.simulate_resize(gpui::size(gpui::px(width), gpui::px(540.0)));
        visual.run_until_parked();
        let name = visual.debug_bounds("arrange view").unwrap();
        let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
        assert!(name.size.width > gpui::px(0.0));
        assert!(name.origin.x >= toolbar.origin.x && name.right() <= toolbar.right());
    }
}

#[gpui::test]
fn workspace_tab_leaves_browser_numeric_and_nested_focus_in_charge(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let browser = editor.read_with(&visual, |v, cx| {
        v.browser.search.read(cx).focus_handle().clone()
    });
    visual.update(|window, _| browser.focus(window));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    // The real numeric control owns its local draft and Tab routing.
    let bounds = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    // Both detail editors use their own focus, even though they descend from the workspace.
    for audio in [false, true] {
        visual.update(|window, cx| {
            editor.update(cx, |v, _| {
                if audio {
                    v.audio_event_focus.focus(window);
                } else {
                    v.midi_focus.focus(window);
                }
            })
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    }
}

#[gpui::test]
fn workspace_tab_respects_modifiers_and_retained_root_modal_state(cx: &mut TestAppContext) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for key in ["shift-tab", "cmd-tab", "alt-tab", "ctrl-tab"] {
        visual.simulate_keystrokes(key);
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    }
    for guard in 0..8 {
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.focus.focus(window);
                match guard {
                    0 => v.close_prompt_open = true,
                    1 => v.save_dialog_open = true,
                    2 => v.export_dialog_open = true,
                    3 => v.preset_dialog_open = true,
                    4 => v.opening_project = true,
                    5 => v.theme_settings.open = true,
                    6 => v.tuning.open = true,
                    _ => v.pending_open_path = Some(std::path::PathBuf::from("unused.hex")),
                }
                cx.notify();
            })
        });
        visual.simulate_keystrokes("tab");
        editor.update(&mut visual, |v, cx| {
            assert_eq!(v.mode, EditorMode::Arrangement, "guard {guard}");
            v.close_prompt_open = false;
            v.save_dialog_open = false;
            v.export_dialog_open = false;
            v.preset_dialog_open = false;
            v.opening_project = false;
            v.theme_settings.open = false;
            v.tuning.open = false;
            v.pending_open_path = None;
            assert_eq!(v.worker_requests_pending, 0);
            cx.notify();
        });
    }
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
}

#[gpui::test]
fn workspace_tab_from_track_header_preserves_rename_input_and_returns_to_workspace(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let track = editor.read_with(&visual, |v, _| v.selected_track.unwrap());
    let selector =
        Box::leak(format!("arrangement-track-header-{}", track.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).unwrap();
    let point = bounds.origin + gpui::point(gpui::px(12.0), gpui::px(9.0));
    visual.simulate_click(point, Modifiers::default());
    visual
        .update(|window, cx| editor.read_with(cx, |v, _| assert!(v.track_header_focused(window))));
    visual.simulate_keystrokes("cmd-r");
    editor.read_with(&visual, |v, _| assert!(v.track_name.is_editing()));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    visual.simulate_keystrokes("escape");
    editor.read_with(&visual, |v, _| assert!(!v.track_name.is_editing()));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
}
