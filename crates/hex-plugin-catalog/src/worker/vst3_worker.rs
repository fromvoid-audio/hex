//! Isolated VST3 bundle discovery, folder batch scanning, and catalog publication.
use super::*;

pub(super) fn scanner_command(scanner: &Path, module: &Path, deadline: ScanDeadline) -> Command {
    let mut command = Command::new(scanner);
    command
        .args(["--bundled", "--trust-user-code"])
        .arg(module)
        .arg(deadline.millis.to_string());
    if deadline.wait_for_plugin {
        command.arg(WAIT_FOR_PLUGIN_ARG);
    }
    command
}

pub(super) fn discovery_app_path(
    executable: &Path,
    launch_app: Option<&Path>,
) -> Result<PathBuf, String> {
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

pub(super) fn drain(
    input: &mut UnixStream,
    output: &mut Vec<u8>,
    limit: usize,
) -> Result<bool, String> {
    hex_plugin_scan::drain_bounded(input, output, limit, "scanner output bound exceeded", |e| {
        format!("scanner output unavailable: {e}")
    })
    .map(|outcome| outcome.any_read)
}

/// Discard buffered bytes from an already-overflowed non-blocking stream so a
/// flooded child cannot wedge on a full OS pipe before observing cancellation.
pub(super) fn discard_pending(input: &mut UnixStream) -> Result<bool, String> {
    let mut scratch = [0_u8; 16 * 1024];
    let mut any_read = false;
    for _ in 0..16 {
        match std::io::Read::read(input, &mut scratch) {
            Ok(0) => return Ok(any_read),
            Ok(_) => any_read = true,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(any_read),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("scanner output unavailable: {e}")),
        }
    }
    Ok(any_read)
}

// Observe direct-child retirement before deciding that currently empty channels
// are final. A child can publish its last report between two parent operations.
pub(super) fn poll_and_drain<T>(
    exit: &mut Option<T>,
    observe: impl FnOnce() -> std::io::Result<Option<T>>,
    streams: [(&mut UnixStream, &mut Vec<u8>, usize); 2],
    overflowed: &mut [bool; 2],
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
    for ((stream, bytes, maximum), is_overflowed) in streams.into_iter().zip(overflowed.iter_mut())
    {
        if *is_overflowed {
            match discard_pending(stream) {
                Ok(value) => received |= value,
                Err(value) => {
                    error.get_or_insert(value);
                }
            }
            continue;
        }
        match drain(stream, bytes, maximum) {
            Ok(value) => received |= value,
            Err(value) => {
                *is_overflowed = true;
                error.get_or_insert(value);
                if let Ok(drained) = discard_pending(stream) {
                    received |= drained;
                }
            }
        }
    }
    Ok((received, error))
}

pub(super) struct DiscoveryOutcome {
    pub(super) result: Result<(Report, String), String>,
    pub(super) chain_retired: bool,
    pub(super) diagnostics: ScanDiagnostics,
    /// Bundle and scan tools inspected before any code ran.
    pub(super) failed_scan: Option<FailedScan>,
}
impl DiscoveryOutcome {
    /// Identity to remember with a failure. Only a failure after the scanner
    /// started this bundle is attributed to it; tool/setup failures before the
    /// launch stay unremembered so the next scan retries them.
    pub(super) fn failure_identity(&self) -> Option<FailedScan> {
        self.failed_scan
            .clone()
            .filter(|_| self.diagnostics.scanner_started)
    }
}
pub(super) fn discover(
    app: &Path,
    path: &Path,
    preinspected: Option<&Package>,
    token: &Token,
    deadline: ScanDeadline,
) -> DiscoveryOutcome {
    token.set_current_module(Some(path));
    let started = Instant::now();
    let mut chain_retired = true; // No child has been started yet.
    let mut failed_scan = None;
    let mut diagnostics = ScanDiagnostics {
        module: path.to_path_buf(),
        ..ScanDiagnostics::default()
    };
    let result = discover_inner(
        app,
        path,
        preinspected,
        token,
        deadline,
        &mut chain_retired,
        &mut diagnostics,
        &mut failed_scan,
    );
    diagnostics.elapsed = started.elapsed();
    token.set_current_module(None);
    DiscoveryOutcome {
        result,
        chain_retired,
        diagnostics,
        failed_scan,
    }
}
/// Bounded grace period after requesting outer scanner cancellation (stdin byte)
/// before escalating to `SIGKILL` (`child.kill()`) on the unreaped direct `Child`.
const OUTER_CANCEL_GRACE: Duration = Duration::from_secs(2);

pub(super) struct WaitChildOutcome {
    pub(super) exit: Option<std::process::ExitStatus>,
    pub(super) out: Vec<u8>,
    pub(super) err: Vec<u8>,
    pub(super) cause: Option<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) killed: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn wait_scanner_child(
    child: &mut std::process::Child,
    output: &mut UnixStream,
    diagnostic: &mut UnixStream,
    cancel: &mut UnixStream,
    token: &Token,
    deadline: ScanDeadline,
    cancel_grace: Duration,
    diagnostics: &mut ScanDiagnostics,
) -> Result<WaitChildOutcome, String> {
    let started = Instant::now();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut overflowed = [false; 2];
    let mut cause = None;
    let mut cancel_sent = false;
    let mut cancel_requested_at: Option<Instant> = None;
    let mut kill_sent_at: Option<Instant> = None;
    let mut exit = None;
    // Direct Child handle operations only (try_wait / kill); never numeric PID
    // or process-group signals. After cancellation or deadline expiry, give the
    // outer scanner a bounded grace window to retire cleanly before escalating
    // to SIGKILL on the unreaped direct Child.
    loop {
        let polled = poll_and_drain(
            &mut exit,
            || child.try_wait(),
            [
                (output, &mut out, OUTPUT_LIMIT),
                (diagnostic, &mut err, ERROR_LIMIT),
            ],
            &mut overflowed,
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
            cancel_requested_at.get_or_insert_with(Instant::now);
            match cancel.write(&[1]) {
                Ok(1) => cancel_sent = true,
                Ok(_) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => cancel_sent = true,
            }
        }
        if exit.is_none()
            && kill_sent_at.is_none()
            && cancel_requested_at.is_some_and(|at| at.elapsed() >= cancel_grace)
        {
            let _ = child.kill();
            kill_sent_at = Some(Instant::now());
            cause.get_or_insert_with(|| "scanner hung after cancellation; terminated".into());
        }
        if exit.is_some() && !received {
            break;
        }
        if kill_sent_at.is_some_and(|at| at.elapsed() >= cancel_grace) {
            // Final bounded reap attempt in case the child exited right at the boundary.
            if exit.is_none()
                && let Ok(status) = child.try_wait()
            {
                exit = status;
                diagnostics.scanner_exit = exit;
            }
            break;
        }
        if !received {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(WaitChildOutcome {
        exit,
        out,
        err,
        cause,
        killed: kill_sent_at.is_some(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn discover_inner(
    app: &Path,
    path: &Path,
    preinspected: Option<&Package>,
    token: &Token,
    deadline: ScanDeadline,
    chain_retired: &mut bool,
    diagnostics: &mut ScanDiagnostics,
    failed_scan: &mut Option<FailedScan>,
) -> Result<(Report, String), String> {
    if token.cancelled() {
        return Err("scan cancelled".into());
    }
    let owned_package;
    let package = match preinspected {
        Some(existing) => existing,
        None => {
            owned_package = Package::inspect(app)?;
            &owned_package
        }
    };
    let initial = Bundle::inspect(path)?;
    *failed_scan = Some(FailedScan {
        bundle: initial.clone(),
        tools_sha256: package.scan_sha256.clone(),
    });
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
    let WaitChildOutcome {
        exit,
        out,
        err,
        mut cause,
        killed: _,
    } = wait_scanner_child(
        &mut child,
        &mut output,
        &mut diagnostic,
        &mut cancel,
        token,
        deadline,
        OUTER_CANCEL_GRACE,
        diagnostics,
    )?;
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
    let verified_executable = report
        .executable
        .as_ref()
        .ok_or("scanner executable identity missing")?;
    initial.unchanged()?;
    initial.executable_unchanged(verified_executable)?;
    crate::fingerprint::record_vst3_verified(&initial, verified_executable);
    // `Package::inspect` already verifies the sealed manifest, scanner binary,
    // native receipt, and helper build identity in one pass.
    let fresh = Package::inspect(app)?;
    if fresh.source != package.source
        || fresh.scan_sha256 != package.scan_sha256
        || fresh.build.receipt_sha256 != package.build.receipt_sha256
        || fresh.build.helper_sha256 != package.build.helper_sha256
    {
        return Err("discovery package changed during scan".into());
    }
    Ok((report, package.source.clone()))
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
            if catalog
                .entries
                .len()
                .saturating_add(catalog.clap_entries.len())
                >= crate::MAX_MODULES
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
    let outcome = discover(app, &path, None, &token, deadline);
    let failure_identity = outcome.failure_identity();
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
        Ok(_) => "vst3 discovered".into(),
        Err(error) => error.clone(),
    };
    match crate::storage::update(catalog_path, |catalog| match result {
        Ok(found) => catalog.record(path, Ok(found)),
        Err(error) => catalog.record_failure(path, error, failure_identity),
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
    match crate::format::backend(crate::PluginFormat::Vst3).standard_roots() {
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
    scan_folders_with_mode(
        catalog_path,
        app,
        roots,
        token,
        deadline,
        ScanMode::Incremental,
    )
}

/// Run the requested folder policy with a common per-bundle deadline.
pub fn scan_folders_with_mode(
    catalog_path: &Path,
    app: &Path,
    roots: &[PathBuf],
    token: Token,
    deadline: ScanDeadline,
    mode: ScanMode,
) -> Update {
    let mut diagnostics = None;
    let mut run = || -> Result<(Vec<Recorded>, String), String> {
        let mut staged = Catalog::load(catalog_path)?;
        let enumeration = crate::format::backend(crate::PluginFormat::Vst3)
            .enumerate(roots, &mut || token.cancelled())?;
        let mut paths: std::collections::BTreeSet<_> = staged
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        paths.extend(enumeration.bundles.iter().cloned());
        if paths.len().saturating_add(staged.clap_entries.len()) > crate::MAX_MODULES {
            return Err("plugin catalog module capacity reached before scanning".into());
        }
        let package = Package::inspect(app)?;
        let tools = ScanTools {
            scan: &package.scan_sha256,
            receipt: &package.build.receipt_sha256,
            helper: &package.build.helper_sha256,
        };
        let batch = scan_batch(
            &mut staged,
            enumeration.bundles,
            &token,
            &mut diagnostics,
            |catalog, path| cached_result(mode, catalog, path, tools),
            |path| discover(app, path, Some(&package), &token, deadline),
        )?;
        Ok((
            batch.results,
            format!(
                "vst3 scan {} · {} discovered · {} failed · {} unchanged failures skipped · {} unchanged plug-ins reused · {} linked entries skipped{}",
                if batch.stopped.is_some() {
                    "stopped early"
                } else {
                    "complete"
                },
                batch.successful,
                batch.failed,
                batch.skipped,
                batch.reused,
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
    if results.is_empty() {
        // A cache hit still returns the latest catalog so the browser can refresh
        // availability, including missing bundles, without writing the cache.
        return match Catalog::load(catalog_path) {
            Ok(catalog) => Update {
                catalog: Some(catalog),
                diagnostics,
                status,
            },
            Err(error) => Update {
                catalog: None,
                diagnostics,
                status: format!("folder scan catalog reload failed: {error}"),
            },
        };
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
pub(super) struct Recorded {
    pub(super) path: PathBuf,
    pub(super) result: Result<(Report, String), String>,
    pub(super) failed_scan: Option<FailedScan>,
}
impl Recorded {
    pub(super) fn apply(&self, catalog: &mut Catalog) -> Result<(), String> {
        match &self.result {
            Ok(found) => catalog.record(self.path.clone(), Ok(found.clone())),
            Err(error) => {
                catalog.record_failure(self.path.clone(), error.clone(), self.failed_scan.clone())
            }
        }
    }

    pub(super) fn apply_incremental(
        &self,
        catalog: &mut Catalog,
    ) -> Result<(usize, usize), String> {
        match &self.result {
            Ok(found) => {
                catalog.record_outcome_incremental(self.path.clone(), Ok(found.clone()), None)
            }
            Err(error) => catalog.record_outcome_incremental(
                self.path.clone(),
                Err(error.clone()),
                self.failed_scan.clone(),
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CachedResult {
    Success,
    Failure,
}

#[derive(Clone, Copy)]
pub(super) struct ScanTools<'a> {
    pub(super) scan: &'a str,
    pub(super) receipt: &'a str,
    pub(super) helper: &'a str,
}

pub(super) fn cached_result(
    mode: ScanMode,
    catalog: &Catalog,
    path: &Path,
    tools: ScanTools<'_>,
) -> Option<CachedResult> {
    if mode == ScanMode::Full {
        return None;
    }
    let entry = catalog.entries.iter().find(|entry| entry.path == path)?;
    if entry.failure.is_some() {
        let current = Bundle::inspect(path).ok()?;
        return catalog
            .failed_unchanged(path, &current, tools.scan)
            .then_some(CachedResult::Failure);
    }
    let report = entry.last_good.as_ref()?;
    (report.receipt_sha256 == tools.receipt
        && report.helper_sha256 == tools.helper
        && report
            .discovery
            .as_ref()
            .is_some_and(|found| found.cacheable())
        && report.executable.as_ref().is_some_and(|executable| {
            crate::fingerprint::verify_vst3_unchanged(&report.bundle, executable).is_ok()
        }))
    .then_some(CachedResult::Success)
}

pub(super) struct Batch {
    pub(super) results: Vec<Recorded>,
    pub(super) successful: usize,
    pub(super) failed: usize,
    pub(super) skipped: usize,
    pub(super) reused: usize,
    /// Bundles left unscanned and the reason, when a chain did not retire.
    pub(super) stopped: Option<(usize, String)>,
}

/// Sequential batch policy, separated from process launch for focused tests.
/// `reuse` can inspect the filesystem but only `discover` runs native code.
pub(super) fn scan_batch(
    staged: &mut Catalog,
    bundles: Vec<PathBuf>,
    token: &Token,
    diagnostics: &mut Option<ScanDiagnostics>,
    mut reuse: impl FnMut(&Catalog, &Path) -> Option<CachedResult>,
    mut discover: impl FnMut(&Path) -> DiscoveryOutcome,
) -> Result<Batch, String> {
    let mut batch = Batch {
        results: Vec::new(),
        successful: 0,
        failed: 0,
        skipped: 0,
        reused: 0,
        stopped: None,
    };
    let total = bundles.len();
    let mut estimated_bytes = serde_json::to_vec(staged)
        .map_err(|e| format!("catalog staging failed: {e}"))?
        .len();
    for (index, path) in bundles.into_iter().enumerate() {
        if token.cancelled() {
            return Err("folder scan cancelled · catalog unchanged".into());
        }
        if let Some(cached) = reuse(staged, &path) {
            match cached {
                CachedResult::Success => batch.reused += 1,
                CachedResult::Failure => batch.skipped += 1,
            }
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
                .map_err(|error| crate::sanitize_failure(&error))
        } else {
            // Metadata from a chain that did not retire is never accepted.
            Err(crate::sanitize_failure(
                &outcome
                    .result
                    .err()
                    .unwrap_or_else(|| "scanner retirement not established".into()),
            ))
        };
        let recorded = Recorded {
            path,
            result,
            failed_scan: failure_identity,
        };
        if recorded.result.is_ok() {
            batch.successful += 1;
        } else {
            batch.failed += 1;
        }
        let stop_reason = (!retired)
            .then(|| recorded.result.as_ref().err().cloned())
            .flatten();
        let (prev_bytes, new_bytes) = recorded.apply_incremental(staged)?;
        estimated_bytes = estimated_bytes
            .saturating_sub(prev_bytes)
            .saturating_add(new_bytes);
        if estimated_bytes > crate::MAX_CATALOG_BYTES {
            let exact = serde_json::to_vec(staged)
                .map_err(|e| format!("catalog staging failed: {e}"))?
                .len();
            estimated_bytes = exact;
            if exact > crate::MAX_CATALOG_BYTES {
                return Err("folder scan catalog byte bound exceeded · catalog unchanged".into());
            }
        }
        batch.results.push(recorded);
        if let Some(error) = stop_reason {
            // One native chain at a time: never launch another while this one
            // may still be alive. Results so far are still published.
            batch.stopped = Some((total - index - 1, error));
            break;
        }
    }
    staged.validate()?;
    if serde_json::to_vec(staged)
        .map_err(|e| format!("catalog staging failed: {e}"))?
        .len()
        > crate::MAX_CATALOG_BYTES
    {
        return Err("folder scan catalog byte bound exceeded · catalog unchanged".into());
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
