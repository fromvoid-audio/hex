//! Visible startup failure reporting. A Finder-launched bundle has no visible
//! stderr, so a failed asset/font check or editor open is recorded in the
//! session log and shown as a native alert before the application quits.
//! GPUI alerts attach to a window, so the report opens a small blank window to
//! carry it; text inside that window is never rendered, keeping it independent
//! of the font check that may have failed. Error text is shown verbatim because
//! it may carry case-sensitive paths; the host supplies the recovery hint for
//! the step that failed, shown lowercase beneath it.
use gpui::{
    App, AppContext, Bounds, PromptLevel, Render, WindowBounds, WindowOptions, div, px, size,
};

/// Run a startup step. On failure `(error, recovery hint)`, log the error, show
/// both in a native alert and quit once it is dismissed. Returns whether the
/// step succeeded.
pub fn open_or_report(
    cx: &mut App,
    open: impl FnOnce(&mut App) -> Result<(), (String, &'static str)>,
) -> bool {
    match open(cx) {
        Ok(()) => true,
        Err((error, recovery)) => {
            report(cx, &error, recovery, |cx| cx.quit());
            false
        }
    }
}

struct Blank;
impl Render for Blank {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
    }
}

fn report(cx: &mut App, error: &str, recovery: &str, quit: impl FnOnce(&mut App) + 'static) {
    // Shown and logged verbatim; only the verbatim-span marks are removed.
    let error = &hex_session::service::text::plain(error);
    eprintln!("{error}");
    eprintln!("{recovery}");
    if let Some(unavailable) = crate::diagnostics::record_state(cx, "startup failed", Some(error)) {
        eprintln!("{unavailable}");
    }
    let log = crate::diagnostics::log_path(cx).map_or_else(
        || crate::theme::text("session logging is unavailable").to_string(),
        |path| {
            format!(
                "{} {}",
                crate::theme::text("details are in the session log:"),
                path.display()
            )
        },
    );
    let detail = format!("{error}\n\n{}\n{log}", crate::theme::text(recovery));
    let bounds = Bounds::centered(None, size(px(420.0), px(160.0)), cx);
    let window = cx.open_window(
        WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("hex".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        },
        |_, cx| cx.new(|_| Blank),
    );
    let answer = window.ok().and_then(|window| {
        window
            .update(cx, |_, window, cx| {
                let quit_label = crate::theme::text("quit");
                window.prompt(
                    PromptLevel::Critical,
                    &crate::theme::text("hex could not start"),
                    Some(&detail),
                    &[quit_label.as_ref()],
                    cx,
                )
            })
            .ok()
    });
    let Some(answer) = answer else {
        // Without a window there is nowhere to attach the alert; stderr and the log keep the error.
        quit(cx);
        return;
    };
    cx.activate(true);
    cx.spawn(async move |cx| {
        let _ = answer.await;
        let _ = cx.update(quit);
    })
    .detach();
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[gpui::test]
    fn failed_startup_is_logged_alerted_and_quits_after_dismissal(cx: &mut gpui::TestAppContext) {
        let directory = std::env::temp_dir().join(format!(
            "hex-ui-startup-failure-test-{}",
            std::process::id()
        ));
        let mut log = hex_diagnostics::SessionLog::start(directory.clone()).expect("test log");
        let quit = Rc::new(Cell::new(false));
        cx.update(|cx| {
            crate::install_session_log(cx, Ok(log.handle()));
            // The test application installs no embedded assets, so the real check fails.
            let error = crate::assets::validate(cx).expect_err("assets are absent");
            let quit = quit.clone();
            report(cx, &error, "reinstall hex.", move |_| quit.set(true));
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        assert!(!quit.get(), "quit waits for the alert");
        cx.simulate_prompt_answer("quit");
        cx.run_until_parked();
        assert!(quit.get());
        log.shutdown().expect("flush log");
        let contents = std::fs::read_to_string(log.path()).expect("read log");
        assert!(contents.contains("startup failed"));
        assert!(contents.contains("embedded icon"));
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[gpui::test]
    fn successful_startup_reports_nothing(cx: &mut gpui::TestAppContext) {
        assert!(cx.update(|cx| open_or_report(cx, |_| Ok(()))));
        assert!(!cx.has_pending_prompt());
    }
}
