//! Control-side event forwarding; filesystem ownership stays in hex-diagnostics.
use gpui::{App, BorrowAppContext, Global};
use hex_diagnostics::DiagnosticsHandle;

struct SessionDiagnostics {
    handle: Result<DiagnosticsHandle, String>,
    last_status: String,
    last_error: Option<String>,
    reported_failure: bool,
}
impl Global for SessionDiagnostics {}

/// Inject the desktop-owned session logger without making views own its worker.
pub fn install_session_log(cx: &mut App, handle: Result<DiagnosticsHandle, String>) {
    cx.set_global(SessionDiagnostics {
        handle,
        last_status: String::new(),
        last_error: None,
        reported_failure: false,
    });
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
