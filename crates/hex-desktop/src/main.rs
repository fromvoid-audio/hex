//! # Purpose
//! Compose the native editor, worker-owned audio backend and application lifecycle.
//!
//! # Dependencies
//! Uses hex-ui/GPUI, hex-session, hex-audio, hex-engine, hex-stream, hex-media, hex-diagnostics,
//! hex-midi and hex-mts at their existing boundaries. Injects hex-export
//! with independently verified control-worker stream pages for offline output.
//! Reads hex-plugin-catalog's copied source metadata at hex-preferences' existing
//! application-support location; neither grants native source admission.
//!
//! # Does not own
//! Does not implement project edits, codecs or musical DSP. Native source/publication adapters
//! retain exact document and output identity; UI clients never acquire source owners.
use gpui::{App, Application};
use std::io::Write;

mod audio_policy;
mod native_audio;
mod native_sources;
mod streamed_export;
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

    let audio_diagnostics = match audio_diagnostics_flags(std::env::args_os().skip(1)) {
        Ok(flags) => flags,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            std::process::exit(1);
        }
    };

    let mut session_log =
        hex_diagnostics::default_directory().and_then(hex_diagnostics::SessionLog::start);
    let log_handle = session_log
        .as_ref()
        .map(|log| log.handle())
        .map_err(Clone::clone);
    if let Ok(handle) = &log_handle {
        let _ = handle.record(&format!(
            "hex version={}; executable={:?}",
            option_env!("HEX_RELEASE_LABEL").unwrap_or(env!("CARGO_PKG_VERSION")),
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
        hex_ui::install_audio_diagnostics(cx, audio_diagnostics);
        let demo = std::env::args_os()
            .skip(1)
            .any(|argument| argument == "--demo");
        let result = if demo {
            hex_ui::open(cx, hex_audio::AudioOutput::open())
        } else {
            native_audio::backend(audio_diagnostics)
                .and_then(|backend| hex_ui::open_editor_backend(cx, backend))
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

/// Startup-only flag; repeated options deliberately use the last value.
fn audio_diagnostics_flags(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<hex_audio::audio_diagnostics::AudioDiagnosticsFlags, &'static str> {
    use hex_audio::audio_diagnostics::AudioDiagnosticsFlags;
    let mut flags = AudioDiagnosticsFlags::OFF;
    for argument in arguments {
        if argument == "--audio-diagnostics" {
            flags = AudioDiagnosticsFlags::ALL;
        } else if let Some(value) = argument
            .to_str()
            .and_then(|arg| arg.strip_prefix("--audio-diagnostics="))
        {
            flags = value.parse()?;
        }
    }
    Ok(flags)
}

#[cfg(test)]
mod diagnostics_flag_tests {
    use super::*;
    use hex_audio::audio_diagnostics::AudioDiagnosticsFlags as Flags;
    #[test]
    fn tracing_is_opt_in_and_categories_or_explicit_off_survive_other_flags() {
        let parse = |args: &[&str]| {
            audio_diagnostics_flags(args.iter().map(|arg| std::ffi::OsString::from(*arg)))
        };
        assert_eq!(parse(&["--demo"]), Ok(Flags::OFF));
        assert_eq!(parse(&["--audio-diagnostics"]), Ok(Flags::ALL));
        assert_eq!(
            parse(&["--audio-diagnostics", "--audio-diagnostics=off"]),
            Ok(Flags::OFF)
        );
        let flags = parse(&["--demo", "--audio-diagnostics=publication,readiness"]).unwrap();
        assert!(flags.contains(Flags::PUBLICATION));
        assert!(!flags.contains(Flags::TIMING));
        assert!(parse(&["--audio-diagnostics=wat"]).is_err());
    }
}
