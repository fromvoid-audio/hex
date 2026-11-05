//! Background control work. The standalone scanner alone owns native children.
use crate::{Catalog, validate_report};
use hex_plugin_scan::macos::{
    identity::Bundle,
    package::Package,
    report::{Cleanup, Report},
};
use std::{
    io::{Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const ERROR_LIMIT: usize = 64 * 1024;
const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const FINISHED: u8 = 3;

/// Per-bundle native scan time, excluding the two-second cleanup allowance.
/// The range matches the bundled scanner's accepted millisecond argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanDeadline(u32);

impl ScanDeadline {
    /// Reject values the bundled scanner cannot accept.
    pub fn from_millis(milliseconds: u32) -> Result<Self, &'static str> {
        if !(1..=30_000).contains(&milliseconds) {
            return Err("scan deadline must be 1 to 30000 milliseconds");
        }
        Ok(Self(milliseconds))
    }

    fn outer_limit(self) -> Duration {
        Duration::from_millis(u64::from(self.0)) + Duration::from_secs(2)
    }

    fn expired(self, elapsed: Duration) -> bool {
        elapsed > self.outer_limit()
    }
}

impl Default for ScanDeadline {
    fn default() -> Self {
        Self(10_000)
    }
}

fn scanner_command(scanner: &Path, module: &Path, deadline: ScanDeadline) -> Command {
    let mut command = Command::new(scanner);
    command
        .args(["--bundled", "--trust-user-code"])
        .arg(module)
        .arg(deadline.0.to_string());
    command
}

struct Operation {
    phase: AtomicU8,
    scanner_started: AtomicBool,
}
/// UI/control owner. Dropping a view requests cancellation without waiting.
pub struct Cancellation(Arc<Operation>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(Arc::new(Operation {
            phase: AtomicU8::new(RUNNING),
            scanner_started: AtomicBool::new(false),
        }))
    }
}
impl Cancellation {
    pub fn token(&self) -> Token {
        Token(Arc::clone(&self.0))
    }
    /// False means publication has already started or the operation is finished.
    pub fn cancel(&self) -> bool {
        matches!(
            self.0
                .phase
                .compare_exchange(RUNNING, CANCELLED, Ordering::AcqRel, Ordering::Acquire),
            Ok(_) | Err(CANCELLED)
        )
    }
    /// Historical observation of this operation's actual outer scanner spawn.
    /// This proves neither current liveness nor native child entry/retirement.
    pub fn has_started_scanner(&self) -> bool {
        self.0.scanner_started.load(Ordering::Acquire)
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}
pub struct Token(Arc<Operation>);
impl Token {
    fn cancelled(&self) -> bool {
        self.0.phase.load(Ordering::Acquire) == CANCELLED
    }
    fn begin_publication(&self) -> Result<bool, String> {
        match self.0.phase.compare_exchange(
            RUNNING,
            COMMITTING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(true),
            Err(CANCELLED) => {
                self.0.phase.store(COMMITTING, Ordering::Release);
                Ok(false)
            }
            Err(_) => Err("scan publication state invalid".into()),
        }
    }
}
impl Drop for Token {
    fn drop(&mut self) {
        self.0.phase.store(FINISHED, Ordering::Release);
    }
}

/// Copied observations, never child custody or permission to publish metadata.
/// Keeps the original scanner output buffer (at most OUTPUT_LIMIT logical bytes),
/// rather than cloning decoded metadata. Outer exit and reported native cleanup
/// stay distinct; malformed output remains available without implying cleanup.
#[derive(Default)]
pub struct ScanDiagnostics {
    pub module: PathBuf,
    pub elapsed: Duration,
    pub scanner_started: bool,
    pub scanner_exit: Option<std::process::ExitStatus>,
    pub native_cleanup: Option<Cleanup>,
    pub report: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl std::fmt::Debug for ScanDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanDiagnostics")
            .field("module", &self.module)
            .field("elapsed", &self.elapsed)
            .field("scanner_started", &self.scanner_started)
            .field("scanner_exit", &self.scanner_exit)
            .field("native_cleanup", &self.native_cleanup)
            .field("report", &String::from_utf8_lossy(&self.report))
            .field("stderr", &String::from_utf8_lossy(&self.stderr))
            .finish()
    }
}

pub struct Update {
    pub catalog: Option<Catalog>,
    pub status: String,
    pub diagnostics: Option<ScanDiagnostics>,
}

fn discovery_app_path(executable: &Path, launch_app: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(contents) = executable.parent().and_then(Path::parent)
        && contents.file_name().and_then(|name| name.to_str()) == Some("Contents")
    {
        return contents
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "app bundle unavailable".into());
    }
    launch_app
        .filter(|app| app.is_absolute())
        .map(Path::to_path_buf)
        .ok_or_else(|| "packaged vst3 tools unavailable in this build".into())
}

fn drain(input: &mut UnixStream, output: &mut Vec<u8>, limit: usize) -> Result<bool, String> {
    let mut buffer = [0_u8; 4096];
    let mut any = false;
    for _ in 0..4 {
        match input.read(&mut buffer) {
            Ok(0) => return Ok(any),
            Ok(n) => {
                any = true;
                if output.len().checked_add(n).is_none_or(|v| v > limit) {
                    return Err("scanner output bound exceeded".into());
                }
                output.extend_from_slice(&buffer[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(any),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("scanner output unavailable: {e}")),
        }
    }
    Ok(any)
}

// Observe direct-child retirement before deciding that currently empty channels
// are final. A child can publish its last report between two parent operations.
fn poll_and_drain<T>(
    exit: &mut Option<T>,
    observe: impl FnOnce() -> std::io::Result<Option<T>>,
    streams: [(&mut UnixStream, &mut Vec<u8>, usize); 2],
) -> Result<(bool, Option<String>), String> {
    if exit.is_none() {
        match observe() {
            Ok(status) => *exit = status,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                return Err(format!(
                    "scanner custody lost; cleanup unconfirmed; no further process operations: {e}"
                ));
            }
        }
    }
    let mut received = false;
    let mut error = None;
    for (stream, bytes, maximum) in streams {
        match drain(stream, bytes, maximum) {
            Ok(value) => received |= value,
            Err(value) => {
                error.get_or_insert(value);
            }
        }
    }
    Ok((received, error))
}

struct DiscoveryOutcome {
    result: Result<(Report, String), String>,
    chain_retired: bool,
    diagnostics: ScanDiagnostics,
    /// Bundle identity inspected before any code ran, when inspection succeeded.
    bundle: Option<Bundle>,
}
impl DiscoveryOutcome {
    /// Identity to remember with a failure. Only a failure after the scanner
    /// started this bundle is attributed to it; tool/setup failures before the
    /// launch stay unremembered so the next scan retries them.
    fn failure_identity(&self) -> Option<Bundle> {
        self.bundle
            .clone()
            .filter(|_| self.diagnostics.scanner_started)
    }
}
fn discover(app: &Path, path: &Path, token: &Token, deadline: ScanDeadline) -> DiscoveryOutcome {
    let started = Instant::now();
    let mut chain_retired = true; // No child has been started yet.
    let mut bundle = None;
    let mut diagnostics = ScanDiagnostics {
        module: path.to_path_buf(),
        ..ScanDiagnostics::default()
    };
    let result = discover_inner(
        app,
        path,
        token,
        deadline,
        &mut chain_retired,
        &mut diagnostics,
        &mut bundle,
    );
    diagnostics.elapsed = started.elapsed();
    DiscoveryOutcome {
        result,
        chain_retired,
        diagnostics,
        bundle,
    }
}
fn discover_inner(
    app: &Path,
    path: &Path,
    token: &Token,
    deadline: ScanDeadline,
    chain_retired: &mut bool,
    diagnostics: &mut ScanDiagnostics,
    bundle: &mut Option<Bundle>,
) -> Result<(Report, String), String> {
    if token.cancelled() {
        return Err("scan cancelled".into());
    }
    let package = Package::inspect(app)?;
    let initial = Bundle::inspect(path)?;
    *bundle = Some(initial.clone());
    let (mut output, child_output) =
        UnixStream::pair().map_err(|e| format!("scanner output channel failed: {e}"))?;
    let (mut diagnostic, child_diagnostic) =
        UnixStream::pair().map_err(|e| format!("scanner diagnostic channel failed: {e}"))?;
    let (mut cancel, child_cancel) =
        UnixStream::pair().map_err(|e| format!("scanner cancel channel failed: {e}"))?;
    for stream in [&output, &diagnostic, &cancel] {
        stream
            .set_nonblocking(true)
            .map_err(|e| format!("scanner channel setup failed: {e}"))?;
    }
    let mut child = scanner_command(&package.scanner, path, deadline)
        .stdin(Stdio::from(OwnedFd::from(child_cancel)))
        .stdout(Stdio::from(OwnedFd::from(child_output)))
        .stderr(Stdio::from(OwnedFd::from(child_diagnostic)))
        .spawn()
        .map_err(|e| format!("scanner start failed: {e}"))?;
    *chain_retired = false;
    diagnostics.scanner_started = true;
    token.0.scanner_started.store(true, Ordering::Release);
    let started = Instant::now();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut cause = None;
    let mut cancel_sent = false;
    let mut exit = None;
    // No numeric PID/group operation other than this direct Child's try_wait.
    // A retained scanner may exceed its deadline while it retires a native child.
    loop {
        let polled = poll_and_drain(
            &mut exit,
            || child.try_wait(),
            [
                (&mut output, &mut out, OUTPUT_LIMIT),
                (&mut diagnostic, &mut err, ERROR_LIMIT),
            ],
        );
        diagnostics.scanner_exit = exit;
        let (received, error) = match polled {
            Ok(value) => value,
            Err(error) => {
                diagnostics.report = out;
                diagnostics.stderr = err;
                return Err(error);
            }
        };
        if let Some(error) = error {
            cause.get_or_insert(error);
        }
        if (token.cancelled() || cause.is_some() || deadline.expired(started.elapsed()))
            && !cancel_sent
        {
            cause.get_or_insert_with(|| {
                if token.cancelled() {
                    "scan cancelled".into()
                } else {
                    "scanner deadline exceeded; cleanup retained".into()
                }
            });
            match cancel.write(&[1]) {
                Ok(1) => cancel_sent = true,
                Ok(_) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => cancel_sent = true,
            }
        }
        if exit.is_some() && !received {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let decoded = serde_json::from_slice::<Report>(&out);
    diagnostics.report = out;
    diagnostics.stderr = err;
    diagnostics.native_cleanup = decoded.as_ref().ok().map(|r| r.cleanup.clone());
    *chain_retired = decoded.as_ref().is_ok_and(|report| {
        report.cleanup.direct_reaped
            && !report.cleanup.pending
            && !report.cleanup.custody_lost
            && !report.cleanup.cleanup_unconfirmed
    });
    if token.cancelled() {
        cause.get_or_insert_with(|| "scan cancelled".into());
    }
    if let Some(error) = cause {
        return Err(error);
    }
    if !exit.is_some_and(|s| s.success()) {
        let detail = decoded
            .as_ref()
            .ok()
            .and_then(|r| r.error.as_deref())
            .map(std::borrow::Cow::Borrowed)
            .unwrap_or_else(|| String::from_utf8_lossy(&diagnostics.stderr));
        let detail = detail
            .chars()
            .filter(|c| !c.is_control())
            .take(128)
            .collect::<String>();
        return Err(if detail.is_empty() {
            "scanner failed; last good catalog retained".into()
        } else {
            format!("scan failed · {detail}")
        });
    }
    let report = decoded.map_err(|_| "scanner returned malformed or incomplete metadata")?;
    validate_report(&report)?;
    if report.bundle != initial
        || report.receipt_sha256 != package.build.receipt_sha256
        || report.helper_sha256 != package.build.helper_sha256
    {
        return Err("scanner result differs from its selected input/tool identity".into());
    }
    package.build.unchanged()?;
    initial.unchanged()?;
    initial.executable_unchanged(
        report
            .executable
            .as_ref()
            .ok_or("scanner executable identity missing")?,
    )?;
    let fresh = Package::inspect(app)?;
    if fresh.source != package.source || fresh.build.receipt_sha256 != package.build.receipt_sha256
    {
        return Err("discovery package changed during scan".into());
    }
    Ok((report, package.source))
}

/// One selected scan. Caller runs this on a background control executor and
/// retains the cancellation owner until this result arrives.
pub fn scan(catalog_path: &Path, app: &Path, path: PathBuf, token: Token) -> Update {
    scan_with_deadline(catalog_path, app, path, token, ScanDeadline::default())
}

/// Scan one selected bundle with a validated per-bundle deadline.
pub fn scan_with_deadline(
    catalog_path: &Path,
    app: &Path,
    path: PathBuf,
    token: Token,
    deadline: ScanDeadline,
) -> Update {
    if !crate::path_valid(&path) || path.extension().and_then(|s| s.to_str()) != Some("vst3") {
        return Update {
            catalog: None,
            diagnostics: None,
            status: "select one absolute bounded .vst3 bundle path".into(),
        };
    }
    match Catalog::load(catalog_path) {
        Ok(catalog)
            if catalog.entries.len() == crate::MAX_MODULES
                && !catalog.entries.iter().any(|entry| entry.path == path) =>
        {
            return Update {
                catalog: None,
                diagnostics: None,
                status: "plugin catalog module capacity reached".into(),
            };
        }
        Ok(_) => {}
        Err(error) => {
            return Update {
                catalog: None,
                diagnostics: None,
                status: error,
            };
        }
    }
    // An explicit selection always runs, even for a remembered failure.
    let outcome = discover(app, &path, &token, deadline);
    let bundle = outcome.failure_identity();
    let DiscoveryOutcome {
        result,
        diagnostics,
        ..
    } = outcome;
    // The atomic transition is the cancellation/publication linearization point.
    let result = match token.begin_publication() {
        Ok(true) => result,
        Ok(false) => Err("scan cancelled".into()),
        Err(status) => {
            return Update {
                catalog: None,
                diagnostics: Some(diagnostics),
                status,
            };
        }
    };
    let status = match &result {
        Ok(_) => "vst3 discovered · hosting unavailable".into(),
        Err(error) => error.clone(),
    };
    match crate::storage::update(catalog_path, |catalog| match result {
        Ok(found) => catalog.record(path, Ok(found)),
        Err(error) => catalog.record_failure(path, error, bundle),
    }) {
        Ok(publication) => Update {
            catalog: Some(publication.catalog),
            diagnostics: Some(diagnostics),
            status: publication.warning.unwrap_or(status),
        },
        Err(error) => Update {
            catalog: None,
            diagnostics: Some(diagnostics),
            status: format!("{status} · catalog update failed: {error}"),
        },
    }
}

/// Explicit trusted standard folders. Resolution and enumeration stay on control.
pub fn scan_standard(catalog_path: &Path, app: &Path, token: Token) -> Update {
    scan_standard_with_deadline(catalog_path, app, token, ScanDeadline::default())
}

/// Scan trusted standard folders with a validated per-bundle deadline.
pub fn scan_standard_with_deadline(
    catalog_path: &Path,
    app: &Path,
    token: Token,
    deadline: ScanDeadline,
) -> Update {
    match crate::roots::macos_standard_paths() {
        Ok(roots) => scan_folders_with_deadline(catalog_path, app, &roots, token, deadline),
        Err(status) => Update {
            catalog: None,
            diagnostics: None,
            status,
        },
    }
}

/// One bounded batch; each native chain retires before the next can start.
/// A bundle that previously failed with an unchanged identity is skipped without
/// running its code. A bundle whose native cleanup is unconfirmed is recorded as
/// a failure and stops further launches, but every result already obtained is
/// still published. Cancelled batches never promote their pending metadata. The
/// short final writer transaction merges fresh state, preserving other windows'
/// records.
pub fn scan_folders(catalog_path: &Path, app: &Path, roots: &[PathBuf], token: Token) -> Update {
    scan_folders_with_deadline(catalog_path, app, roots, token, ScanDeadline::default())
}

/// Scan a bounded folder batch with the same deadline for each bundle.
pub fn scan_folders_with_deadline(
    catalog_path: &Path,
    app: &Path,
    roots: &[PathBuf],
    token: Token,
    deadline: ScanDeadline,
) -> Update {
    let mut diagnostics = None;
    let mut run = || -> Result<(Vec<Recorded>, String), String> {
        let mut staged = Catalog::load(catalog_path)?;
        let enumeration = crate::roots::enumerate(roots, || token.cancelled())?;
        let mut paths: std::collections::BTreeSet<_> = staged
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        paths.extend(enumeration.bundles.iter().cloned());
        if paths.len() > crate::MAX_MODULES {
            return Err("plugin catalog module capacity reached before scanning".into());
        }
        Package::inspect(app)?;
        let batch = scan_batch(
            &mut staged,
            enumeration.bundles,
            &token,
            &mut diagnostics,
            |path| Bundle::inspect(path).ok(),
            |path| discover(app, path, &token, deadline),
        )?;
        Ok((
            batch.results,
            format!(
                "vst3 scan {} · {} discovered · {} failed · {} unchanged failures skipped · {} linked entries skipped{} · hosting unavailable",
                if batch.stopped.is_some() {
                    "stopped early"
                } else {
                    "complete"
                },
                batch.successful,
                batch.failed,
                batch.skipped,
                enumeration.skipped_links,
                batch
                    .stopped
                    .map(|(remaining, error)| format!(
                        " · native cleanup unconfirmed · {error} · {remaining} not scanned"
                    ))
                    .unwrap_or_default(),
            ),
        ))
    };
    let (results, status) = match run() {
        Ok(value) => value,
        Err(status) => {
            return Update {
                catalog: None,
                diagnostics,
                status,
            };
        }
    };
    match token.begin_publication() {
        Ok(true) => {}
        Ok(false) => {
            return Update {
                catalog: None,
                diagnostics,
                status: "folder scan cancelled · catalog unchanged".into(),
            };
        }
        Err(status) => {
            return Update {
                catalog: None,
                diagnostics,
                status,
            };
        }
    }
    match crate::storage::update(catalog_path, |catalog| {
        for recorded in &results {
            recorded.apply(catalog)?;
        }
        Ok(())
    }) {
        Ok(publication) => Update {
            catalog: Some(publication.catalog),
            diagnostics,
            status: publication.warning.unwrap_or(status),
        },
        Err(error) => Update {
            catalog: None,
            diagnostics,
            status: format!("folder scan catalog update failed: {error}"),
        },
    }
}

/// One pending batch record, replayed under the final writer lock.
struct Recorded {
    path: PathBuf,
    result: Result<(Report, String), String>,
    bundle: Option<Bundle>,
}
impl Recorded {
    fn apply(&self, catalog: &mut Catalog) -> Result<(), String> {
        match &self.result {
            Ok(found) => catalog.record(self.path.clone(), Ok(found.clone())),
            Err(error) => {
                catalog.record_failure(self.path.clone(), error.clone(), self.bundle.clone())
            }
        }
    }
}

struct Batch {
    results: Vec<Recorded>,
    successful: usize,
    failed: usize,
    skipped: usize,
    /// Bundles left unscanned and the reason, when a chain did not retire.
    stopped: Option<(usize, String)>,
}

/// Sequential batch policy, separated from process launch for focused tests.
/// `identify` only inspects bundle metadata; `discover` may run native code.
fn scan_batch(
    staged: &mut Catalog,
    bundles: Vec<PathBuf>,
    token: &Token,
    diagnostics: &mut Option<ScanDiagnostics>,
    mut identify: impl FnMut(&Path) -> Option<Bundle>,
    mut discover: impl FnMut(&Path) -> DiscoveryOutcome,
) -> Result<Batch, String> {
    let mut batch = Batch {
        results: Vec::new(),
        successful: 0,
        failed: 0,
        skipped: 0,
        stopped: None,
    };
    let total = bundles.len();
    for (index, path) in bundles.into_iter().enumerate() {
        if token.cancelled() {
            return Err("folder scan cancelled · catalog unchanged".into());
        }
        if identify(&path).is_some_and(|current| staged.failed_unchanged(&path, &current)) {
            batch.skipped += 1;
            continue;
        }
        let outcome = discover(&path);
        let failure_identity = outcome.failure_identity();
        if outcome.result.is_err() || diagnostics.is_none() {
            *diagnostics = Some(outcome.diagnostics);
        }
        if token.cancelled() {
            return Err("folder scan cancelled · catalog unchanged".into());
        }
        let retired = outcome.chain_retired;
        let result = if retired {
            outcome
                .result
                .map(|(mut report, source)| {
                    report.stdout.clear();
                    report.stderr.clear();
                    (report, source)
                })
                .map_err(|error| error.chars().take(128).collect::<String>())
        } else {
            // Metadata from a chain that did not retire is never accepted.
            Err(outcome
                .result
                .err()
                .unwrap_or_else(|| "scanner retirement not established".into())
                .chars()
                .take(128)
                .collect())
        };
        let recorded = Recorded {
            path,
            result,
            bundle: failure_identity,
        };
        if recorded.result.is_ok() {
            batch.successful += 1;
        } else {
            batch.failed += 1;
        }
        let stop_reason = (!retired)
            .then(|| recorded.result.as_ref().err().cloned())
            .flatten();
        recorded.apply(staged)?;
        if serde_json::to_vec(staged)
            .map_err(|e| format!("catalog staging failed: {e}"))?
            .len()
            > crate::MAX_CATALOG_BYTES
        {
            return Err("folder scan catalog byte bound exceeded · catalog unchanged".into());
        }
        batch.results.push(recorded);
        if let Some(error) = stop_reason {
            // One native chain at a time: never launch another while this one
            // may still be alive. Results so far are still published.
            batch.stopped = Some((total - index - 1, error));
            break;
        }
    }
    Ok(batch)
}

pub fn available_app() -> Result<PathBuf, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("app executable unavailable: {error}"))?;
    // Source launch selects its verified tools explicitly. An installed app
    // always uses its own bundle and cannot be redirected by this setting.
    let launch_app = std::env::var_os("HEX_LAUNCH_DISCOVERY_APP").map(PathBuf::from);
    let app = discovery_app_path(&executable, launch_app.as_deref())?;
    Package::inspect(&app)?;
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_launch_can_select_absolute_tools_and_packaged_apps_keep_their_own() {
        let tools = Path::new("/checkout/artifacts/tools/hex.app");
        let raw = Path::new("/checkout/target/release/hex-desktop");
        assert_eq!(discovery_app_path(raw, Some(tools)).unwrap(), tools);
        assert!(discovery_app_path(raw, Some(Path::new("hex.app"))).is_err());
        assert!(discovery_app_path(raw, None).is_err());
        let packaged = Path::new("/Applications/hex.app/Contents/MacOS/hex");
        assert_eq!(
            discovery_app_path(packaged, Some(tools)).unwrap(),
            Path::new("/Applications/hex.app")
        );
    }
    #[test]
    fn scan_deadline_is_bounded_and_applies_to_scanner_and_outer_guard() {
        assert!(ScanDeadline::from_millis(0).is_err());
        assert!(ScanDeadline::from_millis(30_001).is_err());
        assert!(ScanDeadline::from_millis(1).is_ok());
        assert!(ScanDeadline::from_millis(30_000).is_ok());
        let default = ScanDeadline::default();
        assert_eq!(default.0, 10_000);
        assert_eq!(default.outer_limit(), Duration::from_secs(12));
        let deadline = ScanDeadline::from_millis(23_456).unwrap();
        let command = scanner_command(Path::new("/scanner"), Path::new("/test.vst3"), deadline);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--bundled", "--trust-user-code", "/test.vst3", "23456"]
        );
        assert_eq!(deadline.outer_limit(), Duration::from_millis(25_456));
        assert!(!deadline.expired(Duration::from_millis(25_456)));
        assert!(deadline.expired(Duration::from_millis(25_457)));
    }
    #[test]
    fn pre_spawn_refusal_reports_no_invented_native_cleanup() {
        let owner = Cancellation::default();
        let module = Path::new("/original-fixture.vst3");
        let outcome = discover(
            Path::new(""),
            module,
            &owner.token(),
            ScanDeadline::default(),
        );
        assert!(outcome.result.is_err());
        assert!(outcome.chain_retired, "no scanner was started");
        let observed = outcome.diagnostics;
        assert_eq!(observed.module, module);
        assert!(!observed.scanner_started);
        assert!(observed.scanner_exit.is_none());
        assert!(observed.native_cleanup.is_none());
        assert!(observed.report.is_empty() && observed.stderr.is_empty());
    }
    #[test]
    fn final_report_published_at_reap_is_drained_without_waiting_for_eof() {
        let (mut output, mut writer) = UnixStream::pair().unwrap();
        let (mut diagnostic, mut diagnostic_writer) = UnixStream::pair().unwrap();
        output.set_nonblocking(true).unwrap();
        diagnostic.set_nonblocking(true).unwrap();
        let payload = vec![b'x'; 8_000];
        let mut out = Vec::new();
        let mut err = Vec::new();
        // The old loop observed empty input here, then reaped and broke,
        // discarding the report published immediately before successful reap.
        assert!(!drain(&mut output, &mut out, OUTPUT_LIMIT).unwrap());
        let mut exit = None;
        let (received, error) = poll_and_drain(
            &mut exit,
            || {
                writer.write_all(&payload).unwrap();
                diagnostic_writer.write_all(b"final diagnostic").unwrap();
                Ok(Some(()))
            },
            [
                (&mut output, &mut out, OUTPUT_LIMIT),
                (&mut diagnostic, &mut err, ERROR_LIMIT),
            ],
        )
        .unwrap();
        assert!(received);
        assert!(error.is_none());
        assert_eq!(out, payload, "the report includes multiple read chunks");
        for _ in 0..4 {
            let (received, error) = poll_and_drain(
                &mut exit,
                || panic!("retired child must never be polled again"),
                [
                    (&mut output, &mut out, OUTPUT_LIMIT),
                    (&mut diagnostic, &mut err, ERROR_LIMIT),
                ],
            )
            .unwrap();
            assert!(error.is_none());
            if !received {
                // Writers deliberately remain open: termination is not EOF-based.
                assert_eq!(out, payload);
                assert_eq!(err, b"final diagnostic");
                return;
            }
        }
        panic!("bounded final report did not finish draining");
    }

    #[test]
    fn cancellation_wins_until_commit_and_never_claims_a_late_cancel() {
        let owner = Cancellation::default();
        let token = owner.token();
        assert!(owner.cancel());
        assert!(
            owner.cancel(),
            "repeated pending cancellation remains accepted"
        );
        assert!(!token.begin_publication().unwrap());
        assert!(!owner.cancel());
        let owner = Cancellation::default();
        let token = owner.token();
        assert!(token.begin_publication().unwrap());
        assert!(!owner.cancel());
    }

    fn identity(path: &Path, info: char) -> Bundle {
        Bundle {
            supplied_path: path.to_path_buf(),
            canonical_path: path.to_path_buf(),
            info_sha256: info.to_string().repeat(64),
        }
    }

    /// Fake discovery outcomes keyed by bundle name; counts native launches.
    fn fake_discovery(path: &Path, launches: &mut Vec<PathBuf>) -> DiscoveryOutcome {
        launches.push(path.to_path_buf());
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let (result, chain_retired) = match name {
            "Hanging" => (
                Err("scanner deadline exceeded; cleanup retained".into()),
                true,
            ),
            "Licensing" => (Err("native cleanup unconfirmed".into()), false),
            "Setup" => (Err("scanner start failed".into()), true),
            _ => (
                Ok((crate::tests::report(path.to_path_buf()), "e".repeat(40))),
                true,
            ),
        };
        DiscoveryOutcome {
            result,
            chain_retired,
            diagnostics: ScanDiagnostics {
                scanner_started: name != "Setup",
                ..ScanDiagnostics::default()
            },
            bundle: Some(identity(path, 'c')),
        }
    }

    #[test]
    fn unretired_bundle_is_recorded_without_discarding_results_and_failures_are_skipped() {
        let root = Path::new("/hex-batch-fixture");
        let [good, hanging, licensing, later] =
            ["Good", "Hanging", "Licensing", "Later"].map(|n| root.join(format!("{n}.vst3")));
        let owner = Cancellation::default();
        let token = owner.token();
        let mut staged = Catalog::default();
        let mut diagnostics = None;
        let mut launches = Vec::new();
        let all = vec![
            good.clone(),
            hanging.clone(),
            licensing.clone(),
            later.clone(),
        ];
        let first = scan_batch(
            &mut staged,
            all.clone(),
            &token,
            &mut diagnostics,
            |path| Some(identity(path, 'c')),
            |path| fake_discovery(path, &mut launches),
        )
        .expect("batch publishes despite an unretired chain");
        // No launch after the chain that did not retire.
        assert_eq!(launches, [good.clone(), hanging.clone(), licensing.clone()]);
        assert_eq!((first.successful, first.failed, first.skipped), (1, 2, 0));
        assert_eq!(
            first.stopped,
            Some((1, "native cleanup unconfirmed".to_owned()))
        );
        let recorded: Vec<_> = first
            .results
            .iter()
            .map(|r| (&r.path, r.result.is_ok()))
            .collect();
        assert_eq!(
            recorded,
            [(&good, true), (&hanging, false), (&licensing, false)]
        );
        let mut published = Catalog::default();
        for result in &first.results {
            result.apply(&mut published).unwrap();
        }
        let good_rows: Vec<_> = published
            .entries
            .iter()
            .filter(|e| e.last_good.is_some())
            .map(|e| &e.path)
            .collect();
        assert_eq!(good_rows, [&good], "the good bundle is published");
        assert!(published.failed_unchanged(&hanging, &identity(&hanging, 'c')));
        assert!(published.failed_unchanged(&licensing, &identity(&licensing, 'c')));

        // The next scan skips both unchanged failures and reaches the rest.
        launches.clear();
        let second = scan_batch(
            &mut published,
            all.clone(),
            &token,
            &mut diagnostics,
            |path| Some(identity(path, 'c')),
            |path| fake_discovery(path, &mut launches),
        )
        .unwrap();
        assert_eq!(launches, [good.clone(), later.clone()]);
        assert_eq!(
            (second.successful, second.failed, second.skipped),
            (2, 0, 2)
        );
        assert!(second.stopped.is_none());

        // A changed bundle identity is scanned again.
        launches.clear();
        scan_batch(
            &mut published,
            vec![hanging.clone()],
            &token,
            &mut diagnostics,
            |path| Some(identity(path, 'f')),
            |path| fake_discovery(path, &mut launches),
        )
        .unwrap();
        assert_eq!(launches, [hanging]);
    }

    #[test]
    fn failure_before_the_scanner_launch_is_not_remembered() {
        let owner = Cancellation::default();
        let token = owner.token();
        let setup = PathBuf::from("/hex-batch-fixture/Setup.vst3");
        let mut staged = Catalog::default();
        let mut launches = Vec::new();
        for _ in 0..2 {
            let batch = scan_batch(
                &mut staged,
                vec![setup.clone()],
                &token,
                &mut None,
                |path| Some(identity(path, 'c')),
                |path| fake_discovery(path, &mut launches),
            )
            .unwrap();
            assert_eq!((batch.failed, batch.skipped), (1, 0));
        }
        assert_eq!(launches.len(), 2, "tool failures are retried");
        assert!(staged.entries[0].failure.is_some());
        assert!(staged.entries[0].failed_bundle.is_none());
    }

    #[test]
    fn cancelled_batch_promotes_nothing() {
        let owner = Cancellation::default();
        let token = owner.token();
        assert!(owner.cancel());
        let mut launches = Vec::new();
        let result = scan_batch(
            &mut Catalog::default(),
            vec![PathBuf::from("/hex-batch-fixture/Good.vst3")],
            &token,
            &mut None,
            |_| None,
            |path| fake_discovery(path, &mut launches),
        );
        assert!(result.is_err());
        assert!(launches.is_empty());
    }
}
