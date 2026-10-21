//! Background control work. The standalone scanner alone owns native children.
use crate::{Catalog, validate_report};
use hex_plugin_scan::macos::{identity::Bundle, package::Package, report::Report};
use std::{
    io::{Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const ERROR_LIMIT: usize = 64 * 1024;
const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const FINISHED: u8 = 3;

/// UI/control owner. Dropping a view requests cancellation without waiting.
pub struct Cancellation(Arc<AtomicU8>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(Arc::new(AtomicU8::new(RUNNING)))
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
                .compare_exchange(RUNNING, CANCELLED, Ordering::AcqRel, Ordering::Acquire),
            Ok(_) | Err(CANCELLED)
        )
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}
pub struct Token(Arc<AtomicU8>);
impl Token {
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == CANCELLED
    }
    fn begin_publication(&self) -> Result<bool, String> {
        match self
            .0
            .compare_exchange(RUNNING, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(true),
            Err(CANCELLED) => {
                self.0.store(COMMITTING, Ordering::Release);
                Ok(false)
            }
            Err(_) => Err("scan publication state invalid".into()),
        }
    }
}
impl Drop for Token {
    fn drop(&mut self) {
        self.0.store(FINISHED, Ordering::Release);
    }
}

pub struct Update {
    pub catalog: Option<Catalog>,
    pub status: String,
}

/// Capture only on the worker; raw executables do not receive a checkout fallback.
pub fn current_app() -> Result<PathBuf, String> {
    let executable =
        std::env::current_exe().map_err(|e| format!("app executable unavailable: {e}"))?;
    let contents = executable
        .parent()
        .and_then(Path::parent)
        .ok_or("app contents unavailable")?;
    if contents.file_name().and_then(|v| v.to_str()) != Some("Contents") {
        return Err("packaged vst3 tools unavailable in this build".into());
    }
    Ok(contents
        .parent()
        .ok_or("app bundle unavailable")?
        .to_path_buf())
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

fn discover(app: &Path, path: &Path, token: &Token) -> Result<(Report, String), String> {
    if token.cancelled() {
        return Err("scan cancelled".into());
    }
    let package = Package::inspect(app)?;
    let initial = Bundle::inspect(path)?;
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
    let mut child = Command::new(&package.scanner)
        .args(["--bundled", "--trust-user-code"])
        .arg(path)
        .arg("10000")
        .stdin(Stdio::from(OwnedFd::from(child_cancel)))
        .stdout(Stdio::from(OwnedFd::from(child_output)))
        .stderr(Stdio::from(OwnedFd::from(child_diagnostic)))
        .spawn()
        .map_err(|e| format!("scanner start failed: {e}"))?;
    let started = Instant::now();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut cause = None;
    let mut cancel_sent = false;
    let mut exit = None;
    // No numeric PID/group operation other than this direct Child's try_wait.
    // A retained scanner may exceed its deadline while it retires a native child.
    loop {
        let mut received = false;
        for (stream, bytes, maximum) in [
            (&mut output, &mut out, OUTPUT_LIMIT),
            (&mut diagnostic, &mut err, ERROR_LIMIT),
        ] {
            match drain(stream, bytes, maximum) {
                Ok(value) => received |= value,
                Err(error) => {
                    cause.get_or_insert(error);
                }
            }
        }
        if (token.cancelled() || cause.is_some() || started.elapsed() > Duration::from_secs(12))
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
        if exit.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => exit = Some(status),
                Ok(None) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    return Err(format!(
                        "scanner custody lost; cleanup unconfirmed; no further process operations: {e}"
                    ));
                }
            }
        }
        if exit.is_some() && !received {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if token.cancelled() {
        cause.get_or_insert_with(|| "scan cancelled".into());
    }
    if let Some(error) = cause {
        return Err(error);
    }
    if !exit.is_some_and(|s| s.success()) {
        let detail = serde_json::from_slice::<Report>(&out)
            .ok()
            .and_then(|r| r.error)
            .unwrap_or_else(|| String::from_utf8_lossy(&err).into_owned());
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
    let report: Report = serde_json::from_slice(&out)
        .map_err(|_| "scanner returned malformed or incomplete metadata")?;
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
    if !crate::path_valid(&path) || path.extension().and_then(|s| s.to_str()) != Some("vst3") {
        return Update {
            catalog: None,
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
                status: "plugin catalog module capacity reached".into(),
            };
        }
        Ok(_) => {}
        Err(error) => {
            return Update {
                catalog: None,
                status: error,
            };
        }
    }
    let result = discover(app, &path, &token);
    // The atomic transition is the cancellation/publication linearization point.
    let result = match token.begin_publication() {
        Ok(true) => result,
        Ok(false) => Err("scan cancelled".into()),
        Err(status) => {
            return Update {
                catalog: None,
                status,
            };
        }
    };
    let status = match &result {
        Ok(_) => "vst3 discovered · hosting unavailable".into(),
        Err(error) => error.clone(),
    };
    match crate::storage::update(catalog_path, |catalog| catalog.record(path, result)) {
        Ok(publication) => Update {
            catalog: Some(publication.catalog),
            status: publication.warning.unwrap_or(status),
        },
        Err(error) => Update {
            catalog: None,
            status: format!("{status} · catalog update failed: {error}"),
        },
    }
}

pub fn available_app() -> Result<PathBuf, String> {
    let app = current_app()?;
    Package::inspect(&app)?;
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
