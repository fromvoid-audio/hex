//! Shared Unix helper supervision, stdin cancellation polling, and direct-child
//! reaping for standalone scanner executables. Never imported by scan libraries.
use crate::{
    framing::FRAME_HEADER,
    io::{drain_bounded, final_drain_passes},
    signal::SignalGuard,
};
use rustix::process::{self, Pid, Signal, WaitId, WaitIdOptions};
use serde::Serialize;
use std::{
    io::{Read, Write},
    os::{
        fd::{AsFd, OwnedFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// Tracks process-group reaping state so `pending()` remains `true` until full
/// completion (both direct child and all process-group descendants reaped).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GroupCleanupState {
    pub direct_reaped: bool,
    pub complete: bool,
    pub group_scoped: bool,
    pub descendants: usize,
    pub descendant_pids: Vec<i32>,
}

impl GroupCleanupState {
    pub fn new(group_scoped: bool) -> Self {
        Self {
            direct_reaped: false,
            complete: false,
            group_scoped,
            descendants: 0,
            descendant_pids: Vec::new(),
        }
    }

    pub fn record_direct_reaped(&mut self) {
        self.direct_reaped = true;
    }

    pub fn record_descendant(&mut self, pid: i32) {
        self.descendants += 1;
        self.descendant_pids.push(pid);
    }

    pub fn record_complete(&mut self) {
        self.complete = true;
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn is_pending(&self) -> bool {
        !self.complete
    }
}

/// Poll a readable file descriptor for cancellation without blocking or
/// mutating its flags. Any available byte (preventing split-read loss of
/// multi-byte words or single-byte cancel writes) or `POLLHUP`/EOF triggers
/// cancellation when `cancel_on_disconnect` is enabled.
pub fn fd_cancelled<Fd: AsFd + Read>(
    reader: &mut Fd,
    cancel_on_disconnect: bool,
) -> Result<bool, String> {
    let mut descriptors = [rustix::event::PollFd::new(
        &*reader,
        rustix::event::PollFlags::IN,
    )];
    let zero = rustix::event::Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    loop {
        match rustix::event::poll(&mut descriptors, Some(&zero)) {
            Ok(_) => break,
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return Err("cancel poll failed".into()),
        }
    }
    let ready = descriptors[0].revents();
    if !ready.contains(rustix::event::PollFlags::IN) {
        return Ok(cancel_on_disconnect && ready.contains(rustix::event::PollFlags::HUP));
    }
    let mut buffer = [0_u8; 64];
    let count = loop {
        match reader.read(&mut buffer) {
            Ok(n) => break n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("cancel read failed".into()),
        }
    };
    // Any byte cancels (single-byte or split multi-byte writes); 0 bytes means
    // EOF/disconnect.
    Ok(count != 0 || cancel_on_disconnect)
}

/// Poll stdin for cancellation. A terminal is optional; readiness polling never
/// changes its file flags.
pub fn stdin_cancelled(cancel_on_disconnect: bool) -> Result<bool, String> {
    let mut input = std::io::stdin();
    fd_cancelled(&mut input, cancel_on_disconnect)
}

pub(crate) fn drain(
    reader: &mut impl Read,
    bytes: &mut Vec<u8>,
    limit: usize,
) -> Result<(), String> {
    drain_bounded(
        reader,
        bytes,
        limit,
        "IPC or diagnostic bound exceeded",
        |_| "pipe read failed".into(),
    )?;
    Ok(())
}

/// Platform-specific reaping and output strategy for the unified Unix
/// supervision loop.
pub(crate) trait SupervisionStrategy {
    type Cleanup;
    type Pending;

    fn prepare_platform(&self) -> Result<(), String>;
    fn unsupported_pid_pending(&self, child: Child) -> Self::Pending;
    fn terminate_and_pending(
        &self,
        child: Child,
        pid: Pid,
        exited: bool,
        cause: &mut Option<String>,
    ) -> Self::Pending;
    fn poll_pending(&self, pending: &mut Self::Pending) -> Result<bool, String>;
    fn cleanup_of(&self, pending: &Self::Pending) -> Self::Cleanup;
}

/// Captured streams, termination cause, and cleanup report returned by the
/// shared Unix helper runner.
pub(crate) struct RawOutput<C> {
    pub packet: Vec<u8>,
    pub stdout: String,
    pub stderr: String,
    pub cause: Option<String>,
    pub cleanup: C,
}

pub(crate) type SupervisedResult<C, P> = Result<(RawOutput<C>, Option<P>), String>;

/// Unified Unix helper supervision loop shared between Linux subreaper
/// supervision and macOS direct-child supervision.
pub(crate) fn run_supervised<S: SupervisionStrategy>(
    strategy: &S,
    helper: &Path,
    module: &Path,
    request: u64,
    deadline: Duration,
    input: &[u8],
    max_packet: usize,
) -> SupervisedResult<S::Cleanup, S::Pending> {
    let frame_limit = max_packet
        .checked_add(FRAME_HEADER)
        .ok_or("packet bound overflow")?;
    if input.len() > frame_limit {
        return Err("request capacity refused".into());
    }
    if deadline.is_zero() || deadline > Duration::from_secs(30) {
        return Err("deadline outside controlled bound".into());
    }
    // Re-establish waitable children even if our launcher inherited SIG_IGN or
    // SA_NOCLDWAIT. Audited safe flag registration installs a SIGCHLD handler
    // without NOCLDWAIT; no workspace unsafe or signal callback allocation.
    let _sigchld =
        SignalGuard::register_sigchld().map_err(|_| "waitable child disposition unavailable")?;
    strategy.prepare_platform()?;
    let (mut protocol, writer) = UnixStream::pair().map_err(|_| "protocol pair failed")?;
    let (mut stdout, out_writer) = UnixStream::pair().map_err(|_| "stdout pair failed")?;
    let (mut stderr, err_writer) = UnixStream::pair().map_err(|_| "stderr pair failed")?;
    for reader in [&protocol, &stdout, &stderr] {
        reader
            .set_nonblocking(true)
            .map_err(|_| "capture flags failed")?;
    }
    let started = Instant::now();
    let child = Command::new(helper)
        .arg(module)
        .arg(request.to_string())
        .env_clear()
        .env("LANG", "C.UTF-8")
        .process_group(0)
        .stdin(Stdio::from(OwnedFd::from(writer)))
        .stdout(Stdio::from(OwnedFd::from(out_writer)))
        .stderr(Stdio::from(OwnedFd::from(err_writer)))
        .spawn()
        .map_err(|_| "helper spawn failed")?;
    let pid = i32::try_from(child.id())
        .ok()
        .filter(|p| *p > 1)
        .and_then(Pid::from_raw);
    let Some(pid) = pid else {
        let pending = strategy.unsupported_pid_pending(child);
        let cleanup = strategy.cleanup_of(&pending);
        let output = RawOutput {
            packet: Vec::new(),
            stdout: String::new(),
            stderr: String::new(),
            cause: Some("unsupported child PID; owned lease retained".into()),
            cleanup,
        };
        return Ok((output, Some(pending)));
    };
    eprintln!("scanner: controlled helper started");
    let mut sent = 0;
    let mut write_closed = input.is_empty();
    let mut packet = Vec::new();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut cause = None;
    let mut exited = false;
    loop {
        if sent < input.len() {
            let end = (sent + 16384).min(input.len());
            match protocol.write(&input[sent..end]) {
                Ok(0) => cause = Some("request write closed".into()),
                Ok(n) => sent += n,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => cause = Some(format!("request write failed: {e}")),
            }
        }
        if sent == input.len() && !write_closed {
            if protocol.shutdown(std::net::Shutdown::Write).is_err() {
                cause = Some("request close failed".into());
            }
            write_closed = true;
        }
        for result in [
            drain(&mut protocol, &mut packet, frame_limit),
            drain(&mut stdout, &mut out, 64 * 1024),
            drain(&mut stderr, &mut err, 64 * 1024),
        ] {
            if let Err(error) = result {
                cause = Some(error);
            }
        }
        match stdin_cancelled(true) {
            Ok(true) => cause = Some("cancelled".into()),
            Err(error) => cause = Some(error),
            Ok(false) => {}
        }
        if started.elapsed() >= deadline {
            cause = Some("deadline exceeded".into());
        }
        if cause.is_some() {
            break;
        }
        let waited = loop {
            match process::waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            ) {
                Err(rustix::io::Errno::INTR) => continue,
                other => break other,
            }
        };
        match waited {
            Ok(Some(status)) => {
                exited = true;
                if sent != input.len() {
                    cause = Some("incomplete request refused".into());
                }
                if !status.exited() || status.exit_status() != Some(0) {
                    cause = Some("native helper failed or crashed".into());
                }
                break;
            }
            Ok(None) => {}
            Err(_) => {
                cause = Some("native observation failed".into());
                break;
            }
        }
        // Wait via `poll(2)` up to 2 ms so helper output, stdin cancel, or
        // SIGCHLD (`EINTR`) wakes the loop immediately without fixed sleep lag.
        let stdin = std::io::stdin();
        let proto_flags = if sent < input.len() {
            rustix::event::PollFlags::IN | rustix::event::PollFlags::OUT
        } else {
            rustix::event::PollFlags::IN
        };
        let mut wait_fds = [
            rustix::event::PollFd::new(&protocol, proto_flags),
            rustix::event::PollFd::new(&stdout, rustix::event::PollFlags::IN),
            rustix::event::PollFd::new(&stderr, rustix::event::PollFlags::IN),
            rustix::event::PollFd::new(&stdin, rustix::event::PollFlags::IN),
        ];
        let timeout = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 2_000_000,
        };
        let _ = rustix::event::poll(&mut wait_fds, Some(&timeout));
    }
    let mut pending = strategy.terminate_and_pending(child, pid, exited, &mut cause);
    let cleanup_start = Instant::now();
    let mut complete = false;
    while cleanup_start.elapsed() < Duration::from_millis(500) {
        match strategy.poll_pending(&mut pending) {
            Ok(true) => {
                complete = true;
                break;
            }
            Ok(false) => {}
            Err(error) => {
                cause = Some(error);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Termination/reaping precedes final drain, including inherited pipes.
    for _ in 0..final_drain_passes(frame_limit) {
        let before = (packet.len(), out.len(), err.len());
        for result in [
            drain(&mut protocol, &mut packet, frame_limit),
            drain(&mut stdout, &mut out, 64 * 1024),
            drain(&mut stderr, &mut err, 64 * 1024),
        ] {
            if let Err(error) = result {
                cause = Some(error);
            }
        }
        if (packet.len(), out.len(), err.len()) == before {
            break;
        }
    }
    let cleanup = strategy.cleanup_of(&pending);
    if !complete {
        cause = Some("cleanup incomplete: owned lease retained".into());
    }
    let output = RawOutput {
        packet,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        cause,
        cleanup,
    };
    Ok((output, (!complete).then_some(pending)))
}

/// Direct-child cleanup summary for the enrolled ORIGINAL never-fork Mac
/// profile (no subreaper or descendant observation/reaping claim).
#[derive(Debug, Default, Serialize)]
pub struct DirectCleanup {
    pub direct_reaped: bool,
    pub descendant_policy: String,
    pub group_signals_scoped: bool,
    pub pending: bool,
}

#[derive(Debug, Serialize)]
pub struct DirectOutput {
    pub packet: Vec<u8>,
    pub stdout: String,
    pub stderr: String,
    pub cause: Option<String>,
    pub cleanup: DirectCleanup,
}

pub struct DirectPending {
    child: Child,
    pid: Option<Pid>,
    direct_reaped: bool,
    complete: bool,
    group_scoped: bool,
}

impl DirectPending {
    pub fn poll(&mut self) -> Result<bool, String> {
        if self.complete {
            return Ok(true);
        }
        if let Some(pid) = self.pid {
            let state = loop {
                match process::waitid(
                    WaitId::Pid(pid),
                    WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
                ) {
                    Ok(value) => break value,
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(_) => return Err("direct child observation failed".into()),
                }
            };
            if state.is_none() {
                return Ok(false);
            }
            loop {
                match self.child.wait() {
                    Ok(_) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err("direct child wait failed".into()),
                }
            }
        } else {
            // An unsupported PID never authorizes a group signal or result.
            let status = loop {
                match self.child.try_wait() {
                    Ok(status) => break status,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err("unidentified child wait failed".into()),
                }
            };
            if status.is_none() {
                return Ok(false);
            }
        }
        self.direct_reaped = true;
        self.complete = true;
        Ok(true)
    }

    pub fn cleanup(&self) -> DirectCleanup {
        DirectCleanup {
            direct_reaped: self.direct_reaped,
            descendant_policy: "not-applicable-audited-original-never-fork".into(),
            group_signals_scoped: self.group_scoped,
            pending: !self.complete,
        }
    }
}

struct DirectChildStrategy;

impl SupervisionStrategy for DirectChildStrategy {
    type Cleanup = DirectCleanup;
    type Pending = DirectPending;

    fn prepare_platform(&self) -> Result<(), String> {
        Ok(())
    }

    fn unsupported_pid_pending(&self, child: Child) -> DirectPending {
        DirectPending {
            child,
            pid: None,
            direct_reaped: false,
            complete: false,
            group_scoped: false,
        }
    }

    fn terminate_and_pending(
        &self,
        child: Child,
        pid: Pid,
        exited: bool,
        cause: &mut Option<String>,
    ) -> DirectPending {
        // The direct child has not been reaped: its PID keeps group identity reserved.
        // No group signal is ever issued after direct reaping, preventing PID reuse.
        let mut group_scoped = false;
        if exited {
            // This audited profile has no descendants. Wait follows the observed
            // direct exit; no signals or group-empty inference are needed.
            group_scoped = true;
        } else if process::getpgid(Some(pid)) == Ok(pid) {
            group_scoped = true;
            let term = process::kill_process_group(pid, Signal::TERM);
            std::thread::sleep(Duration::from_millis(5));
            // Identity remains reserved by the unreaped direct child. Revalidate the
            // group before the last signal; a failed capability stays a failure.
            if process::getpgid(Some(pid)) == Ok(pid) {
                let kill = process::kill_process_group(pid, Signal::KILL);
                if kill.is_err() && kill != Err(rustix::io::Errno::SRCH) {
                    *cause = Some("scoped kill failed".into());
                }
            } else {
                *cause = Some("owned group identity unavailable before kill".into());
            }
            if term.is_err() && term != Err(rustix::io::Errno::SRCH) {
                *cause = Some("scoped termination failed".into());
            }
        } else {
            *cause = Some("owned group identity unavailable; no signal authorized".into());
        }
        DirectPending {
            child,
            pid: Some(pid),
            direct_reaped: false,
            complete: false,
            group_scoped,
        }
    }

    fn poll_pending(&self, pending: &mut DirectPending) -> Result<bool, String> {
        pending.poll()
    }

    fn cleanup_of(&self, pending: &DirectPending) -> DirectCleanup {
        pending.cleanup()
    }
}

/// Run one direct-child helper request for the audited never-fork profile.
pub fn run_direct_child(
    helper: &Path,
    module: &Path,
    request: u64,
    deadline: Duration,
    input: &[u8],
    max_packet: usize,
) -> Result<(DirectOutput, Option<DirectPending>), String> {
    let (raw, pending) = run_supervised(
        &DirectChildStrategy,
        helper,
        module,
        request,
        deadline,
        input,
        max_packet,
    )?;
    let output = DirectOutput {
        packet: raw.packet,
        stdout: raw.stdout,
        stderr: raw.stderr,
        cause: raw.cause,
        cleanup: raw.cleanup,
    };
    Ok((output, pending))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_cleanup_state_stays_pending_after_direct_reap_until_descendants_complete() {
        let mut state = GroupCleanupState::new(true);
        assert!(state.is_pending());
        assert!(!state.direct_reaped);

        // Direct child exited and was reaped, but descendants in the process
        // group are still running (`waitpgid` returned `Ok(None)`).
        state.record_direct_reaped();
        assert!(state.direct_reaped);
        assert!(
            state.is_pending(),
            "cleanup must stay pending while process-group descendants remain unreaped"
        );

        // One descendant reaped, more still alive.
        state.record_descendant(4242);
        assert_eq!(state.descendants, 1);
        assert_eq!(state.descendant_pids, vec![4242]);
        assert!(state.is_pending());

        // `waitpgid` finally returned `ECHILD` — cleanup is now complete.
        state.record_complete();
        assert!(state.is_complete());
        assert!(!state.is_pending());
    }

    #[test]
    fn fd_cancelled_handles_single_byte_split_word_and_eof_disconnect() {
        // Quiet open channel does not cancel.
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        assert!(!fd_cancelled(&mut reader, true).unwrap());

        // Single-byte cancel signal (e.g. hex-plugin-catalog worker `&[1]`) cancels.
        writer.write_all(&[1]).unwrap();
        assert!(fd_cancelled(&mut reader, true).unwrap());
        assert!(!fd_cancelled(&mut reader, true).unwrap());

        // Fragmented first half of "cancel" ("can") cancels immediately instead
        // of draining stdin and losing cancellation.
        writer.write_all(b"can").unwrap();
        assert!(fd_cancelled(&mut reader, true).unwrap());

        // Closing the peer (POLLHUP / EOF) cancels when `cancel_on_disconnect`
        // is true, and stays false when `cancel_on_disconnect` is false.
        drop(writer);
        assert!(!fd_cancelled(&mut reader, false).unwrap());
        assert!(fd_cancelled(&mut reader, true).unwrap());
    }

    #[test]
    fn fd_cancelled_retries_interrupted_read_without_failing_scan() {
        struct InterruptedOnce {
            inner: UnixStream,
            remaining_interrupts: usize,
        }

        impl AsFd for InterruptedOnce {
            fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
                self.inner.as_fd()
            }
        }

        impl Read for InterruptedOnce {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.remaining_interrupts > 0 {
                    self.remaining_interrupts -= 1;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "simulated SIGCHLD EINTR",
                    ));
                }
                self.inner.read(buf)
            }
        }

        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(&[1]).unwrap();
        let mut flaky = InterruptedOnce {
            inner: reader,
            remaining_interrupts: 2,
        };
        assert!(fd_cancelled(&mut flaky, true).unwrap());
        assert_eq!(flaky.remaining_interrupts, 0);
    }
}
