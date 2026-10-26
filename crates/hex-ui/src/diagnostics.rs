//! Control-side event forwarding; filesystem ownership stays in hex-diagnostics.
use gpui::{App, AppContext, BorrowAppContext, Global, PromptLevel, Window};
use hex_audio::audio_diagnostics::{AudioDiagnosticsFlags, MAX_DRAIN_PER_POLL};
use hex_diagnostics::DiagnosticsHandle;
use std::sync::atomic::Ordering::Acquire;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AudioSnapshot {
    output: u64,
    applied: u64,
    command: u64,
    stream_applied_command: u64,
    rejected_action: Option<(u64, hex_engine::PlaybackActionRejection)>,
    playing: bool,
    rejected: Option<(u64, hex_engine::PlaybackHandoffRejection)>,
    buffering: bool,
    readiness_refused: bool,
}

struct SessionDiagnostics {
    audio_flags: AudioDiagnosticsFlags,
    audio_snapshot: Option<AudioSnapshot>,
    audio_losses: Option<(u64, u64, u64)>,
    handle: Result<DiagnosticsHandle, String>,
    last_status: String,
    last_error: Option<String>,
    reported_failure: bool,
    opening_log: bool,
}
impl Global for SessionDiagnostics {}

/// Inject the desktop-owned session logger without making views own its worker.
pub fn install_session_log(cx: &mut App, handle: Result<DiagnosticsHandle, String>) {
    cx.set_global(SessionDiagnostics {
        audio_flags: AudioDiagnosticsFlags::OFF,
        audio_snapshot: None,
        audio_losses: None,
        handle,
        last_status: String::new(),
        last_error: None,
        reported_failure: false,
        opening_log: false,
    });
}

/// Select detailed audio tracing after installing the desktop-owned log.
/// Startup-only application configuration; it grants no audio/source authority.
/// `--audio-diagnostics` supplies these flags in the supported desktop entrypoint.
pub fn install_audio_diagnostics(cx: &mut App, flags: AudioDiagnosticsFlags) {
    if cx.try_global::<SessionDiagnostics>().is_none() {
        return;
    }
    let diagnostics = cx.global_mut::<SessionDiagnostics>();
    diagnostics.audio_flags = flags;
    if flags.enabled()
        && let Ok(handle) = &diagnostics.handle
    {
        let _ = handle.record(&format!("audio_diag v=1 stage=configuration build={:?} flags={flags:?} capacity={} drain_budget={} receipt_time=control", crate::build_info::LABEL,
            hex_audio::audio_diagnostics::TRACE_CAPACITY, MAX_DRAIN_PER_POLL));
    }
}

/// Consume actual callback observations on the existing UI/control poll. No
/// native/project owner escapes and a slow log can only lose diagnostics.
pub(crate) fn record_audio(cx: &mut App, audio: Option<&mut crate::EditorAudio>) {
    let Some(config) = cx.try_global::<SessionDiagnostics>() else {
        return;
    };
    let flags = config.audio_flags;
    if !flags.enabled() {
        return;
    }
    let Some(audio) = audio else {
        return;
    };
    audio.control.set_audio_diagnostics(flags);
    let Some(status) = audio.status() else {
        return;
    };
    let output = audio.control.output_identity().unwrap_or(0);
    let snapshot = AudioSnapshot {
        output,
        applied: status.applied_revision.load(Acquire),
        command: status.consumed_command.load(Acquire),
        stream_applied_command: status.stream_applied_command_prefix.load(Acquire),
        rejected_action: status.rejected_action(),
        playing: status.playing.load(Acquire),
        rejected: status.rejected_revision(),
        buffering: status.stream_buffering.load(Acquire),
        readiness_refused: status.stream_readiness_refused.load(Acquire),
    };
    let losses = status.audio_diagnostics_dropped();
    let diagnostics = cx.global_mut::<SessionDiagnostics>();
    let Ok(handle) = &diagnostics.handle else {
        for _ in 0..MAX_DRAIN_PER_POLL {
            if audio.control.take_audio_diagnostic().is_none() {
                break;
            }
        }
        return;
    };
    for _ in 0..MAX_DRAIN_PER_POLL {
        let Some(event) = audio.control.take_audio_diagnostic() else {
            break;
        };
        let _ = handle.record(&format!("audio_diag v=1 stage=callback event={event:?}"));
    }
    if diagnostics.audio_snapshot != Some(snapshot)
        && handle
            .record(&format!(
                "audio_diag v=1 stage=observed_status snapshot={snapshot:?}"
            ))
            .is_ok()
    {
        diagnostics.audio_snapshot = Some(snapshot);
    }
    let lost = (output, losses, handle.overflow_count());
    if diagnostics.audio_losses != Some(lost)
        && handle
            .record(&format!(
                "audio_diag v=1 stage=loss output={output} callback_dropped={} logger_dropped={}",
                lost.1, lost.2
            ))
            .is_ok()
    {
        diagnostics.audio_losses = Some(lost);
    }
}

/// Worker event receipt is distinct from actual callback application. Retain
/// exact resource-free correlation and avoid formatting private paths/errors.
pub(crate) fn record_audio_backend_event(cx: &mut App, event: &crate::EditorAudioEvent) {
    use crate::{EditorAudioEvent as E, EditorPublicationState as P};
    let Some(diagnostics) = cx.try_global::<SessionDiagnostics>() else {
        return;
    };
    if !diagnostics
        .audio_flags
        .contains(AudioDiagnosticsFlags::PUBLICATION)
    {
        return;
    }
    let Ok(handle) = &diagnostics.handle else {
        return;
    };
    let details = match event {
        E::Opened {
            project_generation,
            restart_request,
            audio,
        } => format!(
            "stage=output_opened generation={project_generation} restart={restart_request:?} output={:?} rate={} mode={:?}",
            audio.control.output_identity(),
            audio.sample_rate,
            audio.mode
        ),
        E::Unavailable {
            project_generation,
            restart_request,
            ..
        } => format!(
            "stage=output_unavailable generation={project_generation} restart={restart_request:?}"
        ),
        E::RelinquishClient {
            old_output_identity,
            target,
        } => format!("stage=output_relinquish output={old_output_identity} target={target:?}"),
        E::Publication {
            context,
            output_identity,
            state,
        } => {
            let stage = match state {
                P::Preparing => "preparing",
                P::Busy => "backpressure",
                P::Submitted { .. }
                | P::VerifiedPreviewSubmitted { .. }
                | P::ResidentSubmitted { .. } => "submitted",
                P::Refused(_) => "refused",
                P::Fault(_) => "fault",
            };
            let (revision, preview, seek) = match state {
                P::Submitted { audio_revision } => (Some(*audio_revision), None, None),
                P::VerifiedPreviewSubmitted {
                    audio_revision,
                    identity,
                } => (Some(*audio_revision), Some(*identity), None),
                P::ResidentSubmitted {
                    audio_revision,
                    metadata,
                    ..
                } => (
                    Some(*audio_revision),
                    metadata.device_preview,
                    metadata.seek_request,
                ),
                _ => (None, None, None),
            };
            format!(
                "stage=worker_{stage} output={output_identity} context={context:?} audio_revision={revision:?} preview={preview:?} seek={seek:?}"
            )
        }
    };
    let _ = handle.record(&format!("audio_diag v=1 {details}"));
}

/// Open the active session's latest log without filesystem/process work on the UI thread.
pub(crate) fn open_session_log(window: &mut Window, cx: &mut App) {
    let Some(diagnostics) = cx.try_global::<SessionDiagnostics>() else {
        show_open_error(window, cx, "session logging is unavailable");
        return;
    };
    if diagnostics.opening_log {
        return;
    }
    let path = match &diagnostics.handle {
        Ok(handle) => handle.path(),
        Err(error) => {
            let error = error.clone();
            show_open_error(window, cx, &error);
            return;
        }
    };
    cx.global_mut::<SessionDiagnostics>().opening_log = true;
    let window_handle = window.window_handle();
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move {
                let mut command = std::process::Command::new(if cfg!(target_os = "macos") {
                    "/usr/bin/open"
                } else {
                    "xdg-open"
                });
                if cfg!(target_os = "macos") {
                    // Select the default text editor rather than the .log file association.
                    command.arg("-t");
                }
                let output = command
                    .arg(&path)
                    .output()
                    .map_err(|error| error.to_string())?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "text editor could not open the log ({}): {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ))
                }
            })
            .await;
        let _ = cx.update(|cx| {
            cx.global_mut::<SessionDiagnostics>().opening_log = false;
        });
        if let Err(error) = result {
            let _ = cx.update_window(window_handle, |_, window, cx| {
                show_open_error(window, cx, &error);
            });
        }
    })
    .detach();
}

fn show_open_error(window: &mut Window, cx: &mut App, error: &str) {
    let answer = crate::theme::text("ok");
    let answers: [&str; 1] = [answer.as_ref()];
    drop(window.prompt(
        PromptLevel::Warning,
        &crate::theme::text("could not open session log"),
        Some(&crate::theme::text(error)),
        &answers,
        cx,
    ));
}

pub(crate) fn record_state(cx: &mut App, status: &str, error: Option<&str>) -> Option<String> {
    cx.try_global::<SessionDiagnostics>()?;
    cx.update_global::<SessionDiagnostics, _>(|diagnostics, _| {
        let failure = match &diagnostics.handle {
            Ok(handle) => {
                if status != diagnostics.last_status || error != diagnostics.last_error.as_deref() {
                    let event = format!("status={status}; error={}", error.unwrap_or("none"));
                    if handle.record(&event).is_ok() {
                        diagnostics.last_status = status.to_owned();
                        diagnostics.last_error = error.map(str::to_owned);
                    }
                }
                handle.failure()
            }
            Err(error) => Some(error.clone()),
        };
        if diagnostics.reported_failure {
            None
        } else {
            diagnostics.reported_failure = failure.is_some();
            failure.map(|error| format!("session logging unavailable: {error}"))
        }
    })
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;

    #[gpui::test]
    fn detailed_audio_logs_are_opt_in_correlated_and_category_filtered(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory =
            std::env::temp_dir().join(format!("hex-ui-audio-diag-test-{}", std::process::id()));
        let mut log = hex_diagnostics::SessionLog::start(directory.clone()).unwrap();
        let event = crate::EditorAudioEvent::Publication {
            context: hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 42,
            },
            output_identity: 99,
            state: crate::EditorPublicationState::Submitted {
                audio_revision: 315,
            },
        };
        cx.update(|cx| {
            install_session_log(cx, Ok(log.handle()));
            record_audio_backend_event(cx, &event);
            install_audio_diagnostics(cx, AudioDiagnosticsFlags::TIMING);
            record_audio_backend_event(cx, &event);
            install_audio_diagnostics(cx, AudioDiagnosticsFlags::PUBLICATION);
            record_audio_backend_event(cx, &event);
            record_audio_backend_event(
                cx,
                &crate::EditorAudioEvent::Publication {
                    context: hex_engine::AudioSourceContext {
                        project_generation: 7,
                        editor_revision: 43,
                    },
                    output_identity: 99,
                    state: crate::EditorPublicationState::Refused("private fixture path".into()),
                },
            );
            install_audio_diagnostics(cx, AudioDiagnosticsFlags::OFF);
            record_audio_backend_event(cx, &event);
        });
        log.shutdown().unwrap();
        let contents = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(contents.matches("stage=worker_submitted").count(), 1);
        assert!(contents.contains("output=99"));
        assert!(contents.contains("project_generation: 7, editor_revision: 42"));
        assert!(contents.contains("audio_revision=Some(315)"));
        assert!(contents.contains("stage=worker_refused"));
        assert!(!contents.contains("private fixture path"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn fault_context_reaches_session_log_once_and_failure_is_reported_once(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory =
            std::env::temp_dir().join(format!("hex-ui-session-log-test-{}", std::process::id()));
        let mut log = hex_diagnostics::SessionLog::start(directory.clone()).expect("test log");
        let fault =
            Some("audio stopped · invalid event batch · frame 128 · audio rev 3".to_owned());
        cx.update(|cx| {
            install_session_log(cx, Ok(log.handle()));
            assert!(record_state(cx, "audio render fault · stopped", fault.as_deref()).is_none());
            assert!(record_state(cx, "audio render fault · stopped", fault.as_deref()).is_none());
        });
        log.shutdown().expect("flush log");
        let contents = std::fs::read_to_string(log.path()).expect("read log");
        assert_eq!(contents.matches("invalid event batch").count(), 1);
        assert!(contents.contains("frame 128 · audio rev 3"));
        cx.update(|cx| {
            install_session_log(cx, Err("disk unavailable".to_owned()));
            assert_eq!(
                record_state(cx, "stopped", fault.as_deref()).as_deref(),
                Some("session logging unavailable: disk unavailable")
            );
            assert!(record_state(cx, "stopped", fault.as_deref()).is_none());
        });
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }
}
