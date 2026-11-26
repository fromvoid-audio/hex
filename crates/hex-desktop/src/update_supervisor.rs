//! Control-side custody and one-shot authorization for the sealed update helper.
//!
//! Preparation is blocking and belongs on a control worker, never GPUI's event
//! loop or the audio callback. The helper receives a fixed request over a pipe;
//! readiness is not approval. Dropping an unapproved handle closes the pipe.

use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin};
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(unix)]
const READY: &[u8] = b"hex-update-observer-ready-v1\n";
const APPROVAL: &[u8] = b"hex-update-approved-v1\n";
const MAX_RECEIPT: u64 = 8 * 1024 * 1024;
#[cfg(unix)]
const MAX_HELPER_FILE: u64 = 128 * 1024 * 1024;
const AVAILABILITY_INTERVAL: Duration = Duration::from_secs(2);
const POLL_SLICE: Duration = Duration::from_millis(50);
const DEFAULT_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

struct ExitSignal(Arc<(Mutex<bool>, Condvar)>);

impl Drop for ExitSignal {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.0;
        let mut finished = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        *finished = true;
        condvar.notify_all();
    }
}

/// Owned handle for the desktop update-control worker.
///
/// Signals shutdown and joins the worker thread (up to a bounded timeout)
/// before the desktop's shared publication lease is dropped.
pub(super) struct UpdateSupervisorHandle {
    stop: Arc<AtomicBool>,
    done: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
    join_timeout: Duration,
}

impl UpdateSupervisorHandle {
    /// Request shutdown and wait up to `join_timeout` for the worker to exit.
    /// Returns `true` when the worker thread has been joined (or was already joined).
    pub(super) fn shutdown(&mut self) -> bool {
        let Some(thread) = self.thread.take() else {
            return true;
        };
        self.stop.store(true, Ordering::Release);
        let (lock, condvar) = &*self.done;
        let deadline = Instant::now() + self.join_timeout;
        let mut finished = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        while !*finished {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            if remaining.is_zero() {
                break;
            }
            let (guard, result) = condvar
                .wait_timeout(finished, remaining)
                .unwrap_or_else(|poison| poison.into_inner());
            finished = guard;
            if result.timed_out() {
                break;
            }
        }
        let exited = *finished;
        drop(finished);
        if exited {
            let _ = thread.join();
            true
        } else {
            // Bounded wait expired; leave the thread detached rather than hanging process exit.
            false
        }
    }
}

impl Drop for UpdateSupervisorHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Outcome of the pre-audio next-launch pending update handoff check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StartupHandoffOutcome {
    SkippedByFlag,
    OtherInstanceRunning,
    NoPendingUpdate,
    HandedOff,
    Failed(String),
}

/// Pre-audio startup gate for next-launch pending updates.
///
/// When launched fresh without `--skip-pending-update` and no other Hex
/// instance holds the update lease, checks whether a verified pending update
/// receipt is staged for `target`. If so, prepares and approves the existing
/// signed update helper with `Action::Restart` so the new build opens
/// automatically once this process exits and releases its shared custody lease.
/// Any refusal falls through to normal startup without discarding user data.
pub(super) fn try_startup_pending_handoff(
    target: &Path,
    sole_instance: bool,
    skip_flag: bool,
) -> StartupHandoffOutcome {
    try_startup_pending_handoff_with(target, sole_instance, skip_flag, |target, selected| {
        let stop = AtomicBool::new(false);
        let prepared = prepare_with_stop(target, Action::Restart, selected, &stop)?;
        prepared.approve()
    })
}

fn try_startup_pending_handoff_with(
    target: &Path,
    sole_instance: bool,
    skip_flag: bool,
    handoff: impl FnOnce(&Path, &str) -> io::Result<()>,
) -> StartupHandoffOutcome {
    if skip_flag {
        return StartupHandoffOutcome::SkippedByFlag;
    }
    if !sole_instance {
        return StartupHandoffOutcome::OtherInstanceRunning;
    }
    let Ok(selected) = select_available_receipt(target) else {
        return StartupHandoffOutcome::NoPendingUpdate;
    };
    match handoff(target, &selected) {
        Ok(()) => StartupHandoffOutcome::HandedOff,
        Err(error) => StartupHandoffOutcome::Failed(error.to_string()),
    }
}

/// Start one desktop-owned worker for a bundle whose startup lease is held.
pub(super) fn start(
    target: PathBuf,
    worker: hex_ui::UpdateWorker,
) -> io::Result<UpdateSupervisorHandle> {
    use hex_ui::{UpdateAction, UpdateCommand, UpdateStatus};
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_stop = Arc::clone(&stop);
    let worker_done = Arc::clone(&done);
    let thread =
        std::thread::Builder::new()
            .name("hex-update-control".into())
            .spawn(move || {
                let _exit_signal = ExitSignal(worker_done);
                let mut delivered: Option<u64> = None;
                let mut selection: Option<(u64, String)> = None;
                let mut availability_cache = None;
                let previous = last_result(&target);
                let mut monitor_available = previous.is_none();
                if let Some((message, failed)) = previous {
                    worker.publish(UpdateStatus::PreviousResult { message, failed });
                }
                let mut next_availability_check = Instant::now() + AVAILABILITY_INTERVAL;
                loop {
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let now = Instant::now();
                    let wait = next_availability_check
                        .saturating_duration_since(now)
                        .min(POLL_SLICE);
                    let command = match worker.commands.recv_timeout(wait) {
                        Ok(command) => command,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if worker_stop.load(Ordering::Acquire) {
                                break;
                            }
                            let now = Instant::now();
                            if now >= next_availability_check {
                                next_availability_check = now + AVAILABILITY_INTERVAL;
                                if monitor_available && selection.is_none() && delivered.is_none() {
                                    let available =
                                        cached_availability(&target, &mut availability_cache)
                                            .unwrap_or(false);
                                    worker.publish(if available {
                                        UpdateStatus::Available
                                    } else {
                                        UpdateStatus::Idle
                                    });
                                }
                            }
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    match command {
                        UpdateCommand::DismissResult => {
                            monitor_available = true;
                            worker.publish(UpdateStatus::Idle);
                        }
                        UpdateCommand::Select(id) if delivered.is_none() && id != 0 => {
                            monitor_available = false;
                            match select_available_receipt(&target) {
                                Ok(digest) => {
                                    selection = Some((id, digest));
                                    worker.publish(UpdateStatus::Selected(id));
                                }
                                Err(error) => {
                                    selection = None;
                                    worker.publish(UpdateStatus::Failed(id, error.to_string()));
                                }
                            }
                        }
                        UpdateCommand::Select(id) => worker
                            .publish(UpdateStatus::Failed(id, "update already preparing".into())),
                        UpdateCommand::Prepare { id, action }
                            if delivered.is_none()
                                && selection
                                    .as_ref()
                                    .is_some_and(|(selected, _)| *selected == id) =>
                        {
                            worker.publish(UpdateStatus::Preparing(id));
                            let action = match action {
                                UpdateAction::InstallOnQuit => Action::Install,
                                UpdateAction::SaveAndRestart => Action::Restart,
                            };
                            let digest = selection
                                .as_ref()
                                .map(|(_, digest)| digest.as_str())
                                .unwrap_or("");
                            match prepare_with_stop(&target, action, digest, &worker_stop) {
                                Ok(handle) => match worker.deliver(hex_ui::UpdateReady {
                                    id,
                                    approval: Box::new(handle),
                                }) {
                                    Ok(()) => {
                                        delivered = Some(id);
                                        worker.publish(UpdateStatus::Ready(id));
                                    }
                                    Err(ready) => {
                                        drop(ready);
                                        worker.publish(UpdateStatus::Failed(
                                            id,
                                            "update handoff unavailable".into(),
                                        ));
                                    }
                                },
                                Err(error) => {
                                    worker.publish(UpdateStatus::Failed(id, error.to_string()))
                                }
                            }
                        }
                        UpdateCommand::Prepare { id, .. } => worker
                            .publish(UpdateStatus::Failed(id, "update already preparing".into())),
                        UpdateCommand::Cancel(id) => {
                            worker.revoke(id);
                            monitor_available = true;
                            if selection
                                .as_ref()
                                .is_some_and(|(selected, _)| *selected == id)
                            {
                                selection = None;
                                worker.publish(UpdateStatus::Idle);
                            }
                            if delivered == Some(id) {
                                delivered = None;
                                worker.publish(UpdateStatus::Idle);
                            }
                        }
                    }
                }
            })?;
    Ok(UpdateSupervisorHandle {
        stop,
        done,
        thread: Some(thread),
        join_timeout: DEFAULT_JOIN_TIMEOUT,
    })
}

impl hex_ui::UpdateApproval for Prepared {
    fn approve(self: Box<Self>) -> Result<(), String> {
        Prepared::approve(*self).map_err(|error| error.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Install,
    Restart,
}

pub(super) struct Prepared {
    child: Option<Child>,
    input: Option<ChildStdin>,
}

impl Prepared {
    /// Send only after the ordinary close path has irrevocably accepted quit.
    /// The helper additionally requires a normal exit and exclusive custody.
    /// Keeps `self.input` intact until both `write_all` and `flush` succeed so
    /// a write failure lets `Drop` signal and reap the helper process group.
    pub(super) fn approve(mut self) -> io::Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| invalid("update approval unavailable"))?;
        input.write_all(APPROVAL)?;
        input.flush()?;
        drop(self.input.take());
        Ok(())
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if self.input.take().is_some()
            && let Some(mut child) = self.child.take()
        {
            // The platform can stall process termination. Never wait on
            // the UI owner which cancels a prepared update.
            signal_child_group(&mut child);
            let _ = std::thread::Builder::new()
                .name("hex-update-reap".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
}

#[cfg(unix)]
fn signal_child_group(child: &mut Child) {
    // The unreaped child pins its PID until after the single group signal.
    // Never use its numeric group ID after wait/try_wait has reaped it.
    if let Ok(pid) = i32::try_from(child.id()) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn signal_child_group(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(unix)]
fn cancel_child_group(child: &mut Child) {
    signal_child_group(child);
    let _ = child.wait();
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Number of newest session directories retained under `.hex-update-runtime/`.
#[cfg(unix)]
const RETAINED_RUNTIME_SESSIONS: usize = 16;

/// Upper bound when scanning unpruned `.hex-update-runtime/` directories.
#[cfg(unix)]
const MAX_SCAN_RUNTIME_SESSIONS: usize = 4096;

#[cfg(unix)]
fn is_valid_session_dir_name(name: &str) -> bool {
    name.len() == 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Prune stale helper session directories in `.hex-update-runtime/`, retaining
/// the `retain` newest valid directories owned by `uid`.
#[cfg(unix)]
fn prune_runtime_sessions(root: &Path, uid: u32, retain: usize) {
    use std::os::unix::fs::MetadataExt;
    if plain_path(root).is_err() {
        return;
    }
    let Ok(root_meta) = fs::symlink_metadata(root) else {
        return;
    };
    if !root_meta.is_dir()
        || root_meta.file_type().is_symlink()
        || root_meta.uid() != uid
        || root_meta.mode() & 0o7777 != 0o700
    {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut sessions: Vec<(std::time::SystemTime, String, PathBuf)> = Vec::new();
    for entry in entries.take(MAX_SCAN_RUNTIME_SESSIONS).flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_valid_session_dir_name(name) {
            continue;
        }
        let session = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&session) else {
            continue;
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != uid
            || metadata.mode() & 0o7777 != 0o700
        {
            continue;
        }
        let stamp = fs::symlink_metadata(session.join("result.json"))
            .ok()
            .and_then(|meta| meta.modified().ok())
            .or_else(|| metadata.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        sessions.push((stamp, name.to_owned(), session));
    }
    if sessions.len() <= retain {
        return;
    }
    sessions.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, _, stale) in sessions.into_iter().skip(retain) {
        let _ = fs::remove_dir_all(stale);
    }
}

/// Surface the newest bounded helper result and prune stale runtime sessions
/// beyond [`RETAINED_RUNTIME_SESSIONS`] so results are never missed after 256 runs.
#[cfg(unix)]
fn last_result(target: &Path) -> Option<(&'static str, bool)> {
    use std::os::unix::fs::MetadataExt;
    let root = target.parent()?.join(".hex-update-runtime");
    plain_path(&root).ok()?;
    let root_meta = fs::symlink_metadata(&root).ok()?;
    if !root_meta.is_dir()
        || root_meta.mode() & 0o7777 != 0o700
        || root_meta.uid() != fs::metadata(target).ok()?.uid()
    {
        return None;
    }
    let mut newest = None;
    let mut valid_dirs = 0usize;
    for entry in fs::read_dir(&root)
        .ok()?
        .take(MAX_SCAN_RUNTIME_SESSIONS)
        .flatten()
    {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_valid_session_dir_name(name) {
            continue;
        }
        let session = entry.path();
        let metadata = match fs::symlink_metadata(&session) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != root_meta.uid()
            || metadata.mode() & 0o7777 != 0o700
        {
            continue;
        }
        valid_dirs += 1;
        let path = session.join("result.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != root_meta.uid()
            || metadata.len() > 4096
        {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(file) = File::open(path) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(4097).read_to_end(&mut bytes).is_err() {
            continue;
        }
        if bytes.len() > 4096 {
            continue;
        }
        let Ok(result) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if result.get("version").and_then(|value| value.as_u64()) != Some(1) {
            continue;
        }
        let value = match result.get("status").and_then(|value| value.as_str()) {
            Some("installed") => ("update installed", false),
            Some("restart_requested") => ("update installed · restart requested", false),
            Some("installed_restart_failed") => {
                ("update installed · restart failed · reopen hex", true)
            }
            Some("installed_followup_failed") => (
                "update installed · receipt failed · inspect before retry",
                true,
            ),
            Some("publication_uncertain") => {
                ("update outcome uncertain · inspect before retry", true)
            }
            Some("refused") => ("update failed · select it again to retry", true),
            _ => continue,
        };
        if newest
            .as_ref()
            .is_none_or(|(previous, _)| modified > *previous)
        {
            newest = Some((modified, value));
        }
    }
    if valid_dirs > RETAINED_RUNTIME_SESSIONS {
        prune_runtime_sessions(&root, root_meta.uid(), RETAINED_RUNTIME_SESSIONS);
    }
    newest.map(|(_, result)| result)
}

#[cfg(not(unix))]
fn last_result(_target: &Path) -> Option<(&'static str, bool)> {
    None
}

#[cfg(unix)]
fn private_directory(path: &Path, uid: u32) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(invalid("update custody is not private"));
    }
    Ok(())
}

fn plain_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(invalid("update path is not absolute"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(invalid("update path contains a link"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_bundle(source: &Path, target: &Path) -> io::Result<()> {
    copy_bundle_bounded(source, target, &mut 0, &mut 0, 0)
}

#[cfg(unix)]
fn copy_bundle_bounded(
    source: &Path,
    target: &Path,
    members: &mut usize,
    bytes: &mut u64,
    depth: usize,
) -> io::Result<()> {
    *members = members
        .checked_add(1)
        .ok_or_else(|| invalid("update helper is too large"))?;
    if *members > 1024 || depth > 16 {
        return Err(invalid("update helper is too large"));
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid("update helper contains a link"));
    }
    if metadata.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_bundle_bounded(
                &entry.path(),
                &target.join(entry.file_name()),
                members,
                bytes,
                depth + 1,
            )?;
        }
    } else if metadata.is_file() && metadata.len() <= MAX_HELPER_FILE {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        let remaining = MAX_HELPER_FILE.saturating_sub(*bytes);
        let copied = io::copy(
            &mut Read::by_ref(&mut input).take(remaining + 1),
            &mut output,
        )?;
        *bytes = bytes
            .checked_add(copied)
            .ok_or_else(|| invalid("update helper is too large"))?;
        if copied > MAX_HELPER_FILE || *bytes > 128 * 1024 * 1024 {
            return Err(invalid("update helper file is too large"));
        }
        fs::set_permissions(target, metadata.permissions())?;
    } else {
        return Err(invalid("invalid update helper member"));
    }
    Ok(())
}

#[cfg(unix)]
fn run_checked(command: &mut Command, timeout: Duration, stop: &AtomicBool) -> io::Result<()> {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if stop.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "update preparation cancelled by shutdown",
            ));
        }
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(invalid("update helper signature refused"))
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "update helper verification timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

fn receipt_paths(target: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let parent = target
        .parent()
        .ok_or_else(|| invalid("invalid update target"))?;
    Ok((
        parent.join(".hex-pending-update/hex.app/Contents/Resources/test-build.json"),
        target.join("Contents/Resources/test-build.json"),
    ))
}

fn receipt_digest(receipt: &Path) -> io::Result<String> {
    plain_path(receipt)?;
    let metadata = fs::metadata(receipt)?;
    if !metadata.is_file() || metadata.len() > MAX_RECEIPT {
        return Err(invalid("invalid pending update receipt"));
    }
    let mut bytes = Vec::new();
    File::open(receipt)?
        .take(MAX_RECEIPT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECEIPT {
        return Err(invalid("pending update receipt is too large"));
    }
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(unix)]
fn selected_receipt(target: &Path) -> io::Result<String> {
    receipt_digest(&receipt_paths(target)?.0)
}

fn select_available_receipt(target: &Path) -> io::Result<String> {
    let (pending, installed) = receipt_paths(target)?;
    let selected = receipt_digest(&pending)?;
    if selected == receipt_digest(&installed)? {
        return Err(invalid("selected update is already installed"));
    }
    Ok(selected)
}

#[derive(Clone, PartialEq, Eq)]
struct ReceiptFingerprint {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn receipt_fingerprint(path: &Path) -> io::Result<ReceiptFingerprint> {
    plain_path(path)?;
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_RECEIPT {
        return Err(invalid("invalid update receipt"));
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(ReceiptFingerprint {
        len: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}

fn cached_availability(
    target: &Path,
    cache: &mut Option<((ReceiptFingerprint, ReceiptFingerprint), bool)>,
) -> io::Result<bool> {
    let (pending, installed) = receipt_paths(target)?;
    let fingerprints = (
        receipt_fingerprint(&pending)?,
        receipt_fingerprint(&installed)?,
    );
    if let Some((old, available)) = cache
        && *old == fingerprints
    {
        return Ok(*available);
    }
    let available = receipt_digest(&pending)? != receipt_digest(&installed)?;
    *cache = Some((fingerprints, available));
    Ok(available)
}

#[cfg(all(test, unix))]
pub(super) fn prepare(target: &Path, action: Action, selected: &str) -> io::Result<Prepared> {
    prepare_with_stop(target, action, selected, &AtomicBool::new(false))
}

#[cfg(unix)]
fn prepare_with_stop(
    target: &Path,
    action: Action,
    selected: &str,
    stop: &AtomicBool,
) -> io::Result<Prepared> {
    use std::os::unix::fs::MetadataExt;
    if stop.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "update preparation cancelled by shutdown",
        ));
    }
    plain_path(target)?;
    if target
        .file_name()
        .is_none_or(|name| !name.eq_ignore_ascii_case("hex.app"))
    {
        return Err(invalid("invalid update target"));
    }
    let parent = target
        .parent()
        .ok_or_else(|| invalid("invalid update target"))?;
    let lock = parent.join(".hex-test-build.lock");
    plain_path(&lock)?;
    if !fs::symlink_metadata(&lock)?.is_file() {
        return Err(invalid("installed app has no update custody"));
    }
    if selected_receipt(target)? != selected {
        return Err(invalid("pending update selection changed"));
    }

    // The installed app holds the cooperative lock while this copy is made.
    // The signed copy is independently verified before any of its code runs.
    let source = target.join("Contents/Helpers/hex-update.app");
    plain_path(&source)?;
    let uid = current_uid();
    let root = parent.join(".hex-update-runtime");
    private_directory(&root, uid)?;
    prune_runtime_sessions(&root, uid, RETAINED_RUNTIME_SESSIONS.saturating_sub(1));
    let mut random = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let session = root.join(
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    );
    // A random name is unique, not permission to reuse an existing directory.
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(&session)?;
    if fs::symlink_metadata(&session)?.uid() != uid {
        return Err(invalid("update session owner differs"));
    }
    let bundle = session.join("hex-update.app");
    copy_bundle(&source, &bundle)?;
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../scripts/hex-test-build.json"))
            .map_err(|_| invalid("invalid update signing configuration"))?;
    let signing = config
        .get("signing")
        .ok_or_else(|| invalid("missing update signing configuration"))?;
    let certificate = signing
        .get("certificate_sha1")
        .and_then(|value| value.as_str())
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| invalid("invalid update signing certificate"))?;
    let team = signing
        .get("team_identifier")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .ok_or_else(|| invalid("invalid update signing team"))?;
    let requirement = format!(
        "=identifier \"org.portalsurfer.hex.update\" and anchor apple generic and certificate leaf[subject.OU] = \"{team}\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf = H\"{certificate}\""
    );
    run_checked(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict", "--test-requirement", &requirement])
            .arg(&bundle),
        Duration::from_secs(30),
        stop,
    )?;
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(bundle.join("Contents/MacOS/hex-update"));
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let Some(mut input) = child.stdin.take() else {
        cancel_child_group(&mut child);
        return Err(invalid("update helper input unavailable"));
    };
    let Some(output) = child.stdout.take() else {
        cancel_child_group(&mut child);
        return Err(invalid("update helper output unavailable"));
    };
    let request = serde_json::json!({"parent_pid": std::process::id(), "target": target,
        "selected_receipt": selected, "action": match action { Action::Install => "install", Action::Restart => "restart" }});
    if let Err(error) = writeln!(input, "{request}").and_then(|()| input.flush()) {
        cancel_child_group(&mut child);
        return Err(error);
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = std::thread::Builder::new()
        .name("hex-update-ready".into())
        .spawn(move || {
            let mut line = Vec::new();
            let result = BufReader::new(output)
                .take(128)
                .read_until(b'\n', &mut line)
                .map(|_| line);
            let _ = sender.send(result);
        });
    if let Err(error) = reader {
        cancel_child_group(&mut child);
        return Err(error);
    }
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if stop.load(Ordering::Acquire) {
            drop(input);
            cancel_child_group(&mut child);
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "update preparation cancelled by shutdown",
            ));
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            drop(input);
            cancel_child_group(&mut child);
            return Err(invalid("update helper did not become ready"));
        };
        match receiver.recv_timeout(remaining.min(POLL_SLICE)) {
            Ok(Ok(line)) if line == READY => {
                return Ok(Prepared {
                    child: Some(child),
                    input: Some(input),
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            _ => {
                drop(input);
                cancel_child_group(&mut child);
                return Err(invalid("update helper did not become ready"));
            }
        }
    }
}

#[cfg(not(unix))]
fn prepare_with_stop(
    _target: &Path,
    _action: Action,
    _selected: &str,
    _stop: &AtomicBool,
) -> io::Result<Prepared> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "local update handoff requires macos",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!(
                    "hex-supervisor-test-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn receipt_identity_matches_legacy_digest() {
        let fixture = Fixture::new();
        let receipt = fixture.0.join("test-build.json");
        fs::write(&receipt, b"abc").unwrap();
        assert_eq!(
            receipt_digest(&receipt).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn private_custody_rejects_link_and_wrong_permissions() {
        let fixture = Fixture::new();
        let uid = fs::metadata(&fixture.0).unwrap().uid();
        let root = fixture.0.join("runtime");
        private_directory(&root, uid).unwrap();
        assert_eq!(fs::metadata(&root).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_directory(&root, uid).is_err());
        let link = fixture.0.join("link");
        symlink(&root, &link).unwrap();
        assert!(private_directory(&link, uid).is_err());
    }

    #[test]
    fn sealed_copy_refuses_links_without_touching_their_targets() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("executable"), b"fixture").unwrap();
        let destination = fixture.0.join("copied");
        copy_bundle(&source, &destination).unwrap();
        assert_eq!(
            fs::read(destination.join("executable")).unwrap(),
            b"fixture"
        );
        let protected = fixture.0.join("protected");
        fs::write(&protected, b"keep").unwrap();
        symlink(&protected, source.join("link")).unwrap();
        assert!(copy_bundle(&source, &fixture.0.join("refused")).is_err());
        assert_eq!(fs::read(protected).unwrap(), b"keep");
    }

    #[test]
    fn superseded_selection_refuses_before_helper_launch() {
        let fixture = Fixture::new();
        let target = fixture.0.join("hex.app");
        fs::create_dir(&target).unwrap();
        fs::write(fixture.0.join(".hex-test-build.lock"), b"").unwrap();
        let resources = fixture
            .0
            .join(".hex-pending-update/hex.app/Contents/Resources");
        fs::create_dir_all(&resources).unwrap();
        let receipt = resources.join("test-build.json");
        fs::write(&receipt, b"first").unwrap();
        let selected = selected_receipt(&target).unwrap();
        fs::write(&receipt, b"second").unwrap();
        let error = match prepare(&target, Action::Install, &selected) {
            Ok(_) => panic!("superseded selection must refuse"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("selection changed"));
        assert!(!fixture.0.join(".hex-update-runtime").exists());
    }

    #[test]
    fn installed_candidate_is_not_advertised_on_next_startup() {
        let fixture = Fixture::new();
        let target = fixture.0.join("hex.app");
        let (pending, installed) = receipt_paths(&target).unwrap();
        fs::create_dir_all(pending.parent().unwrap()).unwrap();
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::write(&pending, b"first candidate").unwrap();
        fs::write(&installed, b"old candidate").unwrap();
        let mut cache = None;
        assert!(cached_availability(&target, &mut cache).unwrap());
        assert!(select_available_receipt(&target).is_ok());
        fs::write(&installed, b"first candidate").unwrap();
        assert!(!cached_availability(&target, &mut cache).unwrap());
        assert!(select_available_receipt(&target).is_err());
        assert_eq!(fs::read(&pending).unwrap(), b"first candidate");
    }

    #[test]
    fn reports_owned_failure_result_without_following_linked_files() {
        let fixture = Fixture::new();
        let target = fixture.0.join("hex.app");
        fs::create_dir(&target).unwrap();
        let uid = fs::metadata(&target).unwrap().uid();
        let root = fixture.0.join(".hex-update-runtime");
        private_directory(&root, uid).unwrap();
        let session = root.join("0123456789abcdef0123456789abcdef");
        private_directory(&session, uid).unwrap();
        let result = session.join("result.json");
        fs::write(&result, br#"{"version":1,"status":"refused"}"#).unwrap();
        assert_eq!(
            last_result(&target),
            Some(("update failed · select it again to retry", true))
        );
        fs::remove_file(&result).unwrap();
        let unrelated = fixture.0.join("user-file");
        fs::write(&unrelated, br#"{"version":1,"status":"installed"}"#).unwrap();
        symlink(&unrelated, result).unwrap();
        assert_eq!(last_result(&target), None);
        assert!(unrelated.exists());
    }

    #[test]
    fn cancellation_reaps_the_helper_group_including_its_child() {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30 & echo ready; wait"])
            .process_group(0)
            .stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let mut line = String::new();
            let first = reader.read_line(&mut line);
            let _ = sender.send((first.is_ok(), line));
            let mut tail = Vec::new();
            let _ = sender.send((reader.read_to_end(&mut tail).is_ok(), String::new()));
        });
        let ready = receiver.recv_timeout(Duration::from_secs(30));
        cancel_child_group(&mut child);
        assert_eq!(ready.unwrap(), (true, "ready\n".into()));
        assert!(receiver.recv_timeout(Duration::from_secs(5)).unwrap().0);
    }

    #[test]
    fn supervisor_handle_joins_promptly_on_shutdown_and_channel_drop() {
        let fixture = Fixture::new();
        let target = fixture.0.join("hex.app");
        fs::create_dir(&target).unwrap();

        // Case 1: channel disconnects before shutdown -> immediate join.
        let (client, worker) = hex_ui::update_channel();
        let mut handle = start(target.clone(), worker).unwrap();
        drop(client);
        let started = Instant::now();
        assert!(handle.shutdown());
        assert!(started.elapsed() < Duration::from_millis(500));
        // Repeated shutdown is idempotent.
        assert!(handle.shutdown());

        // Case 2: client remains alive (e.g. still held in GPUI global on macOS quit)
        // -> stop flag wakes the poll slice and joins promptly without hanging for 2s.
        let (client, worker) = hex_ui::update_channel();
        let mut handle = start(target, worker).unwrap();
        let started = Instant::now();
        assert!(handle.shutdown());
        assert!(started.elapsed() < Duration::from_millis(500));
        drop(client);
    }

    #[test]
    fn supervisor_handle_shutdown_respects_bounded_timeout() {
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_done = Arc::clone(&done);
        let thread = std::thread::Builder::new()
            .name("hex-update-timeout-test".into())
            .spawn(move || {
                let _exit = ExitSignal(worker_done);
                let _ = release_rx.recv();
            })
            .unwrap();
        let mut handle = UpdateSupervisorHandle {
            stop,
            done,
            thread: Some(thread),
            join_timeout: Duration::from_millis(40),
        };
        let started = Instant::now();
        assert!(
            !handle.shutdown(),
            "blocked worker must not hang past bounded timeout"
        );
        assert!(started.elapsed() < Duration::from_millis(400));
        let _ = release_tx.send(());
    }

    #[test]
    fn prepared_approve_write_failure_kills_and_reaps_helper_child() {
        use std::os::unix::process::CommandExt;
        // Close stdin in the child immediately, then sleep so the child stays alive
        // unless Prepared's Drop signals and reaps it on approve() write failure.
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "exec 0<&-; echo closed; sleep 30"])
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let mut line = String::new();
        BufReader::new(output).read_line(&mut line).unwrap();
        assert_eq!(line, "closed\n");

        let prepared = Prepared {
            child: Some(child),
            input: Some(input),
        };
        let err = prepared.approve().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);

        // Drop must have signaled the child group and reaped the child promptly.
        let deadline = Instant::now() + Duration::from_secs(2);
        while nix::sys::signal::kill(pid, None).is_ok() {
            assert!(
                Instant::now() < deadline,
                "helper child {pid} leaked alive after approve() write failure"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn runtime_sessions_pruned_and_last_result_finds_newest_beyond_256_dirs() {
        let fixture = Fixture::new();
        let target = fixture.0.join("hex.app");
        fs::create_dir(&target).unwrap();
        let root = fixture.0.join(".hex-update-runtime");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

        let total = 270usize;
        for index in 0..total {
            let name = format!("{index:032x}");
            let session = root.join(&name);
            fs::create_dir(&session).unwrap();
            fs::set_permissions(&session, fs::Permissions::from_mode(0o700)).unwrap();
            if index + 1 == total {
                // Ensure the newest directory has a strictly newer mtime.
                std::thread::sleep(Duration::from_millis(15));
                let result_path = session.join("result.json");
                fs::write(&result_path, r#"{"version":1,"status":"installed"}"#).unwrap();
            }
        }

        // Must find the newest result even with >256 session dirs, and prune stale dirs.
        assert_eq!(last_result(&target), Some(("update installed", false)));
        let remaining: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(remaining.len(), RETAINED_RUNTIME_SESSIONS);
        let newest_name = format!("{:032x}", total - 1);
        assert!(
            remaining.contains(&newest_name),
            "pruning must retain the newest session directory"
        );
    }

    #[test]
    fn startup_handoff_gate_respects_flag_sole_instance_and_receipt_state() {
        let fixture = Fixture::new();
        let target = fixture.0.join("hex.app");
        let (pending, installed) = receipt_paths(&target).unwrap();
        fs::create_dir_all(pending.parent().unwrap()).unwrap();
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::write(&installed, b"installed-v1").unwrap();

        // No staged pending candidate yet -> NoPendingUpdate.
        assert_eq!(
            try_startup_pending_handoff_with(&target, true, false, |_, _| {
                panic!("handoff must not run without pending update");
            }),
            StartupHandoffOutcome::NoPendingUpdate
        );

        // Stage a newer candidate.
        fs::write(&pending, b"pending-v2").unwrap();
        let expected_digest = receipt_digest(&pending).unwrap();

        // --skip-pending-update opt-out skips handoff.
        assert_eq!(
            try_startup_pending_handoff_with(&target, true, true, |_, _| {
                panic!("handoff must not run when skipped by flag");
            }),
            StartupHandoffOutcome::SkippedByFlag
        );

        // Another Hex instance holds the shared lease -> refuse handoff.
        assert_eq!(
            try_startup_pending_handoff_with(&target, false, false, |_, _| {
                panic!("handoff must not run while another instance holds lease");
            }),
            StartupHandoffOutcome::OtherInstanceRunning
        );

        // Sole instance + newer pending candidate -> invokes handoff with exact digest.
        let mut seen = None;
        let outcome = try_startup_pending_handoff_with(&target, true, false, |t, digest| {
            seen = Some((t.to_path_buf(), digest.to_owned()));
            Ok(())
        });
        assert_eq!(outcome, StartupHandoffOutcome::HandedOff);
        assert_eq!(seen, Some((target.clone(), expected_digest)));

        // Helper error falls back cleanly without panicking.
        let failed = try_startup_pending_handoff_with(&target, true, false, |_, _| {
            Err(invalid("helper verification refused"))
        });
        assert_eq!(
            failed,
            StartupHandoffOutcome::Failed("helper verification refused".into())
        );

        // Once installed receipt matches pending receipt -> NoPendingUpdate (no loop).
        fs::write(&installed, b"pending-v2").unwrap();
        assert_eq!(
            try_startup_pending_handoff_with(&target, true, false, |_, _| {
                panic!("already-installed candidate must not loop");
            }),
            StartupHandoffOutcome::NoPendingUpdate
        );
    }
}
