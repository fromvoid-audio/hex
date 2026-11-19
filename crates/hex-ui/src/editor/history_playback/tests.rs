use super::*;
use crate::editor::{
    arrangement_seek::{SeekKind, SeekStage},
    edit_cursor::EditCursor,
};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use std::{sync::Arc, sync::atomic::Ordering::Acquire, time::Instant};

#[gpui::test]
fn play_menu_preserves_history_through_delayed_ack_and_completed_repaints(cx: &mut TestAppContext) {
    let mut fixture = crate::editor::worker_ui_fixture::open(cx, feedback_commands());
    let editor = fixture.editor.clone();
    let project = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let track = project.tracks()[0].id();
    cx.update(crate::editor::bind_editor_keys);
    editor.update(cx, |view, cx| {
        view.edit_cursor = Some(EditCursor {
            track: Some(track),
            frame: 256,
            generation: view.project_generation,
            pulse_color: 0xffffff,
            pulse_started: Instant::now(),
        });
        view.error = None;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.run_until_parked();
    let play = visual.debug_bounds("transport-play-icon").unwrap().center();
    visual.simulate_mouse_down(play, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(play, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.context_menu.as_ref().unwrap().items[0].label,
            "play with history"
        );
        assert!(
            view.prepared_seek.is_none(),
            "opening the menu never starts playback"
        );
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.context_menu.is_none());
        let pending = view.prepared_seek.as_ref().expect("history request queued");
        assert!(matches!(pending.kind, SeekKind::History));
        assert_eq!(pending.destination_frame, 256);
        assert_eq!(
            view.state.as_ref().unwrap().project.as_ref(),
            project.as_ref()
        );
    });
    wait_for_submission(&editor, &mut visual, 256);
    for _ in 0..2 {
        editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(matches!(
                view.prepared_seek.as_ref().unwrap().kind,
                SeekKind::History
            ));
            assert_eq!(view.playback_start_frame(), 256);
            assert_eq!(view.audio_status().unwrap().position.load(Acquire), 0);
        });
    }
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.prepared_seek.is_none(),
            "exact callback acknowledgement retires the intent"
        );
        assert_eq!(view.playback_start_frame(), 256);
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 320);
    });
    // Compare after the existing 256-frame transport declick. This tests the
    // actual UI-to-native history choice; a mislabeled Fresh request diverges.
    for _ in 0..4 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    let frame = editor.read_with(&visual, |view, _| {
        view.audio_status().unwrap().performance_frame.load(Acquire)
    });
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let resources =
        hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
            &hex_engine::native_device_resources::ResidentDeviceAssets::default(),
            &project,
            1 << 20,
            &cancelled,
        )
        .unwrap();
    let graph = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            &project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancelled,
        )
        .unwrap(),
    );
    let mut oracle =
        hex_engine::SessionPlayback::new_with_device_resources(graph, &resources).unwrap();
    oracle
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    for chunk in vec![[0.0; 2]; frame as usize].chunks_mut(127) {
        oracle.render(chunk).unwrap();
    }
    let mut expected = [[0.0; 2]; 64];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.001));
    let mut actual = [[0.0; 2]; 64];
    fixture.backend.render(&mut actual).unwrap();
    assert_eq!(actual, expected, "menu history continuation at {frame}");
    fixture.finish(cx);
}

#[gpui::test]
fn history_menu_refuses_stale_loop_and_work_limit_without_a_request(cx: &mut TestAppContext) {
    let project = hex_project::Project::new(48_000).unwrap();
    let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        let action = MenuAction {
            generation: view.project_generation + 1,
            revision: 1,
            frame: 0,
        };
        view.history_playback_menu_action(action, cx);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("target changed")
        );
        let action = MenuAction {
            generation: view.project_generation,
            revision: 1,
            frame: hex_session::arrangement_seek::MAX_SEEK_WORK_FRAMES + 1,
        };
        view.history_playback_menu_action(action, cx);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("limit exceeded")
        );
        assert!(view.prepared_seek.is_none());
        assert!(view.latched_seek_intent.is_none());
    });
    let mut snapshot = hex_project::Project::new(48_000).unwrap().snapshot();
    snapshot.loop_region = Some(hex_project::LoopRegion::new(0, 512, true).unwrap());
    let (_, loop_editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::from_snapshot(snapshot).unwrap(),
        Arc::default(),
    );
    loop_editor.update(cx, |view, cx| {
        view.history_playback_menu_action(
            MenuAction {
                generation: view.project_generation,
                revision: 1,
                frame: 0,
            },
            cx,
        );
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("loop off")
        );
        assert!(view.prepared_seek.is_none());
        assert!(view.latched_seek_intent.is_none());
    });
}

/// Execute the same finite setup commands in an independent canonical editor to
/// retain its generated IDs, then let the real Service prepare its own resources.
fn feedback_commands() -> Vec<hex_project::Command> {
    use hex_project::{BeatTime, Command, Created};
    use hex_session::device_gesture::{ParameterId, ParameterValue};
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &state,
        ParameterId::new(78).unwrap(),
        ParameterValue::Float(0.3),
    )
    .unwrap();
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let mut commands = Vec::new();
    let mut edit = |command: Command| {
        let result = editor.execute(command.clone()).unwrap();
        commands.push(command);
        result
    };
    let Created::Track(track) = edit(Command::InsertTrackWithDevice {
        name: "feedback".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state,
    }) else {
        panic!("instrument track")
    };
    let Created::Clip(clip) = edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }) else {
        panic!("midi clip")
    };
    edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 100,
    });
    commands
}

fn wait_for_submission(
    editor: &gpui::Entity<crate::editor::EditorView>,
    visual: &mut VisualTestContext,
    destination: u64,
) {
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    loop {
        editor.update(visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        if editor.read_with(visual, |view, _| {
            view.prepared_seek.as_ref().is_some_and(|pending| {
                pending.destination_frame == destination
                    && matches!(pending.stage, SeekStage::Published { .. })
            })
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "history submission unavailable: {:?}",
            editor.read_with(visual, |view, _| (
                view.prepared_seek
                    .as_ref()
                    .map(|p| (p.stage, p.cancelled, p.destination_frame)),
                view.error.as_ref().map(|e| e.to_string()),
                view.status.to_string(),
            ))
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn latest_history_intent_on_an_ordinary_graph_waits_for_prior_then_exact_ack(
    cx: &mut TestAppContext,
) {
    let mut fixture = crate::editor::worker_ui_fixture::open(
        cx,
        vec![hex_project::Command::AddTrack {
            name: "arrangement".into(),
        }],
    );
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    editor.update(&mut visual, |view, _| view.request_history_playback(256));
    wait_for_submission(&editor, &mut visual, 256);
    editor.update(&mut visual, |view, _| {
        view.request_history_playback(512);
        assert!(!view.prepared_seek.as_ref().unwrap().cancelled);
        let latest = view.latched_seek_intent.as_ref().unwrap();
        assert!(matches!(latest.kind, SeekKind::History));
        assert_eq!(latest.destination_frame, 512);
    });
    // An already submitted publication retains custody until its exact ACK;
    // only then may the latest latched request capture the new active fence.
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    wait_for_submission(&editor, &mut visual, 512);
    for _ in 0..2 {
        editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let latest = view.prepared_seek.as_ref().unwrap();
            assert!(matches!(latest.kind, SeekKind::History));
            assert_eq!(latest.destination_frame, 512);
            assert_eq!(view.audio_status().unwrap().position.load(Acquire), 320);
        });
    }
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.prepared_seek.is_none());
        assert!(view.latched_seek_intent.is_none());
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 576);
    });
    fixture.finish(cx);
}

fn open_play_menu(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    let play = visual
        .debug_bounds("transport-play-icon")
        .or_else(|| visual.debug_bounds("transport-pause-icon"))
        .expect("playback options button")
        .center();
    visual.simulate_mouse_down(play, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(play, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
}

fn wait_for_save(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(visual, |view, _| view.playback_settings.saving) {
        visual.run_until_parked();
        assert!(
            Instant::now() < deadline,
            "playback menu save did not finish"
        );
    }
}

#[gpui::test]
fn playback_menu_order_session_availability_and_keyboard_save_keep_history_independent(
    cx: &mut TestAppContext,
) {
    let (handle, editor) = crate::editor::rendered_test_fixture::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    open_play_menu(&mut visual);
    editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert_eq!(
            menu.items.iter().map(|item| item.label).collect::<Vec<_>>(),
            ["play with history", "note chase"]
        );
        assert!(menu.items[1].checked);
        assert_eq!(menu.selected, 0);
    });
    visual.simulate_keystrokes("down enter");
    wait_for_save(&editor, &mut visual);
    editor.update(&mut visual, |view, cx| {
        assert!(!view.playback_settings.preferences.note_chase);
        assert!(
            !hex_preferences::load_playback_preferences(&view.playback_settings.path)
                .unwrap()
                .note_chase
        );
        assert!(view.prepared_seek.is_none());
        assert!(view.latched_seek_intent.is_none());
        view.mode = EditorMode::Session;
        cx.notify();
    });
    open_play_menu(&mut visual);
    editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].label, "note chase");
        assert!(!menu.items[0].checked);
    });
    // Activation uses the same real atomic save owner; failed writes retain
    // both the active checkmark and the user's existing malformed bytes.
    editor.update(&mut visual, |view, _| {
        std::fs::write(&view.playback_settings.path, b"original malformed settings").unwrap();
    });
    visual.simulate_keystrokes("enter");
    wait_for_save(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(!view.playback_settings.preferences.note_chase);
        assert!(view.error.as_ref().unwrap().contains("could not save"));
        assert_eq!(
            std::fs::read(&view.playback_settings.path).unwrap(),
            b"original malformed settings"
        );
    });
}

#[gpui::test]
fn disabled_popup_row_ignores_pointer_enter_and_navigation_skips_it(cx: &mut TestAppContext) {
    let (handle, editor) = crate::editor::rendered_test_fixture::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        view.playback_settings.saving = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    open_play_menu(&mut visual);
    visual.simulate_keystrokes("down");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.context_menu.as_ref().unwrap().selected, 0);
    });
    editor.update(&mut visual, |view, cx| {
        view.context_menu.as_mut().unwrap().selected = 1;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    let position = visual
        .debug_bounds("context-menu-disabled-1")
        .unwrap()
        .center();
    visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(view.playback_settings.preferences.note_chase);
        assert!(view.context_menu.is_some());
        assert!(view.prepared_seek.is_none());
    });
}

#[gpui::test]
fn playback_option_revalidates_stale_and_disabled_actions_and_surfaces_load_errors(
    cx: &mut TestAppContext,
) {
    let (_, editor) = crate::editor::rendered_test_fixture::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        let action = NoteChaseAction {
            generation: view.project_generation,
            enabled: true,
            checked: true,
        };
        view.playback_settings.preferences.note_chase = false;
        view.note_chase_menu_action(action.clone(), cx);
        assert!(!view.playback_settings.saving);
        assert!(!view.playback_settings.preferences.note_chase);
        assert!(view.error.as_ref().unwrap().contains("settings changed"));
        for state in ["loading", "saving", "unwritable"] {
            view.playback_settings.preferences.note_chase = true;
            view.playback_settings.loading = state == "loading";
            view.playback_settings.saving = state == "saving";
            view.playback_settings.writable = state != "unwritable";
            view.playback_settings.error = Some("playback settings could not be read".into());
            view.note_chase_menu_action(
                NoteChaseAction {
                    enabled: false,
                    ..action.clone()
                },
                cx,
            );
            assert!(view.playback_settings.preferences.note_chase);
            assert_eq!(view.playback_settings.saving, state == "saving");
            assert!(view.error.as_ref().unwrap().contains("could not be read"));
        }
        view.playback_settings.loading = false;
        view.playback_settings.saving = false;
        view.playback_settings.writable = true;
        view.project_generation += 1;
        view.note_chase_menu_action(action, cx);
        assert!(!view.playback_settings.saving);
        assert!(view.playback_settings.preferences.note_chase);
    });
}

#[gpui::test]
fn saved_play_menu_toggle_controls_held_midi_on_the_next_arrangement_start(
    cx: &mut TestAppContext,
) {
    let mut fixture = crate::editor::worker_ui_fixture::open_with_section_support(
        cx,
        feedback_commands(),
        false,
        true,
    );
    let editor = fixture.editor.clone();
    cx.update(crate::editor::bind_editor_keys);
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    let frame = 512;
    editor.update(&mut visual, |view, cx| {
        view.play_start_at_beginning = false;
        view.edit_cursor = Some(EditCursor {
            track: None,
            frame,
            generation: view.project_generation,
            pulse_color: 0xffffff,
            pulse_started: Instant::now(),
        });
        cx.notify();
    });
    for enabled in [false, true] {
        open_play_menu(&mut visual);
        visual.simulate_keystrokes("down enter");
        wait_for_save(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let saved =
                hex_preferences::load_playback_preferences(&view.playback_settings.path).unwrap();
            assert_eq!(saved.note_chase, enabled);
            assert_eq!(view.playback_settings.preferences.note_chase, enabled);
            assert!(!view.audio_status().unwrap().playing.load(Acquire));
        });

        visual.simulate_keystrokes("space");
        let mut frames = [[0.0; 2]; 512];
        for block in frames.chunks_exact_mut(64) {
            fixture.backend.render(block).unwrap();
        }
        editor.read_with(&visual, |view, _| {
            let status = view.audio_status().unwrap();
            assert!(status.playing.load(Acquire));
            assert_eq!(status.position.load(Acquire), frame + frames.len() as u64);
        });
        assert_eq!(
            frames.iter().flatten().any(|sample| sample.abs() > 1.0e-6),
            enabled
        );

        if !enabled {
            visual.simulate_keystrokes("space");
            for block in frames.chunks_exact_mut(64) {
                fixture.backend.render(block).unwrap();
            }
            editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                cx.notify();
                assert!(!view.audio_status().unwrap().playing.load(Acquire));
            });
        }
    }
    fixture.finish(cx);
}
