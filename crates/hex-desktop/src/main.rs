//! # Purpose
//! Compose the native editor, worker-owned audio backend and application lifecycle.
//!
//! # Dependencies
//! Uses hex-ui/GPUI, hex-session, hex-audio, hex-engine, hex-stream, hex-media, hex-diagnostics,
//! hex-midi and hex-mts at their existing boundaries. Injects hex-export
//! with independently verified control-worker stream pages for offline output.
//! Reads hex-plugin-catalog's copied source metadata at hex-preferences' existing
//! application-support location; neither grants native source admission.
//! Uses fs2, sha2 and nix on control workers for local update custody, receipt identity
//! and bounded owned-helper cancellation; none is used by an audio callback.
//!
//! # Does not own
//! Does not implement project edits, codecs or musical DSP. Native source/publication adapters
//! retain exact document and output identity; UI clients never acquire source owners.
use gpui::{App, Application};
use startup_failure::{Stage, StartupFailure};
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::process::ExitCode;
use std::rc::Rc;

mod audio_policy;
mod native_audio;
mod native_sources;
mod startup_failure;
mod streamed_export;
mod time_selection_profile;
mod update_lease;
mod update_supervisor;

/// Exits 1 after any fatal startup failure, once its alert is dismissed.
fn main() -> ExitCode {
    // Keep shared publication custody until every desktop worker has shut
    // down. Do not pass this file descriptor to any update helper.
    let update_lease = update_lease::acquire_for_current_executable()
        .map_err(StartupFailure::at(Stage::UpdateCustody));

    let mut arguments = std::env::args_os().skip(1);
    if arguments
        .next()
        .is_some_and(|argument| argument == "--profile-time-selection")
    {
        // A terminal-only profiling mode: errors stay on stderr.
        let result = update_lease
            .map_err(|failure| failure.to_string())
            .and_then(|_lease| time_selection_profile::run(arguments));
        if let Err(error) = result {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    let options = diagnostic_options(std::env::args_os().skip(1))
        .map_err(|error| StartupFailure::new(Stage::Arguments, error));
    let (_update_lease, (audio_diagnostics, action_diagnostics)) = match (update_lease, options) {
        (Ok(lease), Ok(options)) => (lease, options),
        (Err(failure), _) | (Ok(_), Err(failure)) => {
            // No worker has started; show the failure instead of vanishing.
            report_before_editor(failure);
            return ExitCode::FAILURE;
        }
    };

    let exit = StartupExit::new(start_session_log(&format!(
        "audio={audio_diagnostics:?} actions={action_diagnostics}"
    )));
    let log_handle = exit.log_handle();
    if let Ok(handle) = &log_handle {
        record_startup_identity(handle);
    }
    let midi_worker = hex_midi::Worker::start();
    let midi_handle = midi_worker.as_ref().ok().map(hex_midi::Worker::handle);
    let mut mts_worker = hex_mts::Worker::start();
    let mts_handle = mts_worker.as_ref().ok().map(hex_mts::Worker::handle);
    let update_client = if _update_lease.is_some() {
        std::env::current_exe()
            .ok()
            .and_then(|executable| std::fs::canonicalize(executable).ok())
            .and_then(|executable| {
                let target = executable.ancestors().nth(3)?.to_path_buf();
                let (client, worker) = hex_ui::update_channel();
                if let Err(error) = update_supervisor::start(target, worker) {
                    eprintln!("local update worker unavailable: {error}");
                    return None;
                }
                Some(client)
            })
    } else {
        None
    };
    let run_exit = Rc::clone(&exit);
    let application = Application::new().with_assets(hex_ui::assets::HexAssets);
    application.run(move |cx: &mut App| {
        run_exit.exit_on_failed_quit(cx);
        if let Some(client) = &update_client {
            hex_ui::install_update_client(cx, client.clone());
        }
        if let Some(handle) = &midi_handle {
            hex_ui::install_controller_input(cx, handle.clone());
        }
        if let Some(handle) = &mts_handle {
            hex_ui::install_mts_publisher(cx, handle.clone());
        }
        hex_ui::install_session_log(cx, log_handle.clone());
        hex_ui::install_audio_diagnostics(cx, audio_diagnostics);
        hex_ui::install_action_diagnostics(cx, action_diagnostics);
        let demo = std::env::args_os()
            .skip(1)
            .any(|argument| argument == "--demo");
        let opened = hex_ui::open_or_report(cx, |cx| {
            if demo {
                hex_ui::open(cx, hex_audio::AudioOutput::open())
                    .map_err(StartupFailure::at(Stage::Editor))
            } else {
                native_audio::backend(audio_diagnostics, log_handle.as_ref().ok().cloned())
                    .and_then(|backend| {
                        hex_ui::open_editor_backend(cx, backend)
                            .map_err(StartupFailure::at(Stage::Editor))
                    })
            }
            .map_err(StartupFailure::into_report)
        });
        run_exit.opened.set(opened);
        if opened {
            cx.activate(true);
        }
    });
    if let Ok(worker) = &mut mts_worker {
        worker.shutdown();
    }
    // The UI/audio lifecycle has ended; joining file I/O is safe here.
    exit.shutdown_log();
    ExitCode::from(exit.code())
}

fn record_startup_identity(handle: &hex_diagnostics::DiagnosticsHandle) {
    let _ = handle.record_typed(
        hex_diagnostics::Level::Info,
        hex_diagnostics::Category::Startup,
        &format!(
            "hex version={}; executable={:?}",
            option_env!("HEX_RELEASE_LABEL").unwrap_or(env!("CARGO_PKG_VERSION")),
            std::env::current_exe()
        ),
    );
}

/// Startup exit status and session-log custody.
///
/// Any run that never opens the editor exits 1. macOS `terminate:` exits the
/// process inside `Application::run` without returning, so a failed startup
/// also flushes the log and exits 1 from the quit observer, once its alert is
/// dismissed. Platforms whose `run` returns use [`StartupExit::code`].
struct StartupExit {
    log: RefCell<Result<hex_diagnostics::SessionLog, String>>,
    /// Set only once the editor (or demo) window opens.
    opened: Cell<bool>,
}

impl StartupExit {
    fn new(log: Result<hex_diagnostics::SessionLog, String>) -> Rc<Self> {
        Rc::new(Self {
            log: RefCell::new(log),
            opened: Cell::new(false),
        })
    }

    fn log_handle(&self) -> Result<hex_diagnostics::DiagnosticsHandle, String> {
        match self.log.try_borrow() {
            Ok(log) => log.as_ref().map(|log| log.handle()).map_err(Clone::clone),
            Err(_) => Err("session log is busy".into()),
        }
    }

    /// Process exit code: 0 only once the editor opened.
    fn code(&self) -> u8 {
        if self.opened.get() { 0 } else { 1 }
    }

    fn shutdown_log(&self) {
        let Ok(mut log) = self.log.try_borrow_mut() else {
            return;
        };
        if let Ok(log) = log.as_mut()
            && let Err(error) = log.shutdown()
        {
            eprintln!("session logging shutdown failed: {error}");
        }
    }

    fn exit_on_failed_quit(self: &Rc<Self>, cx: &mut App) {
        let exit = Rc::clone(self);
        cx.on_app_quit(move |_| {
            if exit.code() != 0 {
                exit.shutdown_log();
                std::process::exit(i32::from(exit.code()));
            }
            async {}
        })
        .detach();
    }
}

/// Starts the session log with the build/platform context plus `context`.
fn start_session_log(context: &str) -> Result<hex_diagnostics::SessionLog, String> {
    hex_diagnostics::default_directory().and_then(|directory| {
        hex_diagnostics::SessionLog::start_with_context(
            directory,
            &format!(
                "build={} platform={}-{} {context}",
                hex_ui::diagnostic_build_identity(),
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        )
    })
}

/// Logs a failure that precedes the editor and shows it in the native startup
/// alert, returning once the alert is dismissed. Starts no worker.
fn report_before_editor(failure: StartupFailure) {
    let exit = StartupExit::new(start_session_log("startup=failed"));
    let log_handle = exit.log_handle();
    let report = failure.into_report();
    let run_exit = Rc::clone(&exit);
    Application::new()
        .with_assets(hex_ui::assets::HexAssets)
        .run(move |cx: &mut App| {
            run_exit.exit_on_failed_quit(cx);
            hex_ui::install_session_log(cx, log_handle.clone());
            hex_ui::open_or_report(cx, |_| Err(report));
        });
    exit.shutdown_log();
}

/// Current development default: recommended failure/action tracing, including
/// optimized builds. Startup-only overrides deliberately use the last value;
/// audio-only flags retain their separate category scope.
fn diagnostic_options(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(hex_audio::audio_diagnostics::AudioDiagnosticsFlags, bool), &'static str> {
    use hex_audio::audio_diagnostics::AudioDiagnosticsFlags;
    let recommended = "publication,transport,readiness".parse::<AudioDiagnosticsFlags>()?;
    let mut flags = recommended;
    let mut actions = true;
    for argument in arguments {
        if argument == "--diagnostics" {
            // Raw timing floods the bounded trace on every callback; request it explicitly.
            flags = recommended;
            actions = true;
        } else if argument == "--diagnostics=off" {
            flags = AudioDiagnosticsFlags::OFF;
            actions = false;
        } else if argument == "--diagnostics=all" {
            flags = AudioDiagnosticsFlags::ALL;
            actions = true;
        } else if argument
            .to_str()
            .is_some_and(|arg| arg.starts_with("--diagnostics="))
        {
            return Err("use --diagnostics, --diagnostics=all or --diagnostics=off");
        } else if argument == "--audio-diagnostics" {
            flags = AudioDiagnosticsFlags::ALL;
        } else if let Some(value) = argument
            .to_str()
            .and_then(|arg| arg.strip_prefix("--audio-diagnostics="))
        {
            flags = value.parse()?;
        }
    }
    Ok((flags, actions))
}

#[cfg(test)]
mod startup_exit_tests {
    use super::{StartupExit, record_startup_identity};

    #[test]
    fn startup_identity_is_classified_in_the_session_log() {
        let dir =
            std::env::temp_dir().join(format!("hex-startup-classification-{}", std::process::id()));
        let mut log = hex_diagnostics::SessionLog::start(dir.clone()).unwrap();
        record_startup_identity(&log.handle());
        log.shutdown().unwrap();
        let contents = std::fs::read_to_string(log.path()).unwrap();
        assert!(
            contents
                .lines()
                .any(|line| line.contains("level=info category=startup")
                    && line.contains("hex version="))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_exits_nonzero_unless_the_editor_opened() {
        let exit = StartupExit::new(Err("session logging is unavailable".into()));
        assert_eq!(exit.code(), 1, "an alerted or interrupted startup fails");
        exit.opened.set(true);
        assert_eq!(exit.code(), 0);
        // Shutdown without a log, and repeated shutdown, are harmless.
        exit.shutdown_log();
        exit.shutdown_log();
        assert!(exit.log_handle().is_err());
    }
}

#[cfg(test)]
mod diagnostics_flag_tests {
    use super::*;
    use hex_audio::audio_diagnostics::AudioDiagnosticsFlags as Flags;
    #[test]
    fn ordinary_development_startup_enables_failure_and_action_diagnostics() {
        for args in [vec![], vec!["--demo"]] {
            let (flags, actions) =
                diagnostic_options(args.into_iter().map(std::ffi::OsString::from)).unwrap();
            assert!(actions);
            assert!(flags.contains(Flags::PUBLICATION));
            assert!(flags.contains(Flags::TRANSPORT));
            assert!(flags.contains(Flags::READINESS));
            assert!(!flags.contains(Flags::TIMING));
        }
    }

    #[test]
    fn audio_categories_and_explicit_off_survive_other_flags() {
        let parse = |args: &[&str]| {
            diagnostic_options(args.iter().map(|arg| std::ffi::OsString::from(*arg)))
                .map(|options| options.0)
        };
        assert_eq!(parse(&["--audio-diagnostics"]), Ok(Flags::ALL));
        assert_eq!(
            parse(&["--audio-diagnostics", "--audio-diagnostics=off"]),
            Ok(Flags::OFF)
        );
        let flags = parse(&["--demo", "--audio-diagnostics=publication,readiness"]).unwrap();
        assert!(flags.contains(Flags::PUBLICATION));
        assert!(!flags.contains(Flags::TIMING));
        assert!(parse(&["--audio-diagnostics=wat"]).is_err());
        let options = diagnostic_options(["--diagnostics".into()].into_iter()).unwrap();
        assert!(options.1);
        assert!(options.0.contains(Flags::READINESS));
        assert!(!options.0.contains(Flags::TIMING));
        assert_eq!(
            diagnostic_options(["--diagnostics".into(), "--diagnostics=off".into()].into_iter()),
            Ok((Flags::OFF, false))
        );
        assert_eq!(
            diagnostic_options(["--diagnostics=off".into()].into_iter()),
            Ok((Flags::OFF, false))
        );
        assert_eq!(
            diagnostic_options(["--audio-diagnostics=off".into()].into_iter()),
            Ok((Flags::OFF, true))
        );
        assert_eq!(
            diagnostic_options(["--diagnostics=off".into(), "--diagnostics".into()].into_iter()),
            Ok(("publication,transport,readiness".parse().unwrap(), true))
        );
        assert!(diagnostic_options(["--diagnostics=wat".into()].into_iter()).is_err());
    }
}
