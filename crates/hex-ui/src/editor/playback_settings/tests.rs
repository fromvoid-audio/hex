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
            if cancel == "toggle" {
                assert_eq!(view.status.as_str(), "playback start cancelled");
            }
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

fn feedback_recovery_commands() -> Vec<hex_project::Command> {
    use hex_project::{BeatTime, Command, Created};
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &state,
        hex_lattice_contract::ParameterId::from_static(78),
        hex_lattice_contract::ParameterValue::Float(0.3),
    )
    .unwrap();
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let track_command = Command::InsertTrackWithDevice {
        name: "recovery".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state,
    };
    let Created::Track(track) = editor.execute(track_command.clone()).unwrap() else {
        panic!("instrument track")
    };
    let clip_command = Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    };
    let Created::Clip(clip) = editor.execute(clip_command.clone()).unwrap() else {
        panic!("midi clip")
    };
    vec![
        track_command,
        clip_command,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
    ]
}

#[gpui::test]
fn saved_off_toggle_waits_for_exact_prepared_recovery_ack_before_silent_start(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering::Acquire;
    let mut fixture = super::super::worker_ui_fixture::open(cx, feedback_recovery_commands());
    let editor = fixture.editor.clone();
    let (generation, request, predecessor) = editor.update(cx, |view, cx| {
        reload_off(view, cx);
        let predecessor = view.audio_status().unwrap().applied_audio_source().unwrap();
        assert_eq!(view.current_seek_requires_preroll(), Some(true));
        let generation = view.recovery.begin(129).unwrap();
        assert!(view.recovery.opened(generation));
        assert!(view.recovery.restored(generation));
        view.pending_recovery_generation = Some(generation);
        view.tick(cx);
        let request = view.recovery.prepared_request().unwrap();
        let positioning = view.prepared_seek.as_ref().unwrap();
        assert_eq!(positioning.request_id, request);
        assert_eq!(positioning.recovery_generation, Some(generation));
        assert_eq!(positioning.note_chase, NoteChase::On);
        assert!(!positioning.start_playing);

        view.play_start_at_beginning = false;
        view.edit_cursor = Some(super::super::edit_cursor::EditCursor {
            track: None,
            frame: 512,
            generation: view.project_generation,
            pulse_color: 0xffffff,
            pulse_started: std::time::Instant::now(),
        });
        view.toggle_play();
        assert_eq!(view.prepared_seek.as_ref().unwrap().request_id, request);
        assert_eq!(view.recovery.prepared_request(), Some(request));
        assert!(matches!(
            view.playback_settings.pending.as_ref().unwrap().source,
            StartSource::Recovery(expected) if expected == generation
        ));
        assert!(
            view.deferred_play.is_none(),
            "no raw Play may race the preferences load"
        );
        (generation, request, predecessor)
    });

    finish_load(&editor, cx);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.recovery.generation(), Some(generation));
        assert_eq!(view.recovery.prepared_request(), Some(request));
        let pending = view.playback_settings.pending.as_ref().unwrap();
        assert_eq!(pending.note_chase, Some(NoteChase::Off));
        assert!(
            matches!(pending.source, StartSource::Recovery(expected) if expected == generation)
        );
        let status = view.audio_status().unwrap();
        assert_eq!(status.applied_audio_source(), Some(predecessor));
        assert!(
            !status.playing.load(Acquire),
            "loading alone cannot acknowledge recovery"
        );
        assert_eq!(view.last_transport_command, None);
    });

    let deadline = crate::test_wait::hang_deadline();
    let recovered = loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let recovered = editor.update(cx, |view, cx| {
            view.tick(cx); // Drives the real finish_audio_recovery_tick and its terminal.
            if view.recovery.generation().is_some() {
                return None;
            }
            assert!(view.playback_settings.pending.is_none());
            assert!(view.deferred_play.is_none());
            let status = view.audio_status().unwrap();
            assert!(
                !status.playing.load(Acquire),
                "the transferred command awaits its own callback"
            );
            assert_eq!(status.position.load(Acquire), 129);
            let source = status.applied_audio_source().unwrap();
            assert_ne!(
                source, predecessor,
                "recovery must install its actual fresh runtime"
            );
            assert_eq!(
                view.audible_playback.as_ref().unwrap().seek_request_id,
                Some(request)
            );
            Some(source)
        });
        if let Some(source) = recovered {
            break source;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "prepared recovery did not reach its exact ACK"
        );
        std::thread::yield_now();
    };

    let mut off = Vec::new();
    for _ in 0..8 {
        let mut block = [[0.0; 2]; 64];
        fixture.backend.render(&mut block).unwrap();
        off.extend_from_slice(&block);
    }
    editor.read_with(cx, |view, _| {
        let status = view.audio_status().unwrap();
        assert_eq!(status.applied_audio_source(), Some(recovered));
        assert!(status.playing.load(Acquire));
        assert_eq!(status.position.load(Acquire), 512 + off.len() as u64);
    });
    assert!(
        off.iter().flatten().all(|sample| *sample == 0.0),
        "Off must suppress the held Operator note"
    );

    // A real persisted On choice makes the same held-note fixture audible.
    // This guards the silence assertion against a nonfunctional transport or fixture.
    editor.update(cx, |view, cx| {
        view.stop();
        view.save_note_chase(true, cx);
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    while editor.read_with(cx, |view, _| view.playback_settings.saving) {
        cx.run_until_parked();
        assert!(
            std::time::Instant::now() < deadline,
            "On preference save did not finish"
        );
    }
    editor.update(cx, |view, cx| {
        view.tick(cx);
        assert!(view.playback_settings.preferences.note_chase);
        view.play_start_at_beginning = false;
        view.toggle_play();
    });
    let mut audible = false;
    for _ in 0..8 {
        let mut block = [[0.0; 2]; 64];
        fixture.backend.render(&mut block).unwrap();
        audible |= block.iter().flatten().any(|sample| sample.abs() > 1.0e-6);
    }
    assert!(
        audible,
        "On must chase the same held Operator note after the real save"
    );
    fixture.finish(cx);
}

#[gpui::test]
fn stopped_edit_pending_gesture_is_not_replayed_on_the_installed_successor(
    cx: &mut TestAppContext,
) {
    use hex_project::{BeatTime, Command, ElementColor};
    use std::sync::atomic::Ordering::Acquire;
    let mut fixture = super::super::worker_ui_fixture::open_with_section_support(
        cx,
        vec![
            Command::AddTrack {
                name: "track".into(),
            },
            Command::AddArrangementSection {
                name: "section".into(),
                color: ElementColor::Blue,
                start: BeatTime::ZERO,
                end: BeatTime::ONE,
            },
        ],
        false,
        true,
    );
    let editor = fixture.editor.clone();
    let predecessor = editor.update(cx, |view, cx| {
        reload_off(view, cx);
        let source = view.normal_start_source().unwrap();
        let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
        assert!(
            view.enqueue(hex_session::service::Request::EditArrangementSection {
                project_generation: view.project_generation,
                revision: view.state.as_ref().unwrap().revision,
                command: Command::DuplicateArrangementSection(section),
            })
        );
        source
    });

    let deadline = crate::test_wait::hang_deadline();
    // No callback can acknowledge the real section edit's requested stopped fence yet.
    while !editor.read_with(cx, |view, _| {
        view.audio_status().unwrap().stopped_edit_pending()
    }) {
        assert!(
            std::time::Instant::now() < deadline,
            "section edit did not request a stopped fence"
        );
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        assert!(view.playback_settings.loading);
        assert!(view.normal_start_source().is_none());
        view.queue_applied_source_play(512, predecessor, false);
        assert!(
            view.playback_settings.pending.is_none(),
            "an ineligible gesture must not wait for a new source"
        );
        assert_eq!(view.last_transport_command, None);
    });
    finish_load(&editor, cx);

    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let settled = editor.update(cx, |view, cx| {
            view.tick(cx);
            let status = view.audio_status().unwrap();
            assert!(!status.playing.load(Acquire));
            !status.stopped_edit_pending()
                && status
                    .applied_audio_source()
                    .is_some_and(|source| source != predecessor)
        });
        if settled {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "section successor did not settle"
        );
        std::thread::yield_now();
    }
    editor.read_with(cx, |view, _| {
        assert!(view.playback_settings.pending.is_none());
        assert_eq!(view.last_transport_command, None);
        assert!(view.pending_audio_commands.is_empty());
    });
    fixture.finish(cx);
}

#[gpui::test]
fn toggle_during_loading_cancels_the_future_seek_and_pauses_active_audio(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    let mut fixture = super::super::worker_ui_fixture::open(cx, Vec::new());
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        view.queue_audio_command(PlaybackCommand::Play);
        view.tick(cx);
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.update(cx, |view, cx| {
        view.tick(cx);
        assert!(view.audio_status().unwrap().playing.load(Acquire));
        reload_off(view, cx);
        view.request_prepared_seek(512, true);
        assert!(view.playback_settings.pending.is_some());
        view.toggle_play();
        assert!(view.playback_settings.pending.is_none());
        assert_eq!(view.last_transport_command, Some(PlaybackCommand::Pause));
    });
    finish_load(&editor, cx);
    for _ in 0..8 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.update(cx, |view, cx| view.tick(cx));
    }
    editor.read_with(cx, |view, _| {
        assert!(!view.audio_status().unwrap().playing.load(Acquire));
        assert!(view.prepared_seek.is_none());
        assert!(view.latched_seek_intent.is_none());
    });
    fixture.finish(cx);
}
