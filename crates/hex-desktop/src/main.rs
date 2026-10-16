//! Desktop composition only: create backend, open the GPUI shell, run lifecycle.
use gpui::{App, Application};
use std::io::Write;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments
        .next()
        .is_some_and(|argument| argument == "--profile-time-selection")
    {
        let result = (|| -> Result<String, String> {
            let directory = arguments
                .next()
                .ok_or("profiling requires a fixture directory")?;
            if arguments.next().is_some() {
                return Err("profiling accepts exactly one fixture directory".into());
            }
            let directory = std::path::PathBuf::from(directory);
            if !directory.is_absolute() {
                return Err("profiling requires an absolute fixture directory".into());
            }
            let directory = std::fs::canonicalize(directory).map_err(|error| error.to_string())?;
            for name in ["1.hex", "100.hex", "shared.wav"] {
                if !directory.join(name).is_file() {
                    return Err(format!("profiling fixture missing: {name}"));
                }
            }
            let settings = directory.join(format!("native-profile-{}", std::process::id()));
            std::fs::create_dir(&settings).map_err(|error| {
                format!("could not create isolated profiling settings: {error}")
            })?;
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            Application::new().run(move |cx| {
                if let Err(error) =
                    hex_ui::open_time_selection_profile(cx, directory, settings, sender.clone())
                {
                    let _ = sender.try_send(Err(error));
                    cx.quit();
                }
            });
            receiver
                .try_recv()
                .map_err(|_| "profiling window closed before completion; failures=1".to_string())?
        })();
        match result {
            Ok(csv) => {
                if let Err(error) = std::io::stdout().lock().write_all(csv.as_bytes()) {
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "profiling output failed; failures=1: {error}"
                    );
                    std::process::exit(1);
                }
            }
            Err(error) => {
                let _ = writeln!(std::io::stderr().lock(), "{error}");
                std::process::exit(1);
            }
        }
        return;
    }

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
