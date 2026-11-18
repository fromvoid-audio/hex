//! Editor projection of the audio owner's one-shot read-error observation.
use super::clip_drag_ui_test::editor_window;
use gpui::{TestAppContext, VisualTestContext};
use hex_project::Project;
use std::sync::{Arc, atomic::Ordering::Relaxed};

const MESSAGE: &str = "streamed media read failed · check original media and retry playback";

#[gpui::test]
fn read_error_notice_preserves_priority_consumption_and_fresh_output(cx: &mut TestAppContext) {
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let (handle, editor) = editor_window(
        cx,
        Project::new(48_000).unwrap(),
        Arc::new(Default::default()),
    );
    let (events, receiver) = std::sync::mpsc::channel();
    editor.update(cx, |view, cx| {
        view.audio_events = receiver;
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::VerifiedStreaming,
        });
        view.tick(cx);
        assert!(view.error.is_none());
    });
    let status = backend.status().unwrap();
    status.stream_read_failed.store(true, Relaxed);
    editor.update(cx, |view, cx| {
        view.tick(cx);
        assert_eq!(view.error.as_deref(), Some(MESSAGE));
        assert_eq!(view.status, "streamed media read failed");
        assert!(!view.stream_source_warning);
    });
    assert!(!status.stream_read_failed.load(Relaxed));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("editor-status-message").is_some());
    editor.update(&mut visual, |view, cx| {
        view.status = "saving project".into();
        view.error = Some("another error".into());
        view.tick(cx);
        assert_eq!(view.status, "saving project");
        assert_eq!(view.error.as_deref(), Some("another error"));
    });
    // Higher-priority faults retain the pending observation until it can be
    // displayed. A later read failure can notify independently after a take.
    for (flag, expected) in [
        (&status.device_fault, "audio device failed"),
        (&status.render_fault, "audio render fault"),
        (&status.stream_source_changed, "original media changed"),
    ] {
        flag.store(true, Relaxed);
        status.stream_read_failed.store(true, Relaxed);
        editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            assert!(view.error.as_deref().unwrap().contains(expected));
        });
        assert!(status.stream_read_failed.load(Relaxed));
        flag.store(false, Relaxed);
        editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            assert!(view.error.as_deref().unwrap().contains(MESSAGE));
        });
        assert!(!status.stream_read_failed.load(Relaxed));
    }
    // A fresh output follows the real Opened event path and cannot consume an
    // old output's still-pending observation or retain its old error notice.
    status.stream_read_failed.store(true, Relaxed);
    editor.update(&mut visual, |view, _| {
        view.audio.take().unwrap().control.close()
    });
    drop(output);
    backend.close();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let generation = editor.read_with(&visual, |view, _| view.project_generation);
    events
        .send(crate::EditorAudioEvent::Opened {
            project_generation: generation,
            restart_request: None,
            audio: crate::editor_backend::EditorAudio {
                control: output.take_client().unwrap(),
                sample_rate: 48_000,
                label: "fresh test output".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::VerifiedStreaming,
            },
        })
        .unwrap();
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        assert_eq!(view.status, "audio ready");
        assert!(view.error.is_none());
        view.audio.take().unwrap().control.close();
    });
    drop(output);
    backend.close();
}
