//! Durable, bounded recovery reporting for undelivered export failures and
//! retained staging locations across shutdown, full/disconnected result
//! channels, and application exit (`#8093`).
//!
//! # Custody & platform limits
//! - Normal in-process export results continue to deliver directly through
//!   `Service::try_export_event` without disk writes.
//! - When an export terminates during shutdown or against a saturated or
//!   disconnected result channel (or sits unread in `export_events` when
//!   `Service` is dropped on application exit), [`ExportRecoveryJournal`]
//!   records the structured destination, retained staging/recovery paths, and
//!   original failure message both in memory and durably in a bounded atomic
//!   sidecar file (`export-recovery-notices.log`, at most 32 newest entries,
//!   512 KiB read cap).
//! - Paths are stored as raw hex-encoded OS bytes (Unix `OsStrExt::as_bytes`;
//!   UTF-8 on non-Unix platforms) so mixed-case names, Unicode directional
//!   controls, and non-UTF-8 Unix filenames round-trip truthfully without loss.
//! - Recording or clearing a recovery notice NEVER deletes, renames, or mutates
//!   the retained staging or destination file on disk; it only preserves the
//!   location and failure so the user can inspect or recover it after restart.
//! - **Platform / older-version limits**:
//!   - Older Hex builds that predate this `v1` notice file ignore it safely
//!     without touching staged WAV files.
//!   - On non-Unix platforms, non-UTF-8 `OsString` surrogates fall back to
//!     lossy UTF-8 encoding.
//!   - If the recovery directory itself is read-only or full during exit, the
//!     in-memory queue still retains the notice for callers querying
//!     `Service::take_export_recovery_notices()` without stalling shutdown.

use crate::service::ExportEvent;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_RECOVERY_NOTICES: usize = 32;
const MAX_JOURNAL_BYTES: usize = 512 * 1024;
const SCHEMA_TAG: &str = "v1";
const NOTICE_FILENAME: &str = "export-recovery-notices.log";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Classification of an undelivered export recovery notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportRecoveryKind {
    StagingRetained,
    IncompleteDestination,
    PublicationConflict,
    PublishedButNotDurable,
    PublishedNotificationFailed,
    ExportFailed,
}

impl ExportRecoveryKind {
    fn as_tag(self) -> &'static str {
        match self {
            Self::StagingRetained => "staging_retained",
            Self::IncompleteDestination => "incomplete_destination",
            Self::PublicationConflict => "publication_conflict",
            Self::PublishedButNotDurable => "published_not_durable",
            Self::PublishedNotificationFailed => "published_notification_failed",
            Self::ExportFailed => "export_failed",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "staging_retained" => Some(Self::StagingRetained),
            "incomplete_destination" => Some(Self::IncompleteDestination),
            "publication_conflict" => Some(Self::PublicationConflict),
            "published_not_durable" => Some(Self::PublishedButNotDurable),
            "published_notification_failed" => Some(Self::PublishedNotificationFailed),
            "export_failed" => Some(Self::ExportFailed),
            _ => None,
        }
    }
}

/// Structured durable record of an export whose result could not reach the live
/// UI consumer before shutdown/disconnect, preserving exact staging/recovery
/// paths and original failure text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRecoveryNotice {
    pub revision: u64,
    pub kind: ExportRecoveryKind,
    pub destination_path: PathBuf,
    pub staging_path: Option<PathBuf>,
    pub recovery_path: Option<PathBuf>,
    pub message: String,
}

impl ExportRecoveryNotice {
    /// Extract a structured recovery notice from an undelivered [`ExportEvent`].
    /// Returns `None` only for clean successful exports with no retained
    /// previous-file recovery path.
    pub fn from_export_event(event: &ExportEvent) -> Option<Self> {
        match &event.result {
            Ok(report) => report.recovery_path.as_ref().map(|recovery| Self {
                revision: event.revision,
                kind: ExportRecoveryKind::PublishedNotificationFailed,
                destination_path: report.path.clone(),
                staging_path: None,
                recovery_path: Some(recovery.clone()),
                message: format!(
                    "export published at {}; previous file retained at {}",
                    hex_storage::text::verbatim(report.path.display()),
                    hex_storage::text::verbatim(recovery.display())
                ),
            }),
            Err(failure) => {
                if let Some(source) = failure.source_error() {
                    let (kind, destination_path, staging_path, recovery_path) = match source {
                        hex_export::ExportError::StagingRetained { path, .. } => (
                            ExportRecoveryKind::StagingRetained,
                            event.path.clone(),
                            Some(path.clone()),
                            None,
                        ),
                        hex_export::ExportError::IncompleteDestination { path, .. } => (
                            ExportRecoveryKind::IncompleteDestination,
                            path.clone(),
                            Some(path.clone()),
                            None,
                        ),
                        hex_export::ExportError::PublicationConflict {
                            path,
                            recovery_path,
                        } => (
                            ExportRecoveryKind::PublicationConflict,
                            path.clone(),
                            None,
                            Some(recovery_path.clone()),
                        ),
                        hex_export::ExportError::PublishedButNotDurable {
                            path,
                            recovery_path,
                            ..
                        } => (
                            ExportRecoveryKind::PublishedButNotDurable,
                            path.clone(),
                            None,
                            recovery_path.clone(),
                        ),
                        _ => (
                            ExportRecoveryKind::ExportFailed,
                            event.path.clone(),
                            None,
                            None,
                        ),
                    };
                    Some(Self {
                        revision: event.revision,
                        kind,
                        destination_path,
                        staging_path,
                        recovery_path,
                        message: failure.message().to_owned(),
                    })
                } else if let Some(report) = failure.published_report() {
                    Some(Self {
                        revision: event.revision,
                        kind: ExportRecoveryKind::PublishedNotificationFailed,
                        destination_path: report.path.clone(),
                        staging_path: None,
                        recovery_path: report.recovery_path.clone(),
                        message: failure.message().to_owned(),
                    })
                } else {
                    Some(Self {
                        revision: event.revision,
                        kind: ExportRecoveryKind::ExportFailed,
                        destination_path: event.path.clone(),
                        staging_path: None,
                        recovery_path: None,
                        message: failure.message().to_owned(),
                    })
                }
            }
        }
    }

    fn encode_line(&self) -> String {
        let dest = encode_path_hex(&self.destination_path);
        let stage = self
            .staging_path
            .as_deref()
            .map(encode_path_hex)
            .unwrap_or_else(|| "-".to_owned());
        let recovery = self
            .recovery_path
            .as_deref()
            .map(encode_path_hex)
            .unwrap_or_else(|| "-".to_owned());
        let message = encode_hex(self.message.as_bytes());
        format!(
            "{SCHEMA_TAG}\t{}\t{}\t{dest}\t{stage}\t{recovery}\t{message}\n",
            self.revision,
            self.kind.as_tag()
        )
    }

    fn decode_line(line: &str) -> Result<Self, String> {
        let mut parts = line.split('\t');
        let Some(version) = parts.next() else {
            return Err("empty export recovery record".into());
        };
        if version != SCHEMA_TAG {
            return Err(format!("unsupported export recovery schema: {version}"));
        }
        let revision = parts
            .next()
            .ok_or("missing revision")?
            .parse::<u64>()
            .map_err(|_| "invalid revision")?;
        let kind_tag = parts.next().ok_or("missing recovery kind")?;
        let kind = ExportRecoveryKind::from_tag(kind_tag)
            .ok_or_else(|| format!("unknown recovery kind: {kind_tag}"))?;
        let dest_hex = parts.next().ok_or("missing destination path")?;
        let stage_hex = parts.next().ok_or("missing staging path")?;
        let recovery_hex = parts.next().ok_or("missing recovery path")?;
        let message_hex = parts.next().ok_or("missing failure message")?;
        if parts.next().is_some() {
            return Err("unexpected trailing export recovery field".into());
        }
        let destination_path = decode_path_hex(dest_hex)?;
        let staging_path = if stage_hex == "-" {
            None
        } else {
            Some(decode_path_hex(stage_hex)?)
        };
        let recovery_path = if recovery_hex == "-" {
            None
        } else {
            Some(decode_path_hex(recovery_hex)?)
        };
        let message_bytes = decode_hex(message_hex)?;
        let message =
            String::from_utf8(message_bytes).map_err(|_| "invalid utf-8 in recovery message")?;
        Ok(Self {
            revision,
            kind,
            destination_path,
            staging_path,
            recovery_path,
            message,
        })
    }
}

/// Bounded in-memory and optional durable store for undelivered export
/// recovery notices across shutdown and application exit.
pub struct ExportRecoveryJournal {
    path: Mutex<Option<PathBuf>>,
    in_memory: Mutex<Vec<ExportRecoveryNotice>>,
}

impl ExportRecoveryJournal {
    pub fn new(path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            path: Mutex::new(path),
            in_memory: Mutex::new(Vec::new()),
        })
    }

    /// Derive the default durable export-recovery notice path beside a session
    /// recovery slot (e.g. `<recovery_dir>/export-recovery-notices.log`).
    pub fn default_path_for_recovery_slot(slot: &Path) -> Option<PathBuf> {
        slot.parent().map(|parent| parent.join(NOTICE_FILENAME))
    }

    /// Configure or replace the durable file path used when recording future
    /// undelivered export events.
    pub fn set_path(&self, path: Option<PathBuf>) {
        let mut slot = self
            .path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = path;
    }

    /// Record an undelivered [`ExportEvent`] in memory and, when a durable path
    /// is configured, atomically append it to the bounded notice file.
    /// Never blocks on channels and never deletes/modifies any staged export file.
    pub fn record_event(&self, event: &ExportEvent) -> Option<ExportRecoveryNotice> {
        let notice = ExportRecoveryNotice::from_export_event(event)?;
        {
            let mut entries = self
                .in_memory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !entries.contains(&notice) {
                entries.push(notice.clone());
                if entries.len() > MAX_RECOVERY_NOTICES {
                    let excess = entries.len() - MAX_RECOVERY_NOTICES;
                    entries.drain(..excess);
                }
            }
        }
        let target_path = self
            .path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(path) = target_path {
            let _ = append_notice_durable(&path, &notice);
        }
        Some(notice)
    }

    /// Drain notices captured in memory by this journal instance.
    pub fn take_notices(&self) -> Vec<ExportRecoveryNotice> {
        let mut entries = self
            .in_memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *entries)
    }

    /// Load persisted export recovery notices from `path`. Returns an empty
    /// list when the file does not exist; refuses symbolic links and oversized
    /// or non-regular files.
    pub fn load_notices(path: &Path) -> Result<Vec<ExportRecoveryNotice>, String> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("could not inspect export recovery log: {error}")),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("export recovery log must be a regular file, not a symbolic link".into());
        }
        if metadata.len() > MAX_JOURNAL_BYTES as u64 {
            return Err("export recovery log exceeds the 512 kib safety bound".into());
        }
        let file = File::open(path).map_err(|error| error.to_string())?;
        let mut raw = Vec::new();
        file.take(MAX_JOURNAL_BYTES as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(|error| error.to_string())?;
        if raw.len() > MAX_JOURNAL_BYTES {
            return Err("export recovery log exceeds the 512 kib safety bound".into());
        }
        let text = String::from_utf8(raw)
            .map_err(|_| "export recovery log contains invalid utf-8".to_owned())?;
        let mut notices = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            notices.push(ExportRecoveryNotice::decode_line(line)?);
        }
        Ok(notices)
    }

    /// Explicitly dismiss one matching notice after user inspection without
    /// modifying or deleting any staged export file on disk.
    pub fn clear_notice(
        path: &Path,
        revision: u64,
        staging_path: Option<&Path>,
    ) -> Result<bool, String> {
        let mut notices = Self::load_notices(path)?;
        let before = notices.len();
        notices.retain(|item| {
            !(item.revision == revision && item.staging_path.as_deref() == staging_path)
        });
        if notices.len() == before {
            return Ok(false);
        }
        write_notices_atomic(path, &notices).map_err(|error| error.to_string())?;
        Ok(true)
    }
}

fn append_notice_durable(path: &Path, notice: &ExportRecoveryNotice) -> io::Result<()> {
    let mut existing = match ExportRecoveryJournal::load_notices(path) {
        Ok(entries) => entries,
        Err(_) => return Err(io::Error::other("refusing invalid export recovery log")),
    };
    if existing.contains(notice) {
        return Ok(());
    }
    existing.push(notice.clone());
    if existing.len() > MAX_RECOVERY_NOTICES {
        let excess = existing.len() - MAX_RECOVERY_NOTICES;
        existing.drain(..excess);
    }
    write_notices_atomic(path, &existing)
}

fn write_notices_atomic(path: &Path, notices: &[ExportRecoveryNotice]) -> io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && (meta.file_type().is_symlink() || !meta.is_file())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to overwrite non-regular or symlink export recovery log",
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut encoded = String::new();
    for notice in notices {
        encoded.push_str(&notice.encode_line());
    }
    let mut temp = None;
    for _ in 0..32 {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".hex-export-recovery-{}-{id}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                temp = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let (temp_path, mut file) = temp.ok_or_else(|| {
        io::Error::other("could not reserve temporary export recovery journal file")
    })?;
    let write_res = file
        .write_all(encoded.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_res {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    #[cfg(unix)]
    {
        let _ = File::open(parent).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

fn encode_path_hex(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        encode_hex(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        encode_hex(path.to_string_lossy().as_bytes())
    }
}

fn decode_path_hex(hex: &str) -> Result<PathBuf, String> {
    let bytes = decode_hex(hex)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        let text = String::from_utf8(bytes).map_err(|_| "invalid utf-8 in recovery path")?;
        Ok(PathBuf::from(OsString::from(text)))
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("odd-length hex string in export recovery record".into());
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let raw = text.as_bytes();
    for pair in raw.chunks_exact(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex digit in export recovery record".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export_error::ExportFailure;

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hex-export-recovery-test-{}-{id}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("test dir");
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn durable_journal_preserves_raw_unix_paths_bounds_entries_and_refuses_symlinks() {
        let dir = Dir::new();
        let journal_path = dir.0.join("export-recovery-notices.log");
        let journal = ExportRecoveryJournal::new(Some(journal_path.clone()));

        #[cfg(unix)]
        let stage = {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(OsString::from_vec(b"/MixedCase/Stage-\xff.wav".to_vec()))
        };
        #[cfg(not(unix))]
        let stage = PathBuf::from("/MixedCase/Stage.wav");

        let event = ExportEvent {
            revision: 7,
            path: PathBuf::from("/MixedCase/Target.WAV"),
            result: Err(ExportFailure::from(
                hex_export::ExportError::StagingRetained {
                    path: stage.clone(),
                    cause: Box::new(hex_export::ExportError::Cancelled),
                },
            )),
        };
        let recorded = journal
            .record_event(&event)
            .expect("staging retained yields notice");
        assert_eq!(recorded.kind, ExportRecoveryKind::StagingRetained);
        assert_eq!(recorded.staging_path.as_deref(), Some(stage.as_path()));
        assert!(recorded.message.contains("export cancelled"));

        // Reloading from disk recovers exact path bytes and failure message.
        let loaded = ExportRecoveryJournal::load_notices(&journal_path).expect("reload");
        assert_eq!(loaded, vec![recorded.clone()]);

        // Duplicate recording is idempotent.
        journal.record_event(&event);
        assert_eq!(
            ExportRecoveryJournal::load_notices(&journal_path)
                .expect("dedup")
                .len(),
            1
        );

        // Bounding keeps at most MAX_RECOVERY_NOTICES newest entries.
        for rev in 10..55 {
            let e = ExportEvent {
                revision: rev,
                path: PathBuf::from(format!("/tmp/out-{rev}.wav")),
                result: Err(ExportFailure::from(
                    hex_export::ExportError::StagingRetained {
                        path: PathBuf::from(format!("/tmp/.hex-export-{rev}.wav")),
                        cause: Box::new(hex_export::ExportError::Cancelled),
                    },
                )),
            };
            journal.record_event(&e);
        }
        let bounded = ExportRecoveryJournal::load_notices(&journal_path).expect("bounded");
        assert_eq!(bounded.len(), MAX_RECOVERY_NOTICES);
        assert_eq!(bounded.last().unwrap().revision, 54);

        // Explicit dismissal removes only the target notice without touching files.
        let fake_stage = dir.0.join(".hex-export-54.wav");
        fs::write(&fake_stage, b"staged audio").unwrap();
        assert!(
            ExportRecoveryJournal::clear_notice(
                &journal_path,
                54,
                Some(Path::new("/tmp/.hex-export-54.wav"))
            )
            .expect("clear")
        );
        assert_eq!(fs::read(&fake_stage).unwrap(), b"staged audio");

        // Symlink log path is refused.
        #[cfg(unix)]
        {
            let target = dir.0.join("victim.txt");
            fs::write(&target, b"keep me").unwrap();
            let link = dir.0.join("symlink.log");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(ExportRecoveryJournal::load_notices(&link).is_err());
            let symlink_journal = ExportRecoveryJournal::new(Some(link));
            symlink_journal.record_event(&event);
            assert_eq!(fs::read(&target).unwrap(), b"keep me");
        }
    }
}
