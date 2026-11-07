//! Private standalone Mac process owner. Never linked into GPUI or the library.
use hex_plugin_scan::macos::report::{Cleanup, GroupOutcome, Report};
use hex_plugin_scan::{
    Discovery,
    macos::{
        build_identity::Build,
        identity::Bundle,
        protocol::{self, Attempt},
    },
};
use rustix::process::{self, Pid, Signal, WaitId, WaitIdOptions};
use std::{
    io::{Read, Write},
    os::{
        fd::OwnedFd,
        unix::{
            net::UnixStream,
            process::{CommandExt, ExitStatusExt},
        },
    },
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const DIAGNOSTIC_LIMIT: usize = 64 * 1024;
const CLEANUP_BUDGET: Duration = Duration::from_millis(500);

struct SignalGuard(Vec<signal_hook::SigId>);
impl Drop for SignalGuard {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

fn signals() -> Result<(SignalGuard, Arc<AtomicBool>), String> {
    let mut guard = SignalGuard(Vec::with_capacity(3));
    guard.0.push(
        signal_hook::flag::register(
            signal_hook::consts::SIGCHLD,
            Arc::new(AtomicBool::new(false)),
        )
        .map_err(|e| format!("waitable child disposition unavailable: {e}"))?,
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        guard.0.push(
            signal_hook::flag::register(signal, Arc::clone(&cancelled))
                .map_err(|e| format!("scoped cancellation signal unavailable: {e}"))?,
        );
    }
    Ok((guard, cancelled))
}

pub struct Pending {
    process: NativeChild,
    cleanup: Cleanup,
    _signal: SignalGuard,
}

impl Pending {
    pub fn cleanup_snapshot(&self) -> Cleanup {
        self.cleanup.clone()
    }

    pub fn poll(&mut self) -> Result<bool, String> {
        poll_child(&mut self.process, &mut self.cleanup)
    }
}

trait ChildOperations {
    fn observe(&mut self) -> Result<bool, String>;
    fn group_signal(&mut self, signal: Signal) -> GroupOutcome;
    fn direct_kill(&mut self) -> String;
    fn reap(&mut self) -> Result<std::process::ExitStatus, String>;
}
struct NativeChild {
    child: Child,
    pid: Pid,
}
impl ChildOperations for NativeChild {
    fn observe(&mut self) -> Result<bool, String> {
        observe(self.pid)
    }
    fn group_signal(&mut self, signal: Signal) -> GroupOutcome {
        signal_group(self.pid, signal)
    }
    fn direct_kill(&mut self) -> String {
        match self.child.kill() {
            Ok(()) => "requested while direct PID reserved".into(),
            Err(e) => format!("request failed; exit still unproven: {e}"),
        }
    }
    fn reap(&mut self) -> Result<std::process::ExitStatus, String> {
        self.child
            .wait()
            .map_err(|e| format!("direct reap failed; custody unknown: {e}"))
    }
}
fn lose_custody(cleanup: &mut Cleanup) {
    cleanup.custody_lost = true;
    cleanup.cleanup_unconfirmed = true;
    cleanup.pending = false;
}
fn poll_child(process: &mut impl ChildOperations, cleanup: &mut Cleanup) -> Result<bool, String> {
    if cleanup.direct_reaped {
        return Ok(true);
    }
    if cleanup.custody_lost {
        return Err("child custody lost; no further numeric operations permitted".into());
    }
    match process.observe() {
        Ok(false) => Ok(false),
        Ok(true) => match process.reap() {
            Ok(status) => {
                cleanup.direct_reaped = true;
                cleanup.pending = false;
                cleanup.direct_exit_code = status.code();
                cleanup.direct_exit_signal = status.signal();
                Ok(true)
            }
            Err(error) => {
                lose_custody(cleanup);
                Err(error)
            }
        },
        Err(error) => {
            lose_custody(cleanup);
            Err(error)
        }
    }
}

fn retire_child(process: &mut impl ChildOperations, cleanup: &mut Cleanup) -> Result<(), String> {
    if cleanup.direct_reaped || cleanup.custody_lost {
        return Ok(());
    }
    cleanup.exit_observed_before_signals = match process.observe() {
        Ok(exited) => exited,
        Err(error) => {
            lose_custody(cleanup);
            return Err(error);
        }
    };
    cleanup.original_group_term = Some(process.group_signal(Signal::TERM));
    std::thread::sleep(Duration::from_millis(5));
    cleanup.original_group_kill = Some(process.group_signal(Signal::KILL));
    if !cleanup.exit_observed_before_signals {
        cleanup.direct_kill = Some(process.direct_kill());
    }
    if [
        cleanup.original_group_term.as_ref(),
        cleanup.original_group_kill.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.compatible_with_direct_retirement(cleanup.exit_observed_before_signals))
    {
        return Err("scoped group termination failed".into());
    }
    Ok(())
}

fn may_publish(cleanup: &Cleanup) -> bool {
    cleanup.may_publish()
}

fn interruption(
    deadline_started: Option<Instant>,
    deadline: Duration,
    cancelled: &mut impl FnMut() -> Result<bool, String>,
) -> Result<Option<&'static str>, String> {
    if cancelled()? {
        Ok(Some("cancelled"))
    } else if deadline_started.is_some_and(|started| started.elapsed() >= deadline) {
        Ok(Some("deadline exceeded"))
    } else {
        Ok(None)
    }
}

fn publish_candidate(
    candidate: Option<Discovery>,
    cleanup: &Cleanup,
    error: &mut Option<String>,
    stopped: &mut Option<&'static str>,
    mut final_check: impl FnMut() -> Result<Option<&'static str>, String>,
) -> Option<Discovery> {
    if stopped.is_none() {
        match final_check() {
            Ok(Some(reason)) => {
                *stopped = Some(reason);
                error.get_or_insert_with(|| format!("scan {reason}"));
            }
            Ok(None) => {}
            Err(value) => {
                error.get_or_insert(value);
            }
        }
    }
    if error.is_none() && stopped.is_none() && may_publish(cleanup) {
        candidate
    } else {
        None
    }
}

fn observe(pid: Pid) -> Result<bool, String> {
    match process::waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    ) {
        Ok(value) => Ok(value.is_some()),
        Err(rustix::io::Errno::INTR) => Ok(false),
        Err(error) => Err(format!(
            "child observation failed; custody unknown: {error}"
        )),
    }
}

fn signal_group(pid: Pid, signal: Signal) -> GroupOutcome {
    match process::kill_process_group(pid, signal) {
        Ok(()) => GroupOutcome::Sent,
        Err(rustix::io::Errno::SRCH) => GroupOutcome::Absent,
        // Darwin's pgrp iteration skips zombies and may return EPERM when no
        // signalable member remains. Keep the actual ambiguous outcome. Only
        // separately observed exit + normal direct reap can qualify metadata;
        // this never establishes group-wide cleanup or changes live EPERM.
        Err(rustix::io::Errno::PERM) => GroupOutcome::PermissionDenied { errno: 1 },
        Err(error) => GroupOutcome::Failed {
            errno: error.raw_os_error(),
            error: error.to_string(),
        },
    }
}

fn drain(reader: &mut UnixStream, output: &mut Vec<u8>, limit: usize) -> Result<bool, String> {
    let mut buffer = [0_u8; 4096];
    for _ in 0..4 {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if output.len().checked_add(count).is_none_or(|n| n > limit) {
                    return Err("scan protocol or diagnostic byte bound exceeded".into());
                }
                output.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("scan channel read failed: {error}")),
        }
    }
    Ok(false)
}

pub fn stdin_cancelled(cancel_on_disconnect: bool) -> Result<bool, String> {
    let mut input = std::io::stdin();
    let mut descriptors = [rustix::event::PollFd::new(
        &input,
        rustix::event::PollFlags::IN,
    )];
    let zero = rustix::event::Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    rustix::event::poll(&mut descriptors, Some(&zero))
        .map_err(|e| format!("cancel poll failed: {e}"))?;
    let ready = descriptors[0].revents();
    if !ready.contains(rustix::event::PollFlags::IN) {
        return Ok(cancel_on_disconnect && ready.contains(rustix::event::PollFlags::HUP));
    }
    let mut bytes = [0_u8; 64];
    let count = input
        .read(&mut bytes)
        .map_err(|e| format!("cancel read failed: {e}"))?;
    // Any stdin byte cancels. A split word cannot lose cancellation, and no
    // line-buffered read or competing stdin reader is introduced.
    Ok(count != 0 || cancel_on_disconnect)
}

pub fn run(
    build: Build,
    bundle: Bundle,
    attempt: Attempt,
    deadline: Duration,
    wait_for_plugin: bool,
    mut cancelled: impl FnMut() -> Result<bool, String>,
) -> Result<(Report, Option<Pending>), String> {
    if deadline.is_zero() || deadline > Duration::from_secs(30) {
        return Err("scan deadline must be1..30000 milliseconds".into());
    }
    let mut deadline_started = Some(Instant::now());
    let mut outgoing = protocol::request(&bundle, attempt)?;
    build.unchanged()?;
    bundle.unchanged()?;
    let (signal, signal_cancelled) = signals()?;
    let mut cancelled = || Ok(signal_cancelled.load(Ordering::Relaxed) || cancelled()?);
    let (mut channel, peer) =
        UnixStream::pair().map_err(|e| format!("scan protocol unavailable: {e}"))?;
    let (mut stdout, out_peer) =
        UnixStream::pair().map_err(|e| format!("scan stdout unavailable: {e}"))?;
    let (mut stderr, err_peer) =
        UnixStream::pair().map_err(|e| format!("scan stderr unavailable: {e}"))?;
    for socket in [&channel, &stdout, &stderr] {
        socket
            .set_nonblocking(true)
            .map_err(|e| format!("scan nonblocking mode failed: {e}"))?;
    }
    let child = Command::new(&build.helper)
        .env_clear()
        .env("LANG", "C.UTF-8")
        .process_group(0)
        .stdin(Stdio::from(OwnedFd::from(peer)))
        .stdout(Stdio::from(OwnedFd::from(out_peer)))
        .stderr(Stdio::from(OwnedFd::from(err_peer)))
        .spawn()
        .map_err(|e| format!("trusted helper spawn failed: {e}"))?;
    let pid = Pid::from_child(&child);
    let mut owner = Pending {
        process: NativeChild { child, pid },
        cleanup: Cleanup {
            descendants: "unknown".into(),
            pending: true,
            ..Cleanup::default()
        },
        _signal: signal,
    };
    let _ = writeln!(std::io::stderr(), "mac scan: owned helper started");
    let mut sent = 0;
    let mut incoming = Vec::new();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut executable = None;
    let mut identifier = None;
    let mut terminal_phase = false;
    let mut admission_notified = false;
    let mut staged = None;
    let mut stopped = None;
    let mut cause = None;

    let work = (|| -> Result<(), String> {
        loop {
            if let Some(reason) = interruption(deadline_started, deadline, &mut cancelled)? {
                stopped = Some(reason);
                return Err(format!("scan {reason}"));
            }
            if sent < outgoing.len() {
                let end = sent.saturating_add(16384).min(outgoing.len());
                match channel.write(&outgoing[sent..end]) {
                    Ok(0) => return Err("scan request channel closed".into()),
                    Ok(count) => sent += count,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) => return Err(format!("scan request write failed: {e}")),
                }
                if terminal_phase && sent == outgoing.len() && !admission_notified {
                    admission_notified = true;
                    // An interactive scan can wait inside the native plug-in
                    // for a modal response. Cancellation still owns the child.
                    if wait_for_plugin {
                        deadline_started = None;
                    }
                    let _ = writeln!(std::io::stderr(), "mac scan: admission sent");
                }
            }
            let eof = drain(&mut channel, &mut incoming, protocol::FRAME_LIMIT)?;
            drain(&mut stdout, &mut out, DIAGNOSTIC_LIMIT)?;
            drain(&mut stderr, &mut err, DIAGNOSTIC_LIMIT)?;
            if staged.is_some() && !incoming.is_empty() {
                return Err("trailing data after terminal scan frame".into());
            }
            if let Some(length) = protocol::frame_length(&incoming)? {
                if incoming.len() > length {
                    return Err("duplicate or trailing scan frames".into());
                }
                if incoming.len() == length && !terminal_phase {
                    if sent != outgoing.len() {
                        return Err("preflight before complete request".into());
                    }
                    let preflight = protocol::preflight(&incoming, attempt, &bundle)?;
                    if let Some(reason) = interruption(deadline_started, deadline, &mut cancelled)?
                    {
                        stopped = Some(reason);
                        return Err(format!("scan {reason} before admission"));
                    }
                    let admitted = bundle.admit(&preflight.executable)?;
                    if let Some(reason) = interruption(deadline_started, deadline, &mut cancelled)?
                    {
                        stopped = Some(reason);
                        return Err(format!("scan {reason} during admission"));
                    }
                    outgoing = protocol::admission(&bundle, &admitted, attempt)?;
                    sent = 0;
                    executable = Some(admitted);
                    identifier = Some(preflight.identifier);
                    incoming.clear();
                    terminal_phase = true;
                } else if incoming.len() == length {
                    staged = Some(protocol::terminal(&incoming, attempt)?);
                    if wait_for_plugin {
                        deadline_started = Some(Instant::now());
                    }
                    incoming.clear();
                    let _ = writeln!(std::io::stderr(), "mac scan: terminal result received");
                }
            }
            match owner.process.observe() {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    lose_custody(&mut owner.cleanup);
                    return Err(error);
                }
            }
            if eof && staged.is_none() && (!terminal_phase || incoming.is_empty()) {
                return Err("scan protocol closed before terminal result".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    })();
    if let Err(error) = work {
        cause = Some(error);
    }

    // Child PID is still unreaped here. Never signal after a reap/unknown state.
    if let Err(error) = retire_child(&mut owner.process, &mut owner.cleanup) {
        cause.get_or_insert(error);
    }
    let cleanup_started = Instant::now();
    while !owner.cleanup.custody_lost && cleanup_started.elapsed() < CLEANUP_BUDGET {
        match owner.poll() {
            Ok(true) => break,
            Ok(false) => {}
            Err(error) => {
                cause.get_or_insert(error);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Bounded final drains; EOF is not a prerequisite or descendant proof.
    for _ in 0..70 {
        for result in [
            drain(&mut channel, &mut incoming, protocol::FRAME_LIMIT),
            drain(&mut stdout, &mut out, DIAGNOSTIC_LIMIT),
            drain(&mut stderr, &mut err, DIAGNOSTIC_LIMIT),
        ] {
            if let Err(error) = result {
                cause.get_or_insert(error);
            }
        }
    }
    if owner.cleanup.custody_lost {
        cause.get_or_insert_with(|| {
            "scan custody lost; cleanup unconfirmed; no further signals or reaps".into()
        });
    } else if !owner.cleanup.direct_reaped {
        cause.get_or_insert_with(|| "scan cleanup pending; control owner retained".into());
    }
    if owner.cleanup.direct_exit_code != Some(0) {
        cause.get_or_insert_with(|| "native scan failed or crashed".into());
    }
    for result in [build.unchanged(), bundle.unchanged().map_err(String::from)] {
        if let Err(error) = result {
            cause.get_or_insert(error);
        }
    }
    if let Some(value) = &executable
        && let Err(error) = bundle.executable_unchanged(value)
    {
        cause.get_or_insert(String::from(error));
    }
    if sent != outgoing.len() {
        cause.get_or_insert_with(|| "incomplete scan request/admission".into());
    }
    if staged.is_some() && !incoming.is_empty() {
        cause.get_or_insert_with(|| "trailing data after terminal scan frame".into());
    }
    if staged.is_none() && terminal_phase && cause.is_none() {
        match protocol::terminal(&incoming, attempt) {
            Ok(value) => staged = Some(value),
            Err(error) => {
                cause = Some(error);
            }
        }
    }
    let discovery = publish_candidate(staged, &owner.cleanup, &mut cause, &mut stopped, || {
        interruption(deadline_started, deadline, &mut cancelled)
    });
    if discovery.is_none() && cause.is_none() {
        cause = Some("terminal scan result missing".into());
    }
    let cleanup = owner.cleanup.clone();
    let report = Report {
        profile: "explicit-user-trusted-macos-factory-only".into(),
        request: attempt.request,
        generation: attempt.generation,
        receipt_sha256: build.receipt_sha256,
        helper_sha256: build.helper_sha256,
        bundle,
        executable,
        bundle_identifier: identifier,
        discovery,
        interruption: stopped.map(str::to_owned),
        error: cause,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        cleanup,
    };
    if report.cleanup.pending {
        // Preserve the actual owner and its custody state, not just a report.
        Ok((report, Some(owner)))
    } else {
        Ok((report, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::ExitStatus;

    struct Kernel {
        events: Vec<&'static str>,
        lost: bool,
        running: bool,
        denied: bool,
    }
    impl ChildOperations for Kernel {
        fn observe(&mut self) -> Result<bool, String> {
            self.events.push("observe");
            if self.lost {
                Err("injected ECHILD".into())
            } else {
                Ok(!self.running)
            }
        }
        fn group_signal(&mut self, signal: Signal) -> GroupOutcome {
            self.events.push(if signal == Signal::TERM {
                "group term"
            } else {
                "group kill"
            });
            if self.denied {
                GroupOutcome::PermissionDenied { errno: 1 }
            } else {
                GroupOutcome::Sent
            }
        }
        fn direct_kill(&mut self) -> String {
            self.events.push("direct kill");
            "requested".into()
        }
        fn reap(&mut self) -> Result<ExitStatus, String> {
            self.events.push("reap");
            Ok(ExitStatus::from_raw(0))
        }
    }
    #[test]
    fn lost_custody_is_terminal_without_further_pid_operations_or_metadata() {
        let mut kernel = Kernel {
            events: Vec::new(),
            lost: true,
            running: false,
            denied: false,
        };
        let mut cleanup = Cleanup {
            pending: true,
            descendants: "unknown".into(),
            ..Cleanup::default()
        };
        assert!(retire_child(&mut kernel, &mut cleanup).is_err());
        assert!(cleanup.custody_lost && cleanup.cleanup_unconfirmed && !cleanup.pending);
        assert!(!may_publish(&cleanup));
        assert!(retire_child(&mut kernel, &mut cleanup).is_ok());
        assert!(poll_child(&mut kernel, &mut cleanup).is_err());
        assert_eq!(kernel.events, ["observe"]);
    }
    #[test]
    fn reaping_is_exactly_once_and_ends_all_pid_and_group_operations() {
        let mut kernel = Kernel {
            events: Vec::new(),
            lost: false,
            running: false,
            denied: false,
        };
        let mut cleanup = Cleanup {
            pending: true,
            descendants: "unknown".into(),
            ..Cleanup::default()
        };
        retire_child(&mut kernel, &mut cleanup).expect("owned retirement");
        assert!(poll_child(&mut kernel, &mut cleanup).expect("owned reap"));
        assert!(may_publish(&cleanup));
        let trace = kernel.events.clone();
        retire_child(&mut kernel, &mut cleanup).expect("already reaped");
        assert!(poll_child(&mut kernel, &mut cleanup).expect("already reaped"));
        assert_eq!(kernel.events, trace);
        assert_eq!(
            trace,
            ["observe", "group term", "group kill", "observe", "reap"]
        );
    }

    #[test]
    fn zombie_group_denial_is_not_confused_with_live_permission_failure() {
        for running in [false, true] {
            let mut kernel = Kernel {
                events: Vec::new(),
                lost: false,
                running,
                denied: true,
            };
            let mut cleanup = Cleanup {
                pending: true,
                descendants: "unknown".into(),
                ..Cleanup::default()
            };
            assert_eq!(retire_child(&mut kernel, &mut cleanup).is_err(), running);
            kernel.running = false;
            assert!(poll_child(&mut kernel, &mut cleanup).expect("direct normal reap"));
            assert_eq!(may_publish(&cleanup), !running);
            assert!(matches!(
                cleanup.original_group_term,
                Some(GroupOutcome::PermissionDenied { errno: 1 })
            ));
            assert_eq!(cleanup.descendants, "unknown");
            assert_eq!(kernel.events.contains(&"direct kill"), running);
        }
    }

    #[test]
    fn cancellation_at_publication_wins_over_valid_metadata_and_normal_reap() {
        let candidate = Discovery {
            request: 1,
            sdk: hex_plugin_scan::SDK.into(),
            public_sdk: hex_plugin_scan::PUBLIC_SDK.into(),
            platform: "darwin-arm64".into(),
            factory_flags: 0,
            vendor: "original".into(),
            url: String::new(),
            email: String::new(),
            classes: Vec::new(),
        };
        let mut kernel = Kernel {
            events: Vec::new(),
            lost: false,
            running: false,
            denied: false,
        };
        let mut cleanup = Cleanup {
            pending: true,
            descendants: "unknown".into(),
            ..Cleanup::default()
        };
        retire_child(&mut kernel, &mut cleanup).expect("retire");
        assert!(poll_child(&mut kernel, &mut cleanup).expect("reap"));
        let mut error = None;
        let mut stopped = None;
        assert!(
            publish_candidate(Some(candidate), &cleanup, &mut error, &mut stopped, || Ok(
                Some("cancelled")
            ))
            .is_none()
        );
        assert_eq!(stopped, Some("cancelled"));
        assert_eq!(error.as_deref(), Some("scan cancelled"));
        assert!(
            may_publish(&cleanup),
            "the cancellation gate, not a failed reap, refused the record"
        );
    }
}
