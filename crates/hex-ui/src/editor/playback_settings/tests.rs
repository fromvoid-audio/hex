use super::*;
use gpui::TestAppContext;
use hex_engine::{PlaybackAction, PlaybackCommand};
use std::sync::Arc;

fn editor(cx: &mut TestAppContext) -> gpui::Entity<EditorView> {
    super::super::rendered_test_fixture::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    )
    .1
}

fn reload_off(view: &mut EditorView, cx: &mut Context<EditorView>) {
    hex_preferences::save_playback_preferences(
        &view.playback_settings.path,
        PlaybackPreferences { note_chase: false },
    )
    .unwrap();
    view.playback_settings.loading = true;
    view.last_transport_command = None;
    view.start_playback_preferences_load(cx);
}

fn finish_load(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext) {
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(cx, |view, _| view.playback_settings.loading) {
        cx.run_until_parked();
        assert!(
            std::time::Instant::now() < deadline,
            "real playback preferences load did not finish"
        );
    }
}

#[gpui::test]
fn real_loader_keeps_latest_captured_start_and_never_admits_placeholder_on(
    cx: &mut TestAppContext,
) {
    let mut fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        reload_off(view, cx);
        view.edit_cursor = Some(super::super::edit_cursor::EditCursor {
            track: None,
            frame: 123,
            generation: view.project_generation,
            pulse_color: 0xffffff,
            pulse_started: std::time::Instant::now(),
        });
        view.toggle_play();
        let source = view.normal_start_source().unwrap();
        view.queue_applied_source_play(456, source, false);
        assert!(view.playback_settings.pending.is_some());
        assert_eq!(view.last_transport_command, None);
        assert!(view.pending_audio_commands.is_empty());
    });
    finish_load(&editor, cx);
    editor.read_with(cx, |view, _| {
        assert!(!view.playback_settings.preferences.note_chase);
        assert!(view.playback_settings.pending.is_none());
        assert_eq!(
            view.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase {
                frame: 456,
                note_chase: NoteChase::Off
            },)
        );
    });
    for _ in 0..8 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    editor.read_with(cx, |view, _| {
        let status = view.audio_status().unwrap();
        assert!(status.playing.load(std::sync::atomic::Ordering::Acquire));
        assert!(status.position.load(std::sync::atomic::Ordering::Acquire) >= 456);
    });
    fixture.finish(cx);
}

#[gpui::test]
fn loading_start_is_cancelled_by_second_toggle_stop_history_and_raw_action(
    cx: &mut TestAppContext,
) {
    let fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    for cancel in ["toggle", "stop", "history", "raw"] {
        editor.update(cx, |view, cx| {
            reload_off(view, cx);
            view.toggle_play();
            assert!(view.playback_settings.pending.is_some());
            match cancel {
                "toggle" => view.toggle_play(),
                "stop" => view.stop(),
                "history" => view.request_history_playback(0),
                _ => view.queue_audio_command(PlaybackCommand::Seek(0)),
            }
            assert!(view.playback_settings.pending.is_none());
            view.last_transport_command = None;
        });
        finish_load(&editor, cx);
        editor.read_with(cx, |view, _| assert_eq!(view.last_transport_command, None));
    }
    fixture.finish(cx);
}

#[gpui::test]
fn loading_start_refuses_changed_revision_and_real_applied_source_loss(cx: &mut TestAppContext) {
    let first = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = first.editor.clone();
    editor.update(cx, |view, cx| {
        reload_off(view, cx);
        view.queue_normal_play(123);
        Arc::make_mut(view.state.as_mut().unwrap()).revision += 1;
    });
    finish_load(&editor, cx);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.last_transport_command, None);
        assert!(view.error.as_ref().unwrap().contains("target changed"));
    });

    first.finish(cx);
    let mut fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    fixture.editor.update(cx, |view, cx| {
        reload_off(view, cx);
        assert!(view.normal_start_source().is_some());
        view.queue_normal_play(123);
        view.audio = None;
    });
    finish_load(&fixture.editor, cx);
    fixture.editor.read_with(cx, |view, _| {
        assert_eq!(view.last_transport_command, None);
        assert!(view.error.as_ref().unwrap().contains("source changed"));
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    fixture.finish(cx);
}

#[gpui::test]
fn real_save_changes_policy_only_after_success_and_preserves_failed_bytes(cx: &mut TestAppContext) {
    let editor = editor(cx);
    editor.update(cx, |view, cx| {
        assert!(view.playback_settings.preferences.note_chase);
        view.save_note_chase(false, cx);
        assert!(view.playback_settings.saving);
        assert!(view.playback_settings.preferences.note_chase);
    });
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(cx, |view, _| view.playback_settings.saving) {
        cx.run_until_parked();
        assert!(std::time::Instant::now() < deadline);
    }
    editor.update(cx, |view, cx| {
        assert!(!view.playback_settings.preferences.note_chase);
        std::fs::write(&view.playback_settings.path, b"malformed original").unwrap();
        view.save_note_chase(true, cx);
        assert!(!view.playback_settings.preferences.note_chase);
    });
    while editor.read_with(cx, |view, _| view.playback_settings.saving) {
        cx.run_until_parked();
        assert!(std::time::Instant::now() < deadline);
    }
    editor.read_with(cx, |view, _| {
        assert!(!view.playback_settings.preferences.note_chase);
        assert!(view.playback_settings.error.is_some());
        assert_eq!(
            std::fs::read(&view.playback_settings.path).unwrap(),
            b"malformed original"
        );
    });
}

#[gpui::test]
fn malformed_real_load_preserves_bytes_and_drops_waiting_start(cx: &mut TestAppContext) {
    let fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        std::fs::write(&view.playback_settings.path, b"malformed original").unwrap();
        view.playback_settings.loading = true;
        view.last_transport_command = None;
        view.start_playback_preferences_load(cx);
        view.toggle_play();
        assert!(view.playback_settings.pending.is_some());
    });
    finish_load(&editor, cx);
    editor.read_with(cx, |view, _| {
        assert!(!view.playback_settings.writable);
        assert!(view.playback_settings.error.is_some());
        assert!(view.playback_settings.pending.is_none());
        assert_eq!(view.last_transport_command, None);
        assert_eq!(
            std::fs::read(&view.playback_settings.path).unwrap(),
            b"malformed original"
        );
    });
    fixture.finish(cx);
}

#[gpui::test]
fn real_loader_captures_session_scene_launch_policy_before_queueing(cx: &mut TestAppContext) {
    let mut fixture = super::super::worker_ui_fixture::open(
        cx,
        vec![hex_project::Command::AddScene {
            name: "scene".into(),
        }],
    );
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        reload_off(view, cx);
        let state = view.state.as_ref().unwrap();
        let scene = state.project.scenes()[0].id();
        let revision = state.revision;
        view.queue_session_launch_batch(
            hex_engine::SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
            revision,
        );
        assert!(view.playback_settings.pending.is_some());
        assert!(view.pending_audio_commands.is_empty());
    });
    finish_load(&editor, cx);
    editor.update(cx, |view, cx| {
        assert!(view.playback_settings.pending.is_none());
        assert!(matches!(
            view.pending_audio_commands.front().unwrap().action,
            PlaybackAction::LaunchAndPlay(
                hex_engine::SessionPlaybackCommand::LaunchSceneWithNoteChase {
                    note_chase: NoteChase::Off,
                    ..
                }
            ),
        ));
        view.tick(cx);
    });
    for _ in 0..8 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    editor.read_with(cx, |view, _| {
        assert!(
            view.audio_status()
                .unwrap()
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
        );
    });
    fixture.finish(cx);
}

#[gpui::test]
fn plugin_space_uses_saved_off_after_real_load_and_callback(cx: &mut TestAppContext) {
    let mut fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        reload_off(view, cx);
        view.edit_cursor = Some(super::super::edit_cursor::EditCursor {
            track: None,
            frame: 256,
            generation: view.project_generation,
            pulse_color: 0xffffff,
            pulse_started: std::time::Instant::now(),
        });
        let (sender, receiver) = std::sync::mpsc::channel();
        view.audio_events = receiver;
        sender
            .send(crate::EditorAudioEvent::PluginTransportToggle {
                project_generation: view.project_generation,
            })
            .unwrap();
        assert!(view.poll_editor_audio_events(cx));
        assert!(view.playback_settings.pending.is_some());
        assert_eq!(view.last_transport_command, None);
    });
    finish_load(&editor, cx);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase {
                frame: 256,
                note_chase: NoteChase::Off
            }),
        )
    });
    for _ in 0..8 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    editor.read_with(cx, |view, _| {
        let status = view.audio_status().unwrap();
        assert!(status.playing.load(std::sync::atomic::Ordering::Acquire));
        assert!(status.position.load(std::sync::atomic::Ordering::Acquire) >= 256);
    });
    fixture.finish(cx);
}

#[gpui::test]
fn unknown_or_temporarily_ineligible_gesture_never_adopts_a_later_source(cx: &mut TestAppContext) {
    let fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    for blocked in ["missing", "queued", "stopped"] {
        editor.update(cx, |view, cx| {
            reload_off(view, cx);
            assert!(view.normal_start_source().is_some());
            match blocked {
                "missing" => {
                    let output = view.audio.take();
                    view.queue_normal_play(123);
                    view.audio = output;
                }
                "queued" => {
                    view.pending_audio_commands.push_back(
                        super::super::session_grid::PendingPlaybackAction {
                            action: PlaybackAction::SetMetronome(false),
                            generation: view.project_generation,
                            revision: None,
                            intent_token: None,
                        },
                    );
                    view.toggle_play();
                    view.pending_audio_commands.clear();
                }
                _ => {
                    view.stopped_playback_restore =
                        Some(super::super::audio_admission::StoppedPlaybackRestore::Preparing);
                    view.toggle_play();
                    view.stopped_playback_restore = None;
                }
            }
            assert!(view.playback_settings.pending.is_none());
            assert!(
                view.normal_start_source().is_some(),
                "the source becomes eligible later"
            );
        });
        finish_load(&editor, cx);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.last_transport_command, None);
            assert!(view.pending_audio_commands.is_empty());
        });
    }
    fixture.finish(cx);
}

#[gpui::test]
fn waiting_start_cannot_attach_to_a_replacement_recovery_generation(cx: &mut TestAppContext) {
    let fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        reload_off(view, cx);
        let old = view.recovery.begin(123).unwrap();
        view.toggle_play();
        assert!(matches!(view.playback_settings.pending.as_ref().unwrap().source, StartSource::Recovery(generation) if generation == old));
        view.recovery.fail();
        let new = view.recovery.begin(456).unwrap();
        assert_ne!(old, new);
    });
    finish_load(&editor, cx);
    editor.update(cx, |view, _| {
        assert!(view.playback_settings.pending.is_none());
        assert_eq!(view.last_transport_command, None);
        assert!(view.deferred_play.is_none());
        assert!(view.error.as_ref().unwrap().contains("source changed"));
        view.recovery.fail();
    });
    fixture.finish(cx);
}

#[gpui::test]
fn ordinary_fresh_playing_seek_captures_off_while_stopped_positioning_keeps_on(
    cx: &mut TestAppContext,
) {
    let fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        reload_off(view, cx);
        view.request_prepared_seek(256, true);
        assert!(view.playback_settings.pending.is_some());
        assert!(view.prepared_seek.is_none());
    });
    finish_load(&editor, cx);
    editor.read_with(cx, |view, _| {
        let pending = view.prepared_seek.as_ref().unwrap();
        assert_eq!(pending.note_chase, NoteChase::Off);
        assert_eq!(pending.destination_frame, 256);
        assert!(pending.start_playing);
    });
    fixture.finish(cx);
    let fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        reload_off(view, cx);
        view.request_prepared_seek(0, false);
        assert!(view.playback_settings.pending.is_none());
        let positioning = view.prepared_seek.as_ref().unwrap();
        assert_eq!(positioning.note_chase, NoteChase::On);
        assert!(!positioning.start_playing);
    });
    finish_load(&editor, cx);
    fixture.finish(cx);
}
