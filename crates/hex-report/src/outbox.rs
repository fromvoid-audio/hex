//! Atomic offline outbox for pending issue reports keyed by UUID `request_id`.

use crate::{
    model::{IntakeReceipt, ReportEnvelope},
    transport::ReportTransport,
};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_RETRY_ATTEMPTS: u32 = 10;
const OUTBOX_SCHEMA: u32 = 1;
const MAX_OUTBOX_FILE_BYTES: u64 = 6 * 1024 * 1024;

/// Persisted on-disk outbox item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxItem {
    pub schema: u32,
    pub envelope: ReportEnvelope,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub created_at_unix_secs: u64,
    pub last_attempt_unix_secs: Option<u64>,
}

/// Summary of one `flush_pending` pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlushReport {
    pub succeeded: Vec<IntakeReceipt>,
    pub failed: Vec<(String, String)>,
    pub skipped_exhausted: usize,
}

/// Default outbox directory (`~/Library/Application Support/hex/report-outbox` on macOS).
pub fn default_outbox_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "macos") {
        let home = env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "home directory is unavailable".to_owned())?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("hex")
            .join("report-outbox"))
    } else if cfg!(windows) {
        let base = env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .or_else(|| env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
            .or_else(|| env::var_os("HOME").filter(|v| !v.is_empty()))
            .ok_or_else(|| "home directory is unavailable".to_owned())?;
        Ok(PathBuf::from(base).join("hex").join("report-outbox"))
    } else {
        if let Some(xdg) = env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            return Ok(xdg.join("hex").join("report-outbox"));
        }
        let home = env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "home directory is unavailable".to_owned())?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("hex")
            .join("report-outbox"))
    }
}

/// Atomic directory-backed report outbox.
#[derive(Clone, Debug)]
pub struct ReportOutbox {
    root: PathBuf,
}

impl ReportOutbox {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Persist a report envelope atomically as `<request_id>.json`.
    pub fn enqueue(&self, envelope: &ReportEnvelope) -> io::Result<PathBuf> {
        validate_request_id(&envelope.request_id)?;
        ensure_private_dir(&self.root)?;
        let item = OutboxItem {
            schema: OUTBOX_SCHEMA,
            envelope: envelope.clone(),
            attempts: 0,
            last_error: None,
            created_at_unix_secs: now_unix_secs(),
            last_attempt_unix_secs: None,
        };
        let target = self.item_path(&envelope.request_id);
        atomic_write_json(&target, &item)?;
        Ok(target)
    }

    /// Read all valid pending outbox items sorted oldest-first.
    pub fn list_pending(&self) -> io::Result<Vec<OutboxItem>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut items = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink()
                || !meta.is_file()
                || meta.len() > MAX_OUTBOX_FILE_BYTES
            {
                continue;
            }
            if let Ok(bytes) = fs::read(&path)
                && let Ok(item) = serde_json::from_slice::<OutboxItem>(&bytes)
                && item.schema == OUTBOX_SCHEMA
                && validate_request_id(&item.envelope.request_id).is_ok()
            {
                items.push(item);
            }
        }
        items.sort_by_key(|item| (item.created_at_unix_secs, item.envelope.request_id.clone()));
        Ok(items)
    }

    /// Remove an outbox item by `request_id` if present.
    pub fn remove(&self, request_id: &str) -> io::Result<()> {
        validate_request_id(request_id)?;
        let path = self.item_path(request_id);
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Record a failed delivery attempt on an existing item.
    pub fn record_failure(&self, item: &OutboxItem, error: &str) -> io::Result<()> {
        validate_request_id(&item.envelope.request_id)?;
        let mut updated = item.clone();
        updated.attempts = updated.attempts.saturating_add(1);
        updated.last_error = Some(error.chars().take(512).collect());
        updated.last_attempt_unix_secs = Some(now_unix_secs());
        let target = self.item_path(&updated.envelope.request_id);
        atomic_write_json(&target, &updated)
    }

    /// Flush all eligible pending items idempotently via `transport`.
    /// Removes succeeded items and preserves failed items with updated retry counts.
    pub fn flush_pending(&self, transport: &dyn ReportTransport) -> io::Result<FlushReport> {
        let pending = self.list_pending()?;
        let mut report = FlushReport::default();
        for item in pending {
            if item.attempts >= MAX_RETRY_ATTEMPTS {
                report.skipped_exhausted = report.skipped_exhausted.saturating_add(1);
                continue;
            }
            match transport.submit(&item.envelope) {
                Ok(receipt) => {
                    self.remove(&item.envelope.request_id)?;
                    report.succeeded.push(receipt);
                }
                Err(error) => {
                    let message = error.to_string();
                    let _ = self.record_failure(&item, &message);
                    report
                        .failed
                        .push((item.envelope.request_id.clone(), message));
                }
            }
        }
        Ok(report)
    }

    fn item_path(&self, request_id: &str) -> PathBuf {
        self.root.join(format!("{request_id}.json"))
    }
}

fn validate_request_id(request_id: &str) -> io::Result<()> {
    if request_id.is_empty()
        || request_id.len() > 64
        || !request_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid report request_id",
        ));
    }
    Ok(())
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn atomic_write_json<T: Serialize>(target: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid outbox target"))?;
    ensure_private_dir(parent)?;
    let tmp_name = format!(
        ".{}.tmp.{}",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("report"),
        std::process::id()
    );
    let tmp = parent.join(tmp_name);
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, target)?;
    Ok(())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{ReportKind, RuntimeSnapshot},
        transport::TransportError,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temp_outbox() -> ReportOutbox {
        let dir = env::temp_dir().join(format!(
            "hex-report-outbox-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        ReportOutbox::new(dir)
    }

    struct StubTransport {
        fail_id: Option<String>,
    }

    impl ReportTransport for StubTransport {
        fn submit(&self, envelope: &ReportEnvelope) -> Result<IntakeReceipt, TransportError> {
            if self.fail_id.as_deref() == Some(envelope.request_id.as_str()) {
                return Err(TransportError::Network("offline".into()));
            }
            Ok(IntakeReceipt {
                report_id: 7,
                project_id: envelope.project_id,
                request_id: envelope.request_id.clone(),
                status: "new".into(),
                deduplicated: false,
            })
        }
    }

    #[test]
    fn enqueues_atomically_flushes_succeeded_and_tracks_failed_retries() {
        let outbox = temp_outbox();
        let env1 = ReportEnvelope::with_request_id(
            "11111111-1111-4111-8111-111111111111".into(),
            2,
            ReportKind::Bug,
            "first".into(),
            RuntimeSnapshot::default(),
            None,
        );
        let env2 = ReportEnvelope::with_request_id(
            "22222222-2222-4222-8222-222222222222".into(),
            2,
            ReportKind::Feature,
            "second".into(),
            RuntimeSnapshot::default(),
            None,
        );
        outbox.enqueue(&env1).unwrap();
        outbox.enqueue(&env2).unwrap();
        assert_eq!(outbox.list_pending().unwrap().len(), 2);

        let transport = StubTransport {
            fail_id: Some(env2.request_id.clone()),
        };
        let first_pass = outbox.flush_pending(&transport).unwrap();
        assert_eq!(first_pass.succeeded.len(), 1);
        assert_eq!(first_pass.succeeded[0].request_id, env1.request_id);
        assert_eq!(first_pass.failed.len(), 1);
        assert_eq!(first_pass.failed[0].0, env2.request_id);

        let remaining = outbox.list_pending().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].envelope.request_id, env2.request_id);
        assert_eq!(remaining[0].attempts, 1);
        assert!(
            remaining[0]
                .last_error
                .as_deref()
                .unwrap()
                .contains("offline")
        );

        // Second pass with healthy transport clears remaining item
        let healthy = StubTransport { fail_id: None };
        let second_pass = outbox.flush_pending(&healthy).unwrap();
        assert_eq!(second_pass.succeeded.len(), 1);
        assert!(outbox.list_pending().unwrap().is_empty());

        fs::remove_dir_all(outbox.root()).unwrap();
    }

    #[test]
    fn rejects_path_traversal_request_ids() {
        let outbox = temp_outbox();
        let bad = ReportEnvelope::with_request_id(
            "../escape".into(),
            2,
            ReportKind::Bug,
            "bad".into(),
            RuntimeSnapshot::default(),
            None,
        );
        assert!(outbox.enqueue(&bad).is_err());
    }
}
