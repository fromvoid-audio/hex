//! Control-side event forwarding; filesystem ownership stays in hex-diagnostics.
use gpui::{App, AppContext, BorrowAppContext, Global, PromptLevel, Window};
use hex_diagnostics::DiagnosticsHandle;

struct SessionDiagnostics {
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
        handle,
        last_status: String::new(),
        last_error: None,
        reported_failure: false,
        opening_log: false,
    });
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

pub(crate) fn record_state(cx: &mut App, status: &str, error: &Option<String>) -> Option<String> {
    cx.try_global::<SessionDiagnostics>()?;
    cx.update_global::<SessionDiagnostics, _>(|diagnostics, _| {
        let failure = match &diagnostics.handle {
            Ok(handle) => {
                if status != diagnostics.last_status || error != &diagnostics.last_error {
                    let event = format!(
                        "status={status}; error={}",
                        error.as_deref().unwrap_or("none")
                    );
                    if handle.record(&event).is_ok() {
                        diagnostics.last_status = status.to_owned();
                        diagnostics.last_error.clone_from(error);
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
            assert!(record_state(cx, "audio render fault · stopped", &fault).is_none());
            assert!(record_state(cx, "audio render fault · stopped", &fault).is_none());
        });
        log.shutdown().expect("flush log");
        let contents = std::fs::read_to_string(log.path()).expect("read log");
        assert_eq!(contents.matches("invalid event batch").count(), 1);
        assert!(contents.contains("frame 128 · audio rev 3"));
        cx.update(|cx| {
            install_session_log(cx, Err("disk unavailable".to_owned()));
            assert_eq!(
                record_state(cx, "stopped", &fault).as_deref(),
                Some("session logging unavailable: disk unavailable")
            );
            assert!(record_state(cx, "stopped", &fault).is_none());
        });
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }
}
