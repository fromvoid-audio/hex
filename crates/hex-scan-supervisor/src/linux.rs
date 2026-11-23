//! Linux-only standalone executable ownership; never imported by a scan library.
use crate::unix::{GroupCleanupState, SupervisionStrategy, run_supervised};
use rustix::process::{self, Pid, Signal, WaitId, WaitIdOptions, WaitOptions};
use serde::Serialize;
use std::{path::Path, process::Child, time::Duration};

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
    pid: Option<Pid>,
    state: GroupCleanupState,
}

impl Pending {
    pub fn poll(&mut self) -> Result<bool, String> {
        if self.state.is_complete() {
            return Ok(true);
        }
        let Some(pid) = self.pid else {
            // An unsupported PID never authorizes a group signal or result.
            if self
                .child
                .try_wait()
                .map_err(|_| "unidentified child wait failed")?
                .is_none()
            {
                return Ok(false);
            }
            self.state.record_direct_reaped();
            self.state.record_complete();
            return Ok(true);
        };
        if !self.state.direct_reaped {
            let state = process::waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map_err(|_| "child observation failed")?;
            if state.is_none() {
                return Ok(false);
            }
            self.child.wait().map_err(|_| "direct reap failed")?;
            self.state.record_direct_reaped();
        }
        loop {
            match process::waitpgid(pid, WaitOptions::NOHANG) {
                Ok(Some((pid, _))) => {
                    self.state.record_descendant(pid.as_raw_nonzero().get());
                }
                Ok(None) => return Ok(false),
                Err(rustix::io::Errno::CHILD) => {
                    self.state.record_complete();
                    return Ok(true);
                }
                Err(_) => return Err("descendant reap failed".into()),
            }
        }
    }

    pub fn cleanup(&self) -> Cleanup {
        Cleanup {
            direct_reaped: self.state.direct_reaped,
            descendants_reaped: self.state.descendants,
            descendant_pids: self.state.descendant_pids.clone(),
            group_signals_scoped: self.state.group_scoped,
            pending: self.state.is_pending(),
        }
    }
}

struct LinuxSubreaperStrategy;

impl SupervisionStrategy for LinuxSubreaperStrategy {
    type Cleanup = Cleanup;
    type Pending = Pending;

    fn prepare_platform(&self) -> Result<(), String> {
        // This function exists only in this single-request dedicated executable.
        process::set_child_subreaper(Some(Pid::INIT)).map_err(|_| "subreaper unavailable")?;
        if process::child_subreaper().map_err(|_| "subreaper query failed")? != Some(Pid::INIT) {
            return Err("subreaper verification failed".into());
        }
        Ok(())
    }

    fn unsupported_pid_pending(&self, child: Child) -> Pending {
        Pending {
            child,
            pid: None,
            state: GroupCleanupState::new(false),
        }
    }

    fn terminate_and_pending(
        &self,
        child: Child,
        pid: Pid,
        _exited: bool,
        cause: &mut Option<String>,
    ) -> Pending {
        // The direct child has not been reaped: its PID keeps group identity reserved.
        // No group signal is ever issued after direct reaping, preventing PID reuse.
        let term = process::kill_process_group(pid, Signal::TERM);
        std::thread::sleep(Duration::from_millis(5));
        let kill = process::kill_process_group(pid, Signal::KILL);
        if term.is_err() && term != Err(rustix::io::Errno::SRCH) {
            *cause = Some("scoped termination failed".into());
        }
        if kill.is_err() && kill != Err(rustix::io::Errno::SRCH) {
            *cause = Some("scoped kill failed".into());
        }
        Pending {
            child,
            pid: Some(pid),
            state: GroupCleanupState::new(true),
        }
    }

    fn poll_pending(&self, pending: &mut Pending) -> Result<bool, String> {
        pending.poll()
    }

    fn cleanup_of(&self, pending: &Pending) -> Cleanup {
        pending.cleanup()
    }
}

/// Run one controlled helper request to completion and reap its process group.
/// `max_packet` is the caller format's protocol payload bound; the request and
/// the captured reply may each add the 12-byte frame header to it.
pub fn run(
    helper: &Path,
    module: &Path,
    request: u64,
    deadline: Duration,
    input: &[u8],
    max_packet: usize,
) -> Result<(Output, Option<Pending>), String> {
    let (raw, pending) = run_supervised(
        &LinuxSubreaperStrategy,
        helper,
        module,
        request,
        deadline,
        input,
        max_packet,
    )?;
    let output = Output {
        packet: raw.packet,
        stdout: raw.stdout,
        stderr: raw.stderr,
        cause: raw.cause,
        cleanup: raw.cleanup,
    };
    Ok((output, pending))
}
