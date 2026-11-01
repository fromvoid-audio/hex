//! Integration coverage for editor custody of reliable audio outcomes.
use super::clip_drag_ui_test::editor_window;
use gpui::TestAppContext;
use hex_engine::{PlaybackAction, SessionPlaybackCommand, playback::Playback};
use std::sync::Arc;

/// Poll as the editor tick does (Pulse #3857). A receipt drained just before
/// the reader observes retirement leaves the reader retained and one batch
/// waiting for display; polling never advances until that batch is shown. The
/// error keeps the displayed text for the caller's assertions.
fn wait_for_retired_outcomes(cx: &mut TestAppContext, editor: &gpui::Entity<super::EditorView>) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(cx, |view, _| {
            view.poll_retired_action_outcomes();
            view.retired_action_message = None;
            view.retired_action_outcomes.is_none()
        });
        if done {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "output did not retire"
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn editor_reports_each_rejection_and_keeps_reader_through_retirement(cx: &mut TestAppContext) {
    let (_window, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, true).unwrap();
    output
        .publication
        .control()
        .unwrap()
        .publish(Playback::new(
            hex_engine::Project::new(48_000, vec![]).unwrap(),
        ))
        .unwrap();
    let mut client = output.take_client().unwrap();
    assert!(
        client
            .attach_action_outcomes(output.take_action_outcomes().unwrap())
            .is_ok()
    );
    let first = client
        .launch_and_play_with_ticket(SessionPlaybackCommand::Stop)
        .unwrap();
    let second = client
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    backend.render(&mut [[0.0; 2]; 8]).unwrap();
    editor.update(cx, |view, cx| {
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.pending_session_ticket = Some((first, view.project_generation));
        view.pending_metronome_ticket = Some(second);
        view.tick(cx);
        assert_eq!(view.pending_session_ticket, None);
        assert_eq!(view.pending_metronome_ticket, None);
        assert_eq!(view.status.as_str(), "session request rejected");
        let error = view.error.as_ref().expect("rejections displayed");
        assert!(
            error.contains(" · "),
            "both rejections must remain visible: {error}"
        );
        let pending = view
            .audio
            .as_mut()
            .unwrap()
            .control
            .command_with_ticket(hex_engine::PlaybackCommand::Pause)
            .unwrap();
        assert!(pending > second);
        assert!(view.retire_audio_preserving_gain_take());
        assert!(view.retired_action_outcomes.is_some());
    });
    backend.close();
    drop(output);
    wait_for_retired_outcomes(cx, &editor);
    editor.update(cx, |view, _| {
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("previous audio action interrupted")
        )
    });
}

#[gpui::test]
fn occupied_retirement_slot_keeps_the_closed_current_reader(cx: &mut TestAppContext) {
    let (_window, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let (mut old_control, mut old_callback) = hex_audio::prepared::channel(48_000).unwrap();
    let old_reader = old_control
        .enable_action_outcomes_on_control(&mut old_callback)
        .unwrap();
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, true).unwrap();
    output
        .publication
        .control()
        .unwrap()
        .publish(Playback::new(
            hex_engine::Project::new(48_000, vec![]).unwrap(),
        ))
        .unwrap();
    let mut client = output.take_client().unwrap();
    assert!(
        client
            .attach_action_outcomes(output.take_action_outcomes().unwrap())
            .is_ok()
    );
    backend.render(&mut [[0.0; 2]; 8]).unwrap();
    let ticket = client
        .command_with_ticket(hex_engine::PlaybackCommand::Pause)
        .unwrap();
    editor.update(cx, |view, _| {
        view.retired_action_outcomes = Some(old_reader);
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        assert!(!view.retire_audio_preserving_gain_take());
        let retained = view.audio.as_ref().expect("closed client retained");
        assert!(retained.control.status().is_none());
        assert!(retained.control.has_action_outcomes());
        assert!(view.retired_action_outcomes.is_some());
    });
    old_control.request_shutdown();
    old_control
        .retire_action_outcomes_on_control(&mut old_callback)
        .unwrap();
    editor.update(cx, |view, _| {
        view.poll_retired_action_outcomes();
        assert!(view.retired_action_outcomes.is_none());
        assert!(view.retire_audio_preserving_gain_take());
        assert!(view.audio.is_none());
        assert!(view.retired_action_outcomes.is_some());
    });
    backend.close();
    drop(output);
    wait_for_retired_outcomes(cx, &editor);
    editor.update(cx, |view, _| {
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("previous audio action interrupted"),
            "ticket {ticket} must terminate"
        )
    });
}

#[gpui::test]
fn callback_rejection_remains_visible_when_output_is_faulted(cx: &mut TestAppContext) {
    let (_window, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, true).unwrap();
    output
        .publication
        .control()
        .unwrap()
        .publish(Playback::new(
            hex_engine::Project::new(48_000, vec![]).unwrap(),
        ))
        .unwrap();
    let mut client = output.take_client().unwrap();
    assert!(
        client
            .attach_action_outcomes(output.take_action_outcomes().unwrap())
            .is_ok()
    );
    let ticket = client
        .launch_and_play_with_ticket(SessionPlaybackCommand::Stop)
        .unwrap();
    backend.render(&mut [[0.0; 2]; 8]).unwrap();
    backend.report_render_error().unwrap();
    editor.update(cx, |view, cx| {
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.pending_session_ticket = Some((ticket, view.project_generation));
        view.tick(cx);
        assert_eq!(view.pending_session_ticket, None);
        let error = view.error.as_ref().expect("fault and rejection displayed");
        assert!(
            error.contains("session") && error.contains("audio"),
            "fault must not hide rejection: {error}"
        );
        view.tick(cx);
        assert!(view.error.as_ref().unwrap().contains("session"));
    });
}

#[gpui::test]
fn retired_action_interruption_survives_current_output_fault(cx: &mut TestAppContext) {
    let (_window, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let (mut old, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
    let reader = old
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    old.publish(Playback::new(
        hex_engine::Project::new(48_000, vec![]).unwrap(),
    ))
    .unwrap();
    old.command_with_ticket(hex_engine::PlaybackCommand::Pause)
        .unwrap();
    old.request_shutdown();
    old.retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, true).unwrap();
    let mut client = output.take_client().unwrap();
    assert!(
        client
            .attach_action_outcomes(output.take_action_outcomes().unwrap())
            .is_ok()
    );
    backend.report_render_error().unwrap();
    editor.update(cx, |view, cx| {
        view.retired_action_outcomes = Some(reader);
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.tick(cx);
        let error = view.error.as_ref().unwrap();
        assert!(
            error.contains("previous audio action interrupted") && error.contains("audio stopped"),
            "{error}"
        );
        view.tick(cx);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("previous audio action interrupted")
        );
    });
}

#[gpui::test]
fn explicit_editor_play_after_render_fault_does_not_run_failed_graph(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    let (_window, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, true).unwrap();
    let audio = hex_engine::Audio::new(48_000, vec![[0.25; 2]; 256]).unwrap();
    let track = hex_engine::Track::new(
        vec![hex_engine::Clip::new(audio, 0, 256, false).unwrap()],
        1.0,
        false,
    )
    .unwrap();
    output
        .publication
        .control()
        .unwrap()
        .publish(Playback::new(
            hex_engine::Project::new(48_000, vec![track]).unwrap(),
        ))
        .unwrap();
    let mut client = output.take_client().unwrap();
    assert!(
        client
            .attach_action_outcomes(output.take_action_outcomes().unwrap())
            .is_ok()
    );
    backend.render(&mut [[0.0; 2]; 8]).unwrap();
    backend.report_render_error().unwrap();
    editor.update(cx, |view, _| {
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.queue_audio_command(hex_engine::PlaybackCommand::PlayFrom(64));
    });
    let mut frames = [[0.0; 2]; 8];
    backend.render(&mut frames).unwrap();
    let status = backend.status().unwrap();
    assert!(
        !status.playing.load(Acquire),
        "explicit PlayFrom restarted faulted DSP"
    );
    assert_eq!(frames, [[0.0; 2]; 8], "faulted DSP emitted audio");
    assert_eq!(status.recovery_position(), 64);
    editor.update(cx, |view, cx| {
        view.tick(cx);
        assert!(view.error.as_ref().unwrap().contains("audio output failed"));
    });
}
