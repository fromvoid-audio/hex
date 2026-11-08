//! Workspace Tab dispatch uses an isolated browser before worker startup.
use super::{EditorMode, EditorView};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_session::service::{Event, Request, Service};

fn next(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none());
            return event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
use std::time::Duration;

#[gpui::test]
fn workspace_generation2_pointer_selected_views_route_actual_tab(cx: &mut TestAppContext) {
    for key_char in [None, Some("\t".to_owned())] {
        let (_files, handle, editor) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let (track, scene) = editor.read_with(&visual, |v, _| {
            (v.selected_track.unwrap(), v.selected_scene.unwrap())
        });
        for console in [false, true] {
            let selector = if console {
                format!("console-cell-{}-{}", track.value(), scene.value())
            } else {
                format!("arrangement-lane-{}", track.value())
            };
            let bounds = visual
                .debug_bounds(Box::leak(selector.into_boxed_str()))
                .unwrap();
            let point = if console {
                bounds.center()
            } else {
                gpui::point(
                    bounds.origin.x + bounds.size.width * 0.6,
                    bounds.origin.y + gpui::px(22.0),
                )
            };
            visual.simulate_click(point, Modifiers::default());
            visual.run_until_parked();
            let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
            let view_state = editor.read_with(&visual, |v, _| {
                (
                    v.selected_clip_ids(),
                    v.time_selection,
                    v.selected_scene,
                    v.selected_cell,
                    v.selected_track,
                    v.track_scroll.offset(),
                    v.session_track_start,
                    v.viewport,
                    (v.clip_view_open, v.device_panel_active, v.offline_cursor),
                )
            });
            let origin = if console {
                EditorMode::Session
            } else {
                EditorMode::Arrangement
            };
            editor.read_with(&visual, |v, _| assert_eq!(v.mode, origin));
            for expected in [
                if console {
                    EditorMode::Arrangement
                } else {
                    EditorMode::Session
                },
                origin,
            ] {
                visual.simulate_event(gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke {
                        modifiers: Modifiers::default(),
                        key: "tab".into(),
                        key_char: key_char.clone(),
                    },
                    is_held: false,
                });
                visual.simulate_event(gpui::KeyUpEvent {
                    keystroke: gpui::Keystroke {
                        modifiers: Modifiers::default(),
                        key: "tab".into(),
                        key_char: key_char.clone(),
                    },
                });
                visual.run_until_parked();
                visual.update(|window, cx| editor.read_with(cx, |v, _| {
                    eprintln!("ordinary Tab: console={console} key_char={key_char:?} expected={expected:?} actual={:?} root={} header={} pointer={} range={} scrub={}", v.mode, v.focus.is_focused(window), v.track_header_focused(window), v.pointer_down.is_some(), v.time_selection_gesture.is_some(), v.scrub_active());
                    assert_eq!(v.mode, expected, "real routed key after ordinary workspace click");
                    assert_eq!(v.state.as_ref().unwrap().project, before.project);
                    assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
                    assert_eq!(v.worker_requests_pending, 0);
                    assert!(v.pending_requests.is_empty());
                    assert_eq!((v.selected_clip_ids(), v.time_selection, v.selected_scene, v.selected_cell,
                        v.selected_track, v.track_scroll.offset(), v.session_track_start, v.viewport,
                        (v.clip_view_open, v.device_panel_active, v.offline_cursor)), view_state);
                }));
            }
            if !console {
                // Use the real toolbar to enter Console for its ordinary click path.
                let button = visual
                    .debug_bounds("workspace-view-toggle")
                    .unwrap()
                    .center();
                visual.simulate_click(button, Modifiers::default());
                visual.run_until_parked();
            }
        }
    }
}

#[gpui::test]
fn workspace_generation2_startup_app_bindings_and_browser_return_route_tab(
    cx: &mut TestAppContext,
) {
    // The test platform cannot attest font registration. Keep production's
    // mandatory font check untouched; install its actual input setup calls.
    cx.update(|cx| {
        super::bind_editor_keys(cx);
        super::menus::install(cx, &[]);
        super::dialogs::install_project_actions(cx);
    });
    let environment = super::test_environment::Environment::new().unwrap();
    std::fs::create_dir(environment.root().join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(window, cx, service, environment.clone())
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    let deadline = crate::test_wait::hang_deadline();
    loop {
        editor.update(&mut visual, |v, cx| {
            v.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        if editor.read_with(&visual, |v, _| v.state.is_some()) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "workspace_generation2_startup_app_bindings_and_browser_return_route_tab: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // No fixture assignment to mode, state, root focus or selection.
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.browser.directory, environment.root().join("samples"));
        assert_eq!(v.mode, EditorMode::Arrangement);
    });
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    for expected in [EditorMode::Session, EditorMode::Arrangement] {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.mode, expected,
                "constructor-owned focus with actual app input setup"
            );
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
        });
    }
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(hex_project::Command::AddTrack {
            name: "focus return".into()
        })))
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        editor.update(&mut visual, |v, cx| {
            v.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        if editor.read_with(&visual, |v, _| {
            v.worker_requests_pending == 0 && v.state.as_ref().unwrap().project.tracks().len() == 1
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "workspace_generation2_startup_app_bindings_and_browser_return_route_tab: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    eprintln!("actual app input setup and constructor-owned startup focus: reciprocal Tab passed");
    // The actual search action establishes its normal focus; a header pointer
    // click must return to the ordinary workspace without a fixture focus call.
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("piano");
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, cx| {
        assert_eq!(v.mode, EditorMode::Arrangement);
        // The existing text handler receives literal Tab. Preserve that local
        // behavior; workspace navigation must not consume the input instead.
        assert_eq!(v.browser.search.read(cx).text(), "piano\t");
    });
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let header = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(
        header.origin + gpui::point(gpui::px(12.0), gpui::px(9.0)),
        Modifiers::default(),
    );
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    for expected in [EditorMode::Session, EditorMode::Arrangement] {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, expected, "actual header returns focus from browser");
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(v.worker_requests_pending, 0);
        });
    }
}

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
            assert!(!v.clip_paint_held());
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
        assert_eq!(visual.debug_bounds("clip-paint-preview").is_none(), cancel);
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
        .debug_bounds("workspace-view-toggle")
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
    assert!(visual.debug_bounds("workspace-view-toggle").is_some());
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert!(visual.debug_bounds("workspace-view-toggle").is_some());
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
        let name = visual.debug_bounds("workspace-view-toggle").unwrap();
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
                    _ => {
                        assert!(v.start_open_picker().is_some());
                    }
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
            if guard == 7 {
                assert!(v.project_open.busy());
                v.cancel_open_after_save();
            }
            assert_eq!(v.worker_requests_pending, 0);
            cx.notify();
        });
    }
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
}

#[gpui::test]
fn workspace_review_tab_preserves_selected_open_confirmation_until_cancel(cx: &mut TestAppContext) {
    let (files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let directory = files
        .wav("open-location.wav", 48_000, 8)
        .parent()
        .unwrap()
        .to_owned();
    let target = directory.join("selected.hex");
    let saved = Service::new(48_000, 8, 8_000_000).unwrap();
    next(&saved);
    saved.submit(Request::SaveNew(target.clone())).unwrap();
    next(&saved);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    assert!(before.unsaved);
    editor.update(&mut visual, |v, cx| {
        v.choose_project(target, hex_session::DocumentAudioMode::Resident);
        assert!(v.pending_open_path.is_some());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(cx.has_pending_prompt());
    visual.update(|window, cx| {
        editor.update(cx, |v, _| {
            assert!(
                v.pending_open_path.is_none(),
                "render transferred the selected path into its owned intent"
            );
            assert!(v.project_open.busy());
            assert!(!v.opening_project);
            v.focus.focus(window);
        })
    });
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, EditorMode::Arrangement);
        assert!(v.project_open.busy());
        assert_eq!(v.state.as_ref().unwrap().project, before.project);
        assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
        assert_eq!(v.worker_requests_pending, 0);
    });
    cx.simulate_prompt_answer("cancel");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(!v.project_open.busy()));
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
}

#[gpui::test]
fn workspace_review_tab_preserves_held_ruler_until_release_or_cancel(cx: &mut TestAppContext) {
    for cancel in [false, true] {
        let (_files, handle, editor) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        // Existing scrub tests hold the real seek-ack gate while exercising
        // pointer coalescing offline. No OS audio device is constructed.
        editor.update(&mut visual, |v, _| v.pending_seek_ticket = Some(1));
        let bounds = visual.debug_bounds("timeline-ruler").unwrap();
        let start = bounds.center();
        let end = start + gpui::point(gpui::px(10.0), gpui::px(0.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        editor.update(&mut visual, |v, _| {
            assert!(v.update_scrub(f32::from(end.x), false))
        });
        visual.simulate_keystrokes("tab");
        editor.update(&mut visual, |v, _| {
            assert_eq!(v.mode, EditorMode::Arrangement);
            assert!(
                v.update_scrub(f32::from(end.x), false),
                "held target still belongs to ruler capture"
            );
            assert_eq!(v.worker_requests_pending, 0);
        });
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        editor.update(&mut visual, |v, _| {
            v.pending_seek_ticket = None;
            if cancel {
                assert!(!v.tick_scrub());
            } else {
                assert!(
                    v.tick_scrub(),
                    "released final target follows existing offline refusal"
                );
                assert!(
                    v.error
                        .as_deref()
                        .is_some_and(|e| e.contains("audio output"))
                );
            }
            assert!(
                !v.cancel_scrub(),
                "release/refusal or Escape retired the future target"
            );
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
    }
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

#[gpui::test]
fn workspace_review_tab_preserves_held_middle_pan_until_release_or_cancel(cx: &mut TestAppContext) {
    for retirement in [0, 1, 2] {
        let cx = &mut cx.new_app();
        let (_files, handle, editor) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            window.activate_window();
            editor.update(cx, |v, cx| {
                let horizon = v.state.as_ref().unwrap().timeline_frames;
                let fit = super::viewport::Viewport::fit(horizon);
                let zoom = fit.zoom(horizon, 0, true);
                assert!(
                    zoom.length < fit.length,
                    "fixture has a real pannable viewport"
                );
                v.viewport = Some(zoom);
                v.focus.focus(window);
                cx.notify();
            });
        });
        visual.run_until_parked();
        let body = visual.debug_bounds("arrangement-content").unwrap();
        let start = body.origin + gpui::point(body.size.width * 0.5, gpui::px(30.0));
        let end = start - gpui::point(gpui::px(24.0), gpui::px(0.0));
        let initial = editor.read_with(&visual, |v, _| v.viewport);
        visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Middle, Modifiers::default());
        visual.run_until_parked();
        let (before, viewport, selection, pan) = editor.read_with(&visual, |v, _| {
            let p = v.arrangement_pan.expect("actual middle-button capture");
            assert_ne!(v.viewport, initial, "actual pointer movement pans");
            (
                v.state.as_ref().unwrap().clone(),
                v.viewport,
                (v.selected_clip_ids(), v.time_selection),
                (
                    p.generation,
                    p.revision,
                    p.last_x,
                    p.last_y,
                    p.timeline_width,
                ),
            )
        });
        assert!(visual.update(|window, cx| editor.read(cx).focus.is_focused(window)));
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.mode,
                EditorMode::Arrangement,
                "held pan owns workspace, retirement={retirement}"
            );
            let p = v
                .arrangement_pan
                .expect("Tab must retain the same pointer owner");
            assert_eq!(
                (
                    p.generation,
                    p.revision,
                    p.last_x,
                    p.last_y,
                    p.timeline_width
                ),
                pan
            );
            assert_eq!(v.viewport, viewport);
            assert_eq!((v.selected_clip_ids(), v.time_selection), selection);
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
        match retirement {
            0 => visual.simulate_mouse_up(end, MouseButton::Middle, Modifiers::default()),
            1 => {
                visual.simulate_keystrokes("escape");
                visual.simulate_mouse_up(end, MouseButton::Middle, Modifiers::default());
            }
            _ => visual.simulate_mouse_move(end, None, Modifiers::default()),
        }
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(v.arrangement_pan.is_none());
            assert_eq!(v.viewport, viewport);
            if retirement == 1 {
                // Existing Escape cancels the selected range as well as pan.
                assert_eq!(v.selected_clip_ids(), selection.0);
                assert!(v.time_selection.is_none());
            } else {
                assert_eq!((v.selected_clip_ids(), v.time_selection), selection);
            }
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, EditorMode::Arrangement);
            assert_eq!(v.viewport, viewport);
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
        });
    }
}

#[gpui::test]
fn workspace_three_returns_held_left_before_drag_preserves_capture(cx: &mut TestAppContext) {
    for trim in [false, true] {
        let cx = &mut cx.new_app();
        cx.update(super::bind_editor_keys);
        let (project, _, _, clip) = super::clip_drag_ui_test::audio_project();
        let (handle, editor) =
            super::clip_drag_ui_test::editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            window.activate_window();
            editor.read(cx).focus.focus(window);
        });
        visual.run_until_parked();
        let selector = Box::leak(if trim {
            format!("clip-trim-right-{}", clip.value()).into_boxed_str()
        } else {
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str()
        });
        let point = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
        let before = editor.read_with(&visual, |v, _| {
            assert!(v.pointer_down.is_some());
            assert!(v.active_clip_drag.is_none() && v.active_trim_drag.is_none());
            v.state.as_ref().unwrap().clone()
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, EditorMode::Arrangement, "pre-drag trim={trim}");
            assert!(v.pointer_down.is_some());
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
        visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert!(v.pointer_down.is_none());
            assert!(v.last_enqueued_edit.is_none());
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    }
}

#[gpui::test]
fn workspace_three_returns_plain_clip_click_retires_pending_pointer(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, _, _, clip) = super::clip_drag_ui_test::audio_project();
    let (handle, editor) = super::clip_drag_ui_test::editor_window(cx, project, Default::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| {
        window.activate_window();
        editor.read(cx).focus.focus(window);
    });
    visual.run_until_parked();
    let name = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    let point = visual.debug_bounds(name).unwrap().center();
    // No Tab or other keyboard event: the existing click path alone must retire.
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| assert!(v.pointer_down.is_some()));
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(
            v.pointer_down.is_none(),
            "ordinary clip click did not retire original press"
        );
        assert!(v.last_enqueued_edit.is_none());
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn workspace_three_returns_delayed_click_preserves_other_pointer_identity(cx: &mut TestAppContext) {
    for mismatch in 0..3 {
        let cx = &mut cx.new_app();
        cx.update(super::bind_editor_keys);
        let (project, _, target, clip) = super::clip_drag_ui_test::audio_project();
        let mut model = hex_project::Editor::new(project, 8).unwrap();
        let hex_project::Created::Clip(other) = model
            .execute(hex_project::Command::CopyClipTo {
                clip,
                track: target,
                start: 0,
            })
            .unwrap()
        else {
            panic!("actual second clip");
        };
        let (handle, editor) = super::clip_drag_ui_test::editor_window(
            cx,
            model.project().clone(),
            Default::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            window.activate_window();
            editor.read(cx).focus.focus(window);
        });
        visual.run_until_parked();
        let name = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
        let point = visual.debug_bounds(name).unwrap().center();
        visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
        // Deliver the original surface's pending click after a different keyed
        // press has taken ownership, as with a delayed callback after redraw.
        let retained = editor.update(&mut visual, |v, _| {
            let generation = v.project_generation + u64::from(mismatch == 0);
            let owner = if mismatch == 1 { other } else { clip };
            let edge = (mismatch == 2).then_some(hex_project::TrimEdge::Right);
            v.remember_pointer_down(generation, owner, edge, 123.0);
            v.pointer_down.unwrap()
        });
        visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.pointer_down, Some(retained), "mismatch={mismatch}");
            assert!(v.last_enqueued_edit.is_none());
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
    }
}

#[gpui::test]
fn workspace_four_returns_completed_range_survives_console_controller_ticks(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| {
        window.activate_window();
        editor.read(cx).focus.focus(window);
    });
    visual.run_until_parked();
    let track = editor.read_with(&visual, |v, _| v.selected_track.unwrap());
    let selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(selector).unwrap();
    let start = lane.origin + gpui::point(lane.size.width * 0.25, gpui::px(22.0));
    let end = lane.origin + gpui::point(lane.size.width * 0.5, gpui::px(22.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| assert!(v.time_selection_gesture.is_some()));
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| {
        assert!(v.time_selection_gesture.is_none());
        let selection = v.time_selection.unwrap();
        assert_ne!(selection.anchor_frame, selection.end_frame);
        assert_eq!(v.edit_cursor.unwrap().generation, v.project_generation);
        (
            selection,
            v.selected_clip_ids(),
            v.state.as_ref().unwrap().clone(),
            v.project_generation,
        )
    });
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
    for _ in 0..3 {
        visual.executor().advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        // Execute the real periodic controller body even on test constructors
        // that intentionally omit the production detached timer.
        editor.update(&mut visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
        });
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.time_selection,
            Some(before.0),
            "completed range after real Console tick"
        );
        assert_eq!(v.selected_clip_ids(), before.1);
        assert_eq!(v.project_generation, before.3);
        assert_eq!(v.state.as_ref().unwrap().project, before.2.project);
        assert_eq!(v.state.as_ref().unwrap().revision, before.2.revision);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, EditorMode::Arrangement);
        assert_eq!(v.time_selection, Some(before.0));
    });
}

#[gpui::test]
fn workspace_four_returns_console_still_retires_active_or_invalid_range(cx: &mut TestAppContext) {
    // These deliberately injected invalid view contexts exercise validation;
    // they are not fabricated worker events or accepted project edits.
    for invalid in 0..4 {
        let cx = &mut cx.new_app();
        let (_files, handle, editor) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            window.activate_window();
            editor.read(cx).focus.focus(window);
        });
        visual.run_until_parked();
        let track = editor.read_with(&visual, |v, _| v.selected_track.unwrap());
        let selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
        let lane = visual.debug_bounds(selector).unwrap();
        let start = lane.origin + gpui::point(lane.size.width * 0.25, gpui::px(22.0));
        let end = lane.origin + gpui::point(lane.size.width * 0.5, gpui::px(22.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        if invalid != 3 {
            visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        }
        editor.update(&mut visual, |v, cx| {
            assert!(
                v.time_selection
                    .is_some_and(|s| s.anchor_frame != s.end_frame)
            );
            v.mode = EditorMode::Session;
            match invalid {
                0 => v.project_generation = v.project_generation.wrapping_add(1),
                1 => v.state = None,
                2 => v.edit_cursor = None,
                _ => assert!(v.time_selection_gesture.is_some()),
            }
            v.tick(cx);
            assert!(
                v.normalized_time_selection().is_none(),
                "invalid context {invalid}"
            );
            if invalid < 3 {
                assert!(
                    v.time_selection.is_none(),
                    "invalid selection retired {invalid}"
                );
            }
            if invalid < 2 {
                assert!(v.edit_cursor.is_none(), "stale cursor retired {invalid}");
            }
            eprintln!(
                "Console invalid context {invalid}: selection={:?}, active={}, queued={}",
                v.time_selection,
                v.time_selection_gesture.is_some(),
                v.worker_requests_pending
            );
            assert!(v.time_selection_gesture.is_none());
            assert!(v.time_range_move_gesture.is_none());
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
    }
}

#[gpui::test]
fn workspace_navigation_controls_fit_edges_and_preserve_state(cx: &mut TestAppContext) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [880.0, 1440.0] {
        visual.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
        visual.run_until_parked();
        let status = visual.debug_bounds("editor-status-bar").unwrap();
        let browser = visual.debug_bounds("toggle browser · cmd+alt+b").unwrap();
        assert!(browser.top() >= status.top() && browser.bottom() <= status.bottom());
        assert_eq!(browser.left(), status.left() + gpui::px(8.0));
        let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
        let mode_button = visual.debug_bounds("workspace-view-toggle").unwrap();
        assert!(mode_button.top() >= toolbar.top() && mode_button.bottom() <= toolbar.bottom());
        assert!(mode_button.right() <= toolbar.right());
        assert!(mode_button.left() > toolbar.center().x);
        let before = editor.read_with(&visual, |v, _| {
            (
                v.state.as_ref().unwrap().clone(),
                v.selected_track,
                v.selected_clip_ids(),
                v.time_selection,
                v.selected_cell,
                v.viewport,
                v.clip_view_open,
                v.device_panel_active,
                v.mode,
                v.browser.open,
            )
        });
        for _ in 0..2 {
            let browser = visual
                .debug_bounds("toggle browser · cmd+alt+b")
                .unwrap()
                .center();
            let was_open = editor.read_with(&visual, |v, _| v.browser.open);
            visual.simulate_click(browser, Modifiers::default());
            editor.read_with(&visual, |v, _| assert_eq!(v.browser.open, !was_open));
            let mode_button = visual
                .debug_bounds("workspace-view-toggle")
                .unwrap()
                .center();
            let was_mode = editor.read_with(&visual, |v, _| v.mode);
            visual.simulate_click(mode_button, Modifiers::default());
            editor.read_with(&visual, |v, _| assert_ne!(v.mode, was_mode));
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, before.0.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.0.revision);
            assert_eq!(
                (
                    v.selected_track,
                    v.selected_clip_ids(),
                    v.time_selection,
                    v.selected_cell,
                    v.viewport,
                    v.clip_view_open,
                    v.device_panel_active,
                    v.mode,
                    v.browser.open
                ),
                (
                    before.1,
                    before.2.clone(),
                    before.3,
                    before.4,
                    before.5,
                    before.6,
                    before.7,
                    before.8,
                    before.9
                )
            );
            assert!(v.pending_requests.is_empty());
        });
    }
}
