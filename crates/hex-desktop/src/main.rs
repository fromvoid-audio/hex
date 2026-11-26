//! # Purpose
//! Compose the native editor, worker-owned audio backend and application lifecycle.
//!
//! # Dependencies
//! Uses hex-ui/GPUI, hex-session, hex-audio, hex-engine, hex-stream, hex-media, hex-diagnostics,
//! hex-report, hex-midi and hex-mts at their existing boundaries. Injects hex-export
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
#[cfg(feature = "profiling-tools")]
use std::io::Write;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

mod audio_policy;
mod granular_sources;
mod native_audio;
mod native_sources;
mod report_worker;
mod startup_failure;
mod streamed_export;
#[cfg(feature = "profiling-tools")]
mod time_selection_profile;
mod update_lease;
mod update_supervisor;

const DESKTOP_BUILD_IDENTITY: hex_ui::BuildIdentity = hex_ui::BuildIdentity::new(
    env!("HEX_DISPLAY_VERSION"),
    env!("HEX_BUILD_IDENTITY"),
    env!("HEX_DIAGNOSTIC_SOURCE"),
);

pub(crate) fn desktop_diagnostic_build_identity() -> &'static str {
    hex_ui::install_build_identity(DESKTOP_BUILD_IDENTITY);
    hex_ui::diagnostic_build_identity()
}

/// Exits 1 after any fatal startup failure, once its alert is dismissed.
fn main() -> ExitCode {
    hex_ui::install_build_identity(DESKTOP_BUILD_IDENTITY);
    // Keep shared publication custody until every desktop worker has shut
    // down. Do not pass this file descriptor to any update helper.
    let update_lease = update_lease::acquire_for_current_executable()
        .map_err(StartupFailure::at(Stage::UpdateCustody));

    #[cfg(feature = "profiling-tools")]
    {
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
    }

    let options = diagnostic_options(std::env::args_os().skip(1))
        .map_err(|error| StartupFailure::new(Stage::Arguments, error));
    let (update_lease, (audio_diagnostics, action_diagnostics)) = match (update_lease, options) {
        (Ok(lease), Ok(options)) => (lease, options),
        (Err(failure), _) | (Ok(_), Err(failure)) => {
            // No worker has started; show the failure instead of vanishing.
            report_before_editor(failure);
            return ExitCode::FAILURE;
        }
    };

    let skip_pending_update = skip_pending_update_flag(std::env::args_os().skip(1));
    let demo = std::env::args_os()
        .skip(1)
        .any(|argument| argument == "--demo");
    if !demo
        && let Some(lease) = &update_lease
        && let Some(target) = current_bundle_target()
    {
        match update_supervisor::try_startup_pending_handoff(
            &target,
            lease.is_sole_instance(),
            skip_pending_update,
        ) {
            update_supervisor::StartupHandoffOutcome::HandedOff => {
                // Release shared publication custody before exiting cleanly so
                // the verified update helper can publish and reopen the new build.
                drop(update_lease);
                return ExitCode::SUCCESS;
            }
            update_supervisor::StartupHandoffOutcome::Failed(error) => {
                eprintln!("startup pending update handoff skipped: {error}");
            }
            update_supervisor::StartupHandoffOutcome::SkippedByFlag
            | update_supervisor::StartupHandoffOutcome::OtherInstanceRunning
            | update_supervisor::StartupHandoffOutcome::NoPendingUpdate => {}
        }
    }

    let exit = StartupExit::new(start_session_log(&format!(
        "audio={audio_diagnostics:?} actions={action_diagnostics}"
    )));
    install_panic_hook(Arc::clone(&exit.log));
    let log_handle = exit.log_handle();
    if let Ok(handle) = &log_handle {
        record_startup_identity(handle);
    }
    let midi_worker = match hex_midi::Worker::start() {
        Ok(worker) => Some(worker),
        Err(error) => {
            record_startup_warning(
                log_handle.as_ref().ok(),
                &format!("midi worker unavailable: {error}"),
            );
            None
        }
    };
    let midi_handle = midi_worker.as_ref().map(hex_midi::Worker::handle);
    let mts_worker = match hex_mts::Worker::start() {
        Ok(worker) => Some(worker),
        Err(error) => {
            record_startup_warning(
                log_handle.as_ref().ok(),
                &format!("mts-esp worker unavailable: {error}"),
            );
            None
        }
    };
    let mts_handle = mts_worker.as_ref().map(hex_mts::Worker::handle);
    let (update_client, update_supervisor) = if update_lease.is_some() {
        match std::env::current_exe()
            .ok()
            .and_then(|executable| std::fs::canonicalize(executable).ok())
            .and_then(|executable| {
                let target = executable.ancestors().nth(3)?.to_path_buf();
                let (client, worker) = hex_ui::update_channel();
                match update_supervisor::start(target, worker) {
                    Ok(handle) => Some((client, handle)),
                    Err(error) => {
                        record_startup_warning(
                            log_handle.as_ref().ok(),
                            &format!("local update worker unavailable: {error}"),
                        );
                        eprintln!("local update worker unavailable: {error}");
                        None
                    }
                }
            }) {
            Some((client, handle)) => (Some(client), Some(handle)),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    let (report_client, report_worker_handle) = {
        let (client, worker) = hex_ui::report_channel();
        match report_worker::start(worker) {
            Ok(handle) => (Some(client), Some(handle)),
            Err(error) => {
                record_startup_warning(
                    log_handle.as_ref().ok(),
                    &format!("issue report worker unavailable: {error}"),
                );
                (None, None)
            }
        }
    };
    exit.attach_runtime(DesktopRuntime {
        update_supervisor,
        report_worker: report_worker_handle,
        mts_worker,
        midi_worker,
        update_lease,
    });
    let run_exit = Rc::clone(&exit);
    let application = Application::new().with_assets(hex_ui::assets::HexAssets);
    application.run(move |cx: &mut App| {
        run_exit.install_quit_teardown(cx);
        if let Some(client) = &update_client {
            hex_ui::install_update_client(cx, client.clone());
        }
        if let Some(client) = &report_client {
            hex_ui::install_report_client(cx, client.clone());
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
    // The UI/audio lifecycle has ended; join workers, flush the session log,
    // and release shared publication custody in deterministic order.
    exit.shutdown_all();
    ExitCode::from(exit.code())
}

fn record_startup_identity(handle: &hex_diagnostics::DiagnosticsHandle) {
    let _ = handle.record_typed(
        hex_diagnostics::Level::Info,
        hex_diagnostics::Category::Startup,
        &format!(
            "hex version={}; executable={:?}",
            env!("HEX_DISPLAY_VERSION"),
            std::env::current_exe()
        ),
    );
}

fn record_startup_warning(handle: Option<&hex_diagnostics::DiagnosticsHandle>, message: &str) {
    if let Some(handle) = handle {
        let _ = handle.record_typed(
            hex_diagnostics::Level::Warn,
            hex_diagnostics::Category::Startup,
            message,
        );
    }
}

fn format_panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = if let Some(text) = info.payload().downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = info.payload().downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_owned()
    };
    match info.location() {
        Some(location) => format!(
            "panic at {}:{}:{}: {payload}",
            location.file(),
            location.line(),
            location.column()
        ),
        None => format!("panic at unknown location: {payload}"),
    }
}

fn record_panic_and_flush_log(
    log: &Mutex<Result<hex_diagnostics::SessionLog, String>>,
    message: &str,
) {
    // Use try_lock so a panic originating while holding the log mutex never
    // deadlocks the panicking thread.
    let Ok(mut guard) = log.try_lock() else {
        return;
    };
    if let Ok(session_log) = guard.as_mut() {
        let _ = session_log.handle().record_typed(
            hex_diagnostics::Level::Error,
            hex_diagnostics::Category::Uncategorised,
            message,
        );
        let _ = session_log.shutdown();
    }
}

fn install_panic_hook(log: Arc<Mutex<Result<hex_diagnostics::SessionLog, String>>>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = format_panic_message(info);
        record_panic_and_flush_log(&log, &message);
        previous(info);
    }));
}

/// Desktop-owned workers and shared publication lease torn down in strict order:
/// every background worker joins before the update lease is released.
#[derive(Default)]
struct DesktopRuntime {
    update_supervisor: Option<update_supervisor::UpdateSupervisorHandle>,
    report_worker: Option<report_worker::ReportWorkerHandle>,
    mts_worker: Option<hex_mts::Worker>,
    midi_worker: Option<hex_midi::Worker>,
    update_lease: Option<update_lease::UpdateLease>,
}

/// Startup exit status, worker teardown, and session-log custody.
///
/// Any run that never opens the editor exits 1. macOS `terminate:` exits the
/// process inside `Application::run` without returning, so the quit observer
/// always shuts down every desktop worker, flushes the session log, and drops
/// the update lease in custody order on **all** exits (both normal quit
/// `code() == 0` and failed startup `code() != 0`). Platforms whose `run`
/// returns call the same idempotent teardown and exit with [`StartupExit::code`].
struct StartupExit {
    log: Arc<Mutex<Result<hex_diagnostics::SessionLog, String>>>,
    runtime: RefCell<Option<DesktopRuntime>>,
    /// Set only once the editor (or demo) window opens.
    opened: Cell<bool>,
}

impl StartupExit {
    fn new(log: Result<hex_diagnostics::SessionLog, String>) -> Rc<Self> {
        Rc::new(Self {
            log: Arc::new(Mutex::new(log)),
            runtime: RefCell::new(None),
            opened: Cell::new(false),
        })
    }

    fn attach_runtime(&self, runtime: DesktopRuntime) {
        if let Ok(mut slot) = self.runtime.try_borrow_mut() {
            *slot = Some(runtime);
        }
    }

    fn log_handle(&self) -> Result<hex_diagnostics::DiagnosticsHandle, String> {
        match self.log.try_lock() {
            Ok(log) => log.as_ref().map(|log| log.handle()).map_err(Clone::clone),
            Err(_) => Err("session log is busy".into()),
        }
    }

    /// Process exit code: 0 only once the editor opened.
    fn code(&self) -> u8 {
        if self.opened.get() { 0 } else { 1 }
    }

    fn shutdown_log(&self) {
        let Ok(mut log) = self.log.lock() else {
            return;
        };
        if let Ok(log) = log.as_mut()
            && let Err(error) = log.shutdown()
        {
            eprintln!("session logging shutdown failed: {error}");
        }
    }

    /// Shut down desktop workers, session log, and update lease in strict custody
    /// order so no worker runs after shared publication custody is released.
    fn shutdown_all(&self) {
        self.shutdown_all_observed(|_| {});
    }

    fn shutdown_all_observed(&self, mut observe: impl FnMut(&'static str)) {
        let runtime = self
            .runtime
            .try_borrow_mut()
            .ok()
            .and_then(|mut slot| slot.take());
        let update_lease = if let Some(mut runtime) = runtime {
            if let Some(mut supervisor) = runtime.update_supervisor.take() {
                let _ = supervisor.shutdown();
                drop(supervisor);
                observe("update_supervisor");
            }
            if let Some(mut report) = runtime.report_worker.take() {
                let _ = report.shutdown();
                drop(report);
                observe("report_worker");
            }
            if let Some(mut mts) = runtime.mts_worker.take() {
                mts.shutdown();
                drop(mts);
                observe("mts_worker");
            }
            if let Some(midi) = runtime.midi_worker.take() {
                drop(midi);
                observe("midi_worker");
            }
            runtime.update_lease.take()
        } else {
            None
        };
        self.shutdown_log();
        observe("session_log");
        if let Some(lease) = update_lease {
            drop(lease);
            observe("update_lease");
        }
    }

    /// Perform ordered teardown on application quit; returns the non-zero exit
    /// code that failed-startup quit must report once teardown is complete.
    fn prepare_quit_exit(&self) -> Option<i32> {
        self.shutdown_all();
        let code = self.code();
        (code != 0).then(|| i32::from(code))
    }

    /// Run ordered desktop teardown on all macOS exits inside GPUI's
    /// `on_app_quit` observer (including normal quit `code() == 0`, because
    /// `NSApplication terminate:` exits inside `Application::run` without
    /// returning), and exit non-zero if startup failed.
    fn install_quit_teardown(self: &Rc<Self>, cx: &mut App) {
        let exit = Rc::clone(self);
        cx.on_app_quit(move |_| {
            if let Some(code) = exit.prepare_quit_exit() {
                std::process::exit(code);
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
                desktop_diagnostic_build_identity(),
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
            run_exit.install_quit_teardown(cx);
            hex_ui::install_session_log(cx, log_handle.clone());
            hex_ui::open_or_report(cx, |_| Err(report));
        });
    exit.shutdown_log();
}

fn current_bundle_target() -> Option<std::path::PathBuf> {
    let executable = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    Some(executable.ancestors().nth(3)?.to_path_buf())
}

fn skip_pending_update_flag(arguments: impl Iterator<Item = std::ffi::OsString>) -> bool {
    arguments
        .into_iter()
        .any(|argument| argument == "--skip-pending-update")
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
    use super::{
        DesktopRuntime, StartupExit, record_startup_identity, update_lease, update_supervisor,
    };
    use std::fs::{self, OpenOptions};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn startup_identity_is_classified_in_the_session_log() {
        let dir = temp_dir("hex-startup-classification");
        let mut log = hex_diagnostics::SessionLog::start(dir.clone()).unwrap();
        record_startup_identity(&log.handle());
        log.shutdown().unwrap();
        let contents = fs::read_to_string(log.path()).unwrap();
        assert!(
            contents
                .lines()
                .any(|line| line.contains("level=info category=startup")
                    && line.contains("hex version="))
        );
        fs::remove_dir_all(dir).unwrap();
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

    #[test]
    fn normal_exit_joins_workers_before_releasing_update_lease() {
        let dir = temp_dir("hex-normal-shutdown-order");
        let target = dir.join("hex.app");
        fs::create_dir(&target).unwrap();
        let lock_path = dir.join(".hex-test-build.lock");
        let lease = update_lease::UpdateLease::acquire_lock_file_for_test(&lock_path).unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&installer).is_err());

        let (client, worker) = hex_ui::update_channel();
        let supervisor = update_supervisor::start(target, worker).unwrap();
        let log = hex_diagnostics::SessionLog::start(dir.join("logs"));
        let exit = StartupExit::new(log);
        exit.opened.set(true);
        exit.attach_runtime(DesktopRuntime {
            update_supervisor: Some(supervisor),
            report_worker: None,
            mts_worker: None,
            midi_worker: None,
            update_lease: Some(lease),
        });

        let mut order = Vec::new();
        exit.shutdown_all_observed(|step| {
            // Exclusive lock must remain blocked until update_lease drops last.
            if step != "update_lease" {
                assert!(
                    fs2::FileExt::try_lock_exclusive(&installer).is_err(),
                    "update lease released before {step} completed"
                );
            }
            order.push(step);
        });
        assert_eq!(order, ["update_supervisor", "session_log", "update_lease"]);
        fs2::FileExt::try_lock_exclusive(&installer)
            .expect("exclusive lock must be available after ordered shutdown");
        let _ = fs2::FileExt::unlock(&installer);
        // Second call (e.g. after Application::run returns) is an idempotent no-op.
        assert_eq!(exit.prepare_quit_exit(), None);
        drop(client);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_startup_quit_shuts_down_workers_and_releases_lease_before_exiting() {
        let dir = temp_dir("hex-failed-startup-shutdown");
        let target = dir.join("hex.app");
        fs::create_dir(&target).unwrap();
        let lock_path = dir.join(".hex-test-build.lock");
        let lease = update_lease::UpdateLease::acquire_lock_file_for_test(&lock_path).unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();

        let (client, worker) = hex_ui::update_channel();
        let supervisor = update_supervisor::start(target, worker).unwrap();
        let mts = hex_mts::Worker::start().ok();
        let midi = hex_midi::Worker::start().ok();
        let had_mts = mts.is_some();
        let had_midi = midi.is_some();
        let log = hex_diagnostics::SessionLog::start(dir.join("logs"));
        let exit = StartupExit::new(log);
        // Editor never opened -> failed startup exit code 1.
        exit.attach_runtime(DesktopRuntime {
            update_supervisor: Some(supervisor),
            report_worker: None,
            mts_worker: mts,
            midi_worker: midi,
            update_lease: Some(lease),
        });

        let mut expected = vec!["update_supervisor"];
        if had_mts {
            expected.push("mts_worker");
        }
        if had_midi {
            expected.push("midi_worker");
        }
        expected.push("session_log");
        expected.push("update_lease");

        let mut order = Vec::new();
        exit.shutdown_all_observed(|step| {
            if step != "update_lease" {
                assert!(
                    fs2::FileExt::try_lock_exclusive(&installer).is_err(),
                    "update lease released before {step} finished on failed startup"
                );
            }
            order.push(step);
        });
        assert_eq!(order, expected);
        assert_eq!(exit.prepare_quit_exit(), Some(1));
        fs2::FileExt::try_lock_exclusive(&installer)
            .expect("exclusive lock must be released before failed startup exits");
        let _ = fs2::FileExt::unlock(&installer);
        drop(client);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn normal_quit_prepare_quit_exit_tears_down_workers_flushes_log_and_releases_lease() {
        let dir = temp_dir("hex-normal-quit-prepare");
        let target = dir.join("hex.app");
        fs::create_dir(&target).unwrap();
        let lock_path = dir.join(".hex-test-build.lock");
        let lease = update_lease::UpdateLease::acquire_lock_file_for_test(&lock_path).unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&installer).is_err());

        let (client, worker) = hex_ui::update_channel();
        let supervisor = update_supervisor::start(target, worker).unwrap();
        let log = hex_diagnostics::SessionLog::start(dir.join("logs")).unwrap();
        let log_path = log.path().to_path_buf();
        let exit = StartupExit::new(Ok(log));
        exit.opened.set(true);
        exit.attach_runtime(DesktopRuntime {
            update_supervisor: Some(supervisor),
            report_worker: None,
            mts_worker: hex_mts::Worker::start().ok(),
            midi_worker: hex_midi::Worker::start().ok(),
            update_lease: Some(lease),
        });
        let handle = exit.log_handle().unwrap();
        super::record_startup_warning(Some(&handle), "midi worker unavailable: test");

        // Normal macOS quit calls prepare_quit_exit() from on_app_quit without
        // returning from Application::run; it must return None and release everything.
        assert_eq!(exit.prepare_quit_exit(), None);
        fs2::FileExt::try_lock_exclusive(&installer)
            .expect("exclusive lock must be available after normal quit teardown");
        let _ = fs2::FileExt::unlock(&installer);
        let contents = fs::read_to_string(&log_path).unwrap();
        assert!(
            contents.contains("level=warn category=startup")
                && contents.contains("midi worker unavailable: test"),
            "session log must flush startup warning on normal quit: {contents}"
        );
        drop(client);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn panic_record_helper_logs_and_flushes_session_log() {
        let dir = temp_dir("hex-panic-log");
        let log = hex_diagnostics::SessionLog::start(dir.clone()).unwrap();
        let log_path = log.path().to_path_buf();
        let shared = std::sync::Mutex::new(Ok(log));
        super::record_panic_and_flush_log(&shared, "panic at main.rs:42:5: simulated crash");
        let contents = fs::read_to_string(&log_path).unwrap();
        assert!(
            contents.contains("level=error category=uncategorised")
                && contents.contains("panic at main.rs:42:5: simulated crash"),
            "panic hook helper must record error and flush session log: {contents}"
        );
        fs::remove_dir_all(dir).unwrap();
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

    #[test]
    fn skip_pending_update_flag_is_recognized_cleanly() {
        assert!(!skip_pending_update_flag(std::iter::empty()));
        assert!(!skip_pending_update_flag(
            ["--demo", "--diagnostics"].into_iter().map(Into::into)
        ));
        assert!(skip_pending_update_flag(
            ["--skip-pending-update"].into_iter().map(Into::into)
        ));
        assert!(skip_pending_update_flag(
            ["--diagnostics", "--skip-pending-update"]
                .into_iter()
                .map(Into::into)
        ));
    }
}
