//! Desktop composition only: create backend, open the GPUI shell, run lifecycle.
use gpui::{App, Application};

fn main() {
    let mut session_log =
        hex_diagnostics::default_directory().and_then(hex_diagnostics::SessionLog::start);
    let log_handle = session_log
        .as_ref()
        .map(|log| log.handle())
        .map_err(Clone::clone);
    if let Ok(handle) = &log_handle {
        let _ = handle.record(&format!(
            "hex version={}; executable={:?}",
            env!("CARGO_PKG_VERSION"),
            std::env::current_exe()
        ));
    }
    Application::new().run(move |cx: &mut App| {
        hex_ui::install_session_log(cx, log_handle.clone());
        let demo = std::env::args_os()
            .skip(1)
            .any(|argument| argument == "--demo");
        let result = if demo {
            hex_ui::open(cx, hex_audio::AudioOutput::open())
        } else {
            hex_ui::open_editor_pending(
                cx,
                hex_audio::request_initial_open().map_err(|error| error.to_string()),
            )
        };
        if let Err(error) = result {
            eprintln!("{}", error.to_lowercase());
            cx.quit();
            return;
        }
        cx.activate(true);
    });
    // The UI/audio lifecycle has ended; joining file I/O is safe here.
    if let Ok(log) = &mut session_log {
        if let Err(error) = log.shutdown() {
            eprintln!("session logging shutdown failed: {error}");
        }
    }
}
