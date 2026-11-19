use super::*;
use gpui::TestAppContext;
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
        view.queue_normal_play(123);
        view.queue_normal_play(456);
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
    let editor = editor(cx);
    for cancel in ["toggle", "stop", "history", "raw"] {
        editor.update(cx, |view, cx| {
            reload_off(view, cx);
            view.queue_normal_play(123);
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
}

#[gpui::test]
fn loading_start_refuses_changed_revision_and_real_applied_source_loss(cx: &mut TestAppContext) {
    let editor = editor(cx);
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
        assert!(view.error.as_ref().unwrap().contains("target changed"));
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
    let editor = editor(cx);
    editor.update(cx, |view, cx| {
        std::fs::write(&view.playback_settings.path, b"malformed original").unwrap();
        view.playback_settings.loading = true;
        view.last_transport_command = None;
        view.start_playback_preferences_load(cx);
        view.queue_normal_play(123);
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
}
