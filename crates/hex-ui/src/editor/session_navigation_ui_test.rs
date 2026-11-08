use super::{EditorMode, EditorView, install_close_guard};
use gpui::{
    AnyWindowHandle, AppContext, Empty, Entity, Pixels, TestAppContext, VisualTestContext, px, size,
};
use hex_project::{BeatTime, Command, Created, Editor, Project, SessionCellContent};
use hex_session::service::{Request, Service, ViewState};
use std::sync::Arc;

fn fixture() -> (
    Project,
    hex_project::TrackId,
    hex_project::TrackId,
    hex_project::SceneId,
    hex_project::SceneId,
) {
    let mut editor =
        Editor::new(Project::new(48_000).expect("test project"), 4).expect("test editor");
    let Created::Track(first_track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "one".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("first instrument")
    else {
        panic!("first track")
    };
    let Created::Track(second_track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "two".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("second instrument")
    else {
        panic!("second track")
    };
    let Created::Scene(first_scene) = editor
        .execute(Command::AddScene { name: "one".into() })
        .expect("first scene")
    else {
        panic!("first scene")
    };
    editor
        .execute(Command::CreateSessionMidiClip {
            track: first_track,
            scene: first_scene,
            name: "pattern".into(),
            source_length: BeatTime::from_quarters(4).expect("valid length"),
        })
        .expect("session midi clip");
    let Created::Scene(second_scene) = editor
        .execute(Command::AddScene { name: "two".into() })
        .expect("second scene")
    else {
        panic!("second scene")
    };
    (
        editor.project().clone(),
        first_track,
        second_track,
        first_scene,
        second_scene,
    )
}

fn bind_test_keys(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
}

fn session_window(
    cx: &mut TestAppContext,
) -> (
    AnyWindowHandle,
    Entity<EditorView>,
    hex_project::TrackId,
    hex_project::SceneId,
) {
    bind_test_keys(cx);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let (project, first_track, second_track, first_scene, _) = fixture();
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("test offline output".into()))
            });
            install_close_guard(window, cx, editor.downgrade());
            editor.update(cx, |view, cx| {
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                view.mode = EditorMode::Session;
                view.selected_cell = Some((first_track, first_scene));
                view.selected_scene = None;
                view.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor, second_track, first_scene)
}

fn worker_session_window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<EditorView>) {
    bind_test_keys(cx);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("test offline output".into()))
            });
            install_close_guard(window, cx, editor.downgrade());
            editor.update(cx, |view, window_cx| {
                view.mode = EditorMode::Session;
                view.focus.focus(window);
                window_cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor)
}

fn submit_edit_and_wait(
    cx: &mut TestAppContext,
    handle: AnyWindowHandle,
    editor: &Entity<EditorView>,
    command: Command,
) {
    let accepted = cx
        .update_window(handle, |_, _, cx| {
            editor.update(cx, |view, _| view.enqueue(Request::Edit(command)))
        })
        .expect("editor window");
    assert!(accepted, "test edit should enter the worker queue");
    wait_for_worker(cx, handle, editor, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
}

fn wait_for_worker(
    cx: &mut TestAppContext,
    handle: AnyWindowHandle,
    editor: &Entity<EditorView>,
    ready: impl Fn(&EditorView) -> bool,
) {
    for _ in 0..500 {
        cx.update_window(handle, |_, _, cx| {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
        })
        .expect("editor window");
        if editor.read_with(cx, |view, _| ready(view)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("project worker did not complete in time");
}

#[gpui::test]
fn session_key_dispatch_navigates_without_edits_and_enter_refuses_offline_playback(
    cx: &mut TestAppContext,
) {
    let (handle, editor, second_track, first_scene) = session_window(cx);
    cx.simulate_keystrokes(handle, "right right down up left left enter");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.selected_cell,
            Some((
                view.state.as_ref().unwrap().project.tracks()[0].id(),
                first_scene
            ))
        );
        assert_ne!(view.selected_cell, Some((second_track, first_scene)));
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert!(!view.play_intent.desired_playing(false));
        assert_eq!(
            view.error.as_deref(),
            Some("session playback requires an audio output")
        );
    });
}

#[gpui::test]
fn session_keyboard_focus_switches_details_between_cells_and_clips(cx: &mut TestAppContext) {
    let (handle, editor, second_track, first_scene) = session_window(cx);
    cx.simulate_keystrokes(handle, "right");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_cell, Some((second_track, first_scene)));
        assert!(view.device_panel_active, "cell focus selects devices");
        assert!(view.clip_view_open, "keyboard focus keeps the pane open");
    });

    cx.simulate_keystrokes(handle, "left");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.selected_cell,
            Some((
                view.state.as_ref().unwrap().project.tracks()[0].id(),
                first_scene
            ))
        );
        assert!(
            !view.device_panel_active,
            "midi clip focus selects clip details"
        );
        assert!(view.clip_view_open, "clip focus keeps the pane open");
    });
}

#[gpui::test]
fn piano_focus_keeps_arrow_and_enter_out_of_session_navigation_and_launch(cx: &mut TestAppContext) {
    let (handle, editor, second_track, first_scene) = session_window(cx);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, _| {
            view.select_session_scene(first_scene);
            view.midi_focus.focus(window);
        });
    })
    .expect("focus piano editor");
    cx.simulate_keystrokes(handle, "right enter cmd-d");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_cell, None);
        assert_eq!(view.selected_scene, Some(first_scene));
        assert_ne!(view.selected_cell, Some((second_track, first_scene)));
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert_eq!(view.state.as_ref().unwrap().project.scenes().len(), 2);
    });
}

#[gpui::test]
fn scene_column_stays_pinned_and_aligned_at_small_and_large_window_widths(cx: &mut TestAppContext) {
    let (handle, _, _, _) = session_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [640.0, 880.0, 1440.0] {
        visual.simulate_resize(size(px(width), px(700.0)));
        visual.run_until_parked();
        let header = visual
            .debug_bounds("session-scenes-header")
            .expect("scene header bounds");
        let row = visual
            .debug_bounds("session-scene-column")
            .expect("scene row bounds");
        let window = visual.update(|window, _| window.bounds());
        let window_right = window.origin.x + window.size.width - px(4.0);
        assert_eq!(header.size.width, px(110.0));
        assert!(near(header.origin.x, row.origin.x));
        assert!(near(header.origin.x + header.size.width, window_right));
        assert!(near(row.origin.x + row.size.width, window_right));
    }
}

#[gpui::test]
fn scene_launch_leads_the_caption_and_uses_the_guarded_offline_action(cx: &mut TestAppContext) {
    let (handle, editor, _, scene) = session_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(700.0)));
    visual.run_until_parked();
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    let launch = visual
        .debug_bounds(Box::leak(
            format!("console-launch-scene-{}", scene.value()).into_boxed_str(),
        ))
        .unwrap();
    let caption = visual
        .debug_bounds(Box::leak(
            format!("console-scene-menu-{}", scene.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(launch.right() < caption.left());
    visual.simulate_click(launch.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_scene, Some(scene));
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert_eq!(
            view.error.as_deref(),
            Some("session playback requires an audio output")
        );
    });
}

#[gpui::test]
fn session_scene_duplicate_and_reorder_keys_use_worker_edits(cx: &mut TestAppContext) {
    let (handle, editor) = worker_session_window(cx);
    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::AddMidiTrack {
            name: "instrument".into(),
        },
    );
    let track = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::AddScene {
            name: "first".into(),
        },
    );
    let first_scene = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.scenes()[0].id()
    });
    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::AddScene {
            name: "second".into(),
        },
    );
    let second_scene = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.scenes()[1].id()
    });
    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::CreateSessionMidiClip {
            track,
            scene: first_scene,
            name: "source".into(),
            source_length: BeatTime::from_quarters(4).expect("valid source length"),
        },
    );
    editor.update(cx, |view, _| view.select_session_scene(first_scene));

    cx.simulate_keystrokes(handle, "cmd-d");
    wait_for_worker(cx, handle, &editor, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.scenes().len() == 3)
    });
    let duplicate_scene = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        let duplicate = state
            .project
            .scenes()
            .iter()
            .find(|scene| scene.id() != first_scene && scene.id() != second_scene)
            .expect("duplicate scene");
        let SessionCellContent::Midi(copy_clip) = state
            .project
            .session_cell(track, duplicate.id())
            .expect("copied midi cell")
        else {
            panic!("duplicate should contain independent midi clip")
        };
        let SessionCellContent::Midi(source_clip) = state
            .project
            .session_cell(track, first_scene)
            .expect("source midi cell")
        else {
            panic!("source should remain a midi clip")
        };
        assert_ne!(source_clip.id(), copy_clip.id());
        assert_eq!(view.selected_scene, Some(duplicate.id()));
        assert_eq!(view.selected_cell, None);
        assert_eq!(view.selected_clip, None);
        duplicate.id()
    });

    cx.simulate_keystrokes(handle, "cmd-alt-up");
    wait_for_worker(cx, handle, &editor, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .scenes()
                    .first()
                    .is_some_and(|scene| scene.id() == duplicate_scene)
            })
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_scene, Some(duplicate_scene));
        assert_eq!(view.selected_cell, None);
        assert_eq!(view.selected_clip, None);
    });

    cx.simulate_keystrokes(handle, "cmd-alt-down");
    wait_for_worker(cx, handle, &editor, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .scenes()
                    .get(1)
                    .is_some_and(|scene| scene.id() == duplicate_scene)
            })
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_scene, Some(duplicate_scene));
        assert_eq!(
            view.state.as_ref().unwrap().project.scenes()[1].id(),
            duplicate_scene
        );
    });
}

#[gpui::test]
fn scene_creation_completion_does_not_change_arrangement_selection(cx: &mut TestAppContext) {
    let (handle, editor) = worker_session_window(cx);
    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::AddMidiTrack {
            name: "instrument".into(),
        },
    );
    let track = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::AddScene {
            name: "first".into(),
        },
    );
    let first_scene = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.scenes()[0].id()
    });

    editor.update(cx, |view, _| {
        let revision = view.state.as_ref().unwrap().revision;
        view.duplicate_scene_if_current(first_scene, revision, view.project_generation);
        assert!(view.pending_scene_duplicate.is_some());
        view.set_mode(EditorMode::Arrangement);
        view.selected_track = Some(track);
        view.selected_scene = Some(first_scene);
    });
    wait_for_worker(cx, handle, &editor, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.scenes().len() == 2)
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.mode, EditorMode::Arrangement);
        assert_eq!(view.selected_track, Some(track));
        assert_eq!(view.selected_scene, Some(first_scene));
        assert_eq!(view.selected_cell, None);
    });
}

#[gpui::test]
fn duplicate_completion_preserves_a_newer_session_selection(cx: &mut TestAppContext) {
    let (handle, editor) = worker_session_window(cx);
    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::AddScene {
            name: "first".into(),
        },
    );
    let source = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.scenes()[0].id()
    });
    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::AddScene {
            name: "second".into(),
        },
    );
    let newer_selection = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.scenes()[1].id()
    });

    editor.update(cx, |view, _| {
        view.select_session_scene(source);
        let revision = view.state.as_ref().unwrap().revision;
        view.duplicate_scene_if_current(source, revision, view.project_generation);
        view.select_session_scene(newer_selection);
    });
    wait_for_worker(cx, handle, &editor, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.scenes().len() == 3)
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.mode, EditorMode::Session);
        assert_eq!(view.selected_scene, Some(newer_selection));
        assert_eq!(view.selected_cell, None);
        assert!(view.pending_scene_duplicate.is_none());
    });
}

#[gpui::test]
fn failed_scene_duplicate_clears_its_pending_selection_intent(cx: &mut TestAppContext) {
    let (handle, editor, _, source) = session_window(cx);
    editor.update(cx, |view, _| {
        view.select_session_scene(source);
        view.duplicate_selected_scene();
        assert!(view.pending_scene_duplicate.is_some());
    });
    wait_for_worker(cx, handle, &editor, |view| {
        view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        assert!(view.pending_scene_duplicate.is_none());
        assert!(view.error.is_some());
        assert!(view.state.as_ref().unwrap().project.scenes().is_empty());
        assert_eq!(view.selected_scene, None);
    });

    submit_edit_and_wait(
        cx,
        handle,
        &editor,
        Command::AddScene {
            name: "later".into(),
        },
    );
    editor.read_with(cx, |view, _| {
        let later = view.state.as_ref().unwrap().project.scenes()[0].id();
        assert_eq!(view.selected_scene, Some(later));
        assert!(view.pending_scene_duplicate.is_none());
    });
}

fn near(left: Pixels, right: Pixels) -> bool {
    (f32::from(left) - f32::from(right)).abs() <= 1.0
}

#[gpui::test]
fn computer_keyboard_dispatch_reserves_notes_and_toggle_without_edit_shortcuts(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, _) = session_window(cx);
    editor.update(cx, |view, _| {
        view.selected_track = Some(view.state.as_ref().unwrap().project.tracks()[0].id());
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("m");
    editor.read_with(&visual, |view, _| assert!(view.computer_keyboard.enabled));
    for key in ["s", "l", "w"] {
        visual.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
            is_held: false,
        });
    }
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.computer_keyboard.held()[1].unwrap().pitch, 61);
        assert_eq!(view.computer_keyboard.held()[2].unwrap().pitch, 62);
        assert_eq!(view.computer_keyboard.held()[14].unwrap().pitch, 74);
        assert!(
            view.last_enqueued_edit.is_none(),
            "s, l and w must remain notes without solo or loop edits"
        );
    });
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("s").unwrap(),
        is_held: true,
    });
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("shift-s").unwrap(),
    });
    editor.read_with(&visual, |view, _| {
        assert!(view.computer_keyboard.held()[2].is_none())
    });
    visual.simulate_keystrokes("m");
    editor.read_with(&visual, |view, _| {
        assert!(!view.computer_keyboard.enabled);
        assert!(view.computer_keyboard.held().iter().all(Option::is_none));
    });
}

#[gpui::test]
fn computer_keyboard_preserves_text_focus_and_cancels_on_focus_loss(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = session_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| {
        window.activate_window();
        editor.update(cx, |view, _| {
            view.selected_track = Some(view.state.as_ref().unwrap().project.tracks()[0].id());
        });
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("m");
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    let lease = editor.read_with(&visual, |view, _| {
        view.computer_keyboard.input.as_ref().unwrap().clone()
    });
    visual.update(|_, cx| {
        editor.update(cx, |view, cx| {
            view.browser.open = true;
            cx.notify();
        })
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.browser.search.read(cx).focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    assert!(
        lease.is_cancelled(),
        "focus loss must immediately cancel the active audio lease"
    );
    editor.read_with(&visual, |view, _| {
        assert!(view.computer_keyboard.held().iter().all(Option::is_none));
        assert!(view.computer_keyboard.suspended);
    });
    visual.simulate_input("asmzcv");
    editor.read_with(&visual, |view, cx| {
        assert!(
            view.computer_keyboard.enabled,
            "typing m must not toggle the keyboard"
        );
        assert_eq!(view.browser.search.read(cx).text(), "asmzcv");
        assert_eq!(
            (
                view.computer_keyboard.base_pitch,
                view.computer_keyboard.velocity
            ),
            (60, 100)
        );
    });
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("a").unwrap(),
        is_held: true,
    });
    editor.read_with(&visual, |view, _| {
        assert!(
            view.computer_keyboard.held()[0].is_none(),
            "focus reset must not retrigger repeat"
        )
    });
}
