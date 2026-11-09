//! Lowercase status text for an output open that produced no audio.
//!
//! A device/project sample-rate mismatch is recoverable: nothing is lost and
//! the same project plays once the device runs at the project rate, so it is
//! shown as a paused state naming both rates in hz and the retry action,
//! distinct from the generic "audio unavailable" failure. Rendering goes
//! through the existing status line and `theme::text`.

use hex_audio::{OutputOpenError, RateMismatch};

/// Status and error text for a refused output open.
pub(super) fn unavailable_message(error: &OutputOpenError) -> String {
    match error {
        OutputOpenError::RateMismatch(mismatch) => rate_mismatch_message(*mismatch),
        OutputOpenError::Failed(error) => {
            format!("audio unavailable · {}", error.to_lowercase())
        }
    }
}

fn rate_mismatch_message(mismatch: RateMismatch) -> String {
    format!(
        "audio paused · output runs at {} hz, project needs {} hz · set the device to {} hz, then retry audio",
        mismatch.device_hz, mismatch.project_hz, mismatch.project_hz
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_mismatch_reads_as_a_lowercase_recoverable_pause() {
        let mismatch = OutputOpenError::RateMismatch(RateMismatch {
            device_hz: 44_100,
            project_hz: 48_000,
        });
        let message = unavailable_message(&mismatch);
        assert_eq!(
            message,
            "audio paused · output runs at 44100 hz, project needs 48000 hz · set the device to 48000 hz, then retry audio"
        );
        assert_eq!(message, message.to_lowercase());
        assert_eq!(
            unavailable_message(&"No Default Output".into()),
            "audio unavailable · no default output"
        );
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests {
    use super::*;
    use crate::EditorAudioEvent;
    use std::sync::{Arc, mpsc};

    #[gpui::test]
    fn rate_mismatch_event_shows_the_recoverable_paused_status(cx: &mut gpui::TestAppContext) {
        let (project, ..) = super::super::clip_drag_ui_test::audio_project();
        let (_, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            project,
            Arc::new(hex_engine::MidiArrangementPreview::default()),
        );
        let (events, receiver) = mpsc::sync_channel(1);
        events
            .send(EditorAudioEvent::Unavailable {
                project_generation: 0,
                restart_request: None,
                error: RateMismatch {
                    device_hz: 44_100,
                    project_hz: 48_000,
                }
                .into(),
            })
            .unwrap();
        editor.update(cx, |view, cx| {
            view.project_generation = 0;
            view.pending_restart_request = None;
            view.audio_events = receiver;
            view.tick(cx);
            let expected = "audio paused · output runs at 44100 hz, project needs 48000 hz · set the device to 48000 hz, then retry audio";
            assert_eq!(view.status, expected);
            assert_eq!(view.error.as_deref(), Some(expected));
            assert!(view.audio.is_none());
        });
    }
}
