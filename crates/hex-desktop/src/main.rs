//! Desktop composition only: create backend, open the GPUI shell, run lifecycle.
use gpui::{App, Application};
use std::io::Write;

mod audio_policy;
mod native_audio;
mod native_sources;
mod time_selection_profile;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments
        .next()
        .is_some_and(|argument| argument == "--profile-time-selection")
    {
        if let Err(error) = time_selection_profile::run(arguments) {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            std::process::exit(1);
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
    let midi_worker = hex_midi::Worker::start();
    let midi_handle = midi_worker.as_ref().ok().map(hex_midi::Worker::handle);
    let mut mts_worker = hex_mts::Worker::start();
    let mts_handle = mts_worker.as_ref().ok().map(hex_mts::Worker::handle);
    Application::new().run(move |cx: &mut App| {
        if let Some(handle) = &midi_handle {
            hex_ui::install_controller_input(cx, handle.clone());
        }
        if let Some(handle) = &mts_handle {
            hex_ui::install_mts_publisher(cx, handle.clone());
        }
        hex_ui::install_session_log(cx, log_handle.clone());
        let demo = std::env::args_os()
            .skip(1)
            .any(|argument| argument == "--demo");
        let result = if demo {
            hex_ui::open(cx, hex_audio::AudioOutput::open())
        } else {
            native_audio::backend().and_then(|backend| hex_ui::open_editor_backend(cx, backend))
        };
        if let Err(error) = result {
            eprintln!("{}", error.to_lowercase());
            cx.quit();
            return;
        }
        cx.activate(true);
    });
    if let Ok(worker) = &mut mts_worker {
        worker.shutdown();
    }
    // The UI/audio lifecycle has ended; joining file I/O is safe here.
    if let Ok(log) = &mut session_log {
        if let Err(error) = log.shutdown() {
            eprintln!("session logging shutdown failed: {error}");
        }
    }
}
