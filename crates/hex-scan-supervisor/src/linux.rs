//! Linux-only standalone executable ownership; never imported by a scan library.
use rustix::process::{self, Pid, Signal, WaitId, WaitIdOptions, WaitOptions};
use serde::Serialize;
use std::{
    io::{Read, Write},
    os::{
        fd::OwnedFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
#[derive(Debug, Default, Serialize)]
pub struct Cleanup {
    pub direct_reaped: bool,
    pub descendants_reaped: usize,
    pub descendant_pids: Vec<i32>,
    pub group_signals_scoped: bool,
    pub pending: bool,
}
#[derive(Debug, Serialize)]
pub struct Output {
    pub packet: Vec<u8>,
    pub stdout: String,
    pub stderr: String,
    pub cause: Option<String>,
    pub cleanup: Cleanup,
}
pub struct Pending {
    child: Child,
    pid: Pid,
    direct_reaped: bool,
    descendants: usize,
    descendant_pids: Vec<i32>,
}
impl Pending {
    pub fn poll(&mut self) -> Result<bool, String> {
        if !self.direct_reaped {
            let state = process::waitid(
                WaitId::Pid(self.pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map_err(|_| "child observation failed")?;
            if state.is_none() {
                return Ok(false);
            }
            self.child.wait().map_err(|_| "direct reap failed")?;
            self.direct_reaped = true;
        }
        loop {
            match process::waitpgid(self.pid, WaitOptions::NOHANG) {
                Ok(Some((pid, _))) => {
                    self.descendants += 1;
                    self.descendant_pids.push(pid.as_raw_nonzero().get());
                }
                Ok(None) => return Ok(false),
                Err(rustix::io::Errno::CHILD) => return Ok(true),
                Err(_) => return Err("descendant reap failed".into()),
            }
        }
    }
    pub fn cleanup(&self) -> Cleanup {
        Cleanup {
            direct_reaped: self.direct_reaped,
            descendants_reaped: self.descendants,
            descendant_pids: self.descendant_pids.clone(),
            group_signals_scoped: true,
            pending: !self.direct_reaped,
        }
    }
}
/// Magic plus little-endian length preceding every scanner protocol payload.
const FRAME_HEADER: usize = 12;
fn drain(reader: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> Result<(), String> {
    let mut buffer = [0_u8; 4096];
    for _ in 0..4 {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if bytes.len().checked_add(n).is_none_or(|v| v > limit) {
                    return Err("IPC or diagnostic bound exceeded".into());
                }
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("pipe read failed".into()),
        }
    }
    Ok(())
}
fn cancellation() -> Result<bool, String> {
    let mut input = std::io::stdin();
    // A terminal is optional; readiness polling never changes its file flags.
    let mut poll = [rustix::event::PollFd::new(
        &input,
        rustix::event::PollFlags::IN,
    )];
    let zero = rustix::event::Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let ready = rustix::event::poll(&mut poll, Some(&zero)).map_err(|_| "cancel poll failed")?;
    if ready == 0 || !poll[0].revents().contains(rustix::event::PollFlags::IN) {
        return Ok(false);
    }
    let mut buffer = [0_u8; 64];
    let n = input.read(&mut buffer).map_err(|_| "cancel read failed")?;
    Ok(buffer[..n].starts_with(b"cancel"))
}
/// Run one controlled helper request to completion and reap its process group.
/// `max_packet` is the caller format's protocol payload bound; the request and
/// the captured reply may each add the 12-byte frame header to it.
pub fn run(
    helper: &std::path::Path,
    module: &std::path::Path,
    request: u64,
    deadline: Duration,
    input: &[u8],
    max_packet: usize,
) -> Result<(Output, Option<Pending>), String> {
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
    let child_notice = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _sigchld = signal_hook::flag::register(signal_hook::consts::SIGCHLD, child_notice)
        .map_err(|_| "waitable child disposition unavailable")?;
    // This function exists only in this single-request dedicated executable.
    process::set_child_subreaper(Some(Pid::INIT)).map_err(|_| "subreaper unavailable")?;
    if process::child_subreaper().map_err(|_| "subreaper query failed")? != Some(Pid::INIT) {
        return Err("subreaper verification failed".into());
    }
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
    // Linux Child::id is a positive pid_t. Successful std spawn also confirms
    // process_group(0)'s child-side setpgid; a setup failure is a spawn error.
    let pid = Pid::from_raw(i32::try_from(child.id()).expect("Linux pid_t range"))
        .expect("positive child PID");
    assert!(
        pid.as_raw_nonzero().get() > 1,
        "spawn never returns init PID"
    );
    eprintln!("scanner: controlled helper started");
    let mut sent = 0;
    let mut write_closed = input.is_empty();
    let mut packet = Vec::new();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut cause = None;
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
        match cancellation() {
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
        match process::waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(Some(status)) => {
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
        std::thread::sleep(Duration::from_millis(2));
    }
    // The direct child has not been reaped: its PID keeps group identity reserved.
    // No group signal is ever issued after direct reaping, preventing PID reuse.
    let term = process::kill_process_group(pid, Signal::TERM);
    std::thread::sleep(Duration::from_millis(5));
    let kill = process::kill_process_group(pid, Signal::KILL);
    if term.is_err() && term != Err(rustix::io::Errno::SRCH) {
        cause = Some("scoped termination failed".into());
    }
    if kill.is_err() && kill != Err(rustix::io::Errno::SRCH) {
        cause = Some("scoped kill failed".into());
    }
    let mut pending = Pending {
        child,
        pid,
        direct_reaped: false,
        descendants: 0,
        descendant_pids: Vec::new(),
    };
    let cleanup_start = Instant::now();
    let mut complete = false;
    while cleanup_start.elapsed() < Duration::from_millis(500) {
        match pending.poll() {
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
    // Group termination/reaping precedes final drain, including inherited pipes.
    for _ in 0..70 {
        for result in [
            drain(&mut protocol, &mut packet, frame_limit),
            drain(&mut stdout, &mut out, 64 * 1024),
            drain(&mut stderr, &mut err, 64 * 1024),
        ] {
            if let Err(error) = result {
                cause = Some(error);
            }
        }
    }
    let mut cleanup = pending.cleanup();
    cleanup.pending = !complete;
    if !complete {
        cause = Some("cleanup incomplete: owned lease retained".into());
    }
    let output = Output {
        packet,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        cause,
        cleanup,
    };
    Ok((output, (!complete).then_some(pending)))
}
