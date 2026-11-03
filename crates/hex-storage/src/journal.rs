//! One complete checkpoint per slot; no backup history or implicit recovery.
use crate::{FileToken, JournalChange, StorageError, encode, replacement};
mod restart;
mod slot_lock;
mod untitled_slot;
use hex_project::Project;
pub use restart::{JournalRestartOutcome, JournalRestartToken, restart_journal};
use slot_lock::{SlotLock, lock_slot};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
pub use untitled_slot::{MAX_UNTITLED_SLOTS, UntitledSlotClaim, untitled_slot_path};

const MAGIC: &[u8; 8] = b"HEXJRNL\0";
const LOCK_MAGIC: &[u8; 8] = b"HEXJLOC\0";
pub(crate) const JOURNAL_SUFFIX: &str = ".hex-journal";
/// Leaves room for the slot lock's `.lock` suffix within a 255-byte component.
pub(crate) const MAX_JOURNAL_NAME_BYTES: usize = 255 - ".lock".len();

/// Journal beside a saved project. Untitled uses a caller-supplied application slot.
pub fn journal_path(target: &Path) -> Result<PathBuf, StorageError> {
    let name = basename(target)?;
    let name = format!(".{name}{JOURNAL_SUFFIX}");
    if name.len() > MAX_JOURNAL_NAME_BYTES {
        return Err(StorageError::Invalid(
            "journal filename exceeds component limit",
        ));
    }
    Ok(parent(target).join(name))
}

/// Read a validated checkpoint without altering the journal or saved project.
pub fn load_journal(path: &Path) -> Result<Project, StorageError> {
    let bytes = read(path)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no recovery journal"))?;
    decode(path, &bytes)
}

/// Exact-byte ownership of one journal slot. A competing writer causes a conflict.
/// Cooperating journal writers serialize publication through an OS file lock.
/// Noncooperating filesystem writers can still race the final check.
pub struct Journal {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

impl Journal {
    /// Claim an empty slot without adopting a checkpoint published by another writer.
    pub fn open_empty(path: PathBuf) -> Result<Self, StorageError> {
        if read(&path)?.is_some() {
            return Err(StorageError::JournalConflict { path });
        }
        Ok(Self { path, bytes: None })
    }

    /// Adopt only a recognized, valid journal. Unknown files are preserved.
    pub fn open(path: PathBuf) -> Result<Self, StorageError> {
        let bytes = read(&path)?;
        if let Some(bytes) = &bytes {
            let _ = decode(&path, bytes)?;
        }
        Ok(Self { path, bytes })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Flush/sync staging before atomic publication; keep the old checkpoint on
    /// prepublication failure. Adopt the published bytes even if directory sync fails.
    pub fn checkpoint(&mut self, project: &Project) -> Result<(), StorageError> {
        self.checkpoint_with_hooks(project, false, || Ok(()), replacement::sync_parent)
    }

    /// Last serialized untitled checkpoint wins; only recognized journals can
    /// be superseded, and no previous generation is retained.
    pub fn checkpoint_latest(&mut self, project: &Project) -> Result<(), StorageError> {
        self.checkpoint_with_hooks(project, true, || Ok(()), replacement::sync_parent)
    }

    fn checkpoint_with_hooks(
        &mut self,
        project: &Project,
        supersede: bool,
        before_publish: impl FnOnce() -> io::Result<()>,
        sync_directory: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Result<(), StorageError> {
        self.checkpoint_with_release(
            project,
            supersede,
            before_publish,
            sync_directory,
            SlotLock::unlock,
        )
    }

    fn checkpoint_with_release(
        &mut self,
        project: &Project,
        supersede: bool,
        before_publish: impl FnOnce() -> io::Result<()>,
        sync_directory: impl FnOnce(&Path) -> io::Result<()>,
        release: impl FnOnce(&File) -> io::Result<()>,
    ) -> Result<(), StorageError> {
        let mut lock = lock_slot(&self.path)?;
        let mut change = JournalChange::Unchanged;
        let result = (|| {
            if supersede {
                let bytes = read(&self.path)?;
                if let Some(bytes) = &bytes {
                    let _ = decode(&self.path, bytes)?;
                }
                self.bytes = bytes;
            }
            self.check()?;
            let mut bytes =
                replacement::encode_envelope(&basename(&self.path)?, &encode(project)?)?;
            bytes[..8].copy_from_slice(MAGIC);
            let generation = next_generation(lock.file_mut(), self.bytes.as_deref())?;
            bytes.extend_from_slice(&generation.to_le_bytes());
            let mut temp = replacement::prepare_temp(parent(&self.path), &bytes, None)?;
            self.check()?;
            before_publish()?;
            if self.bytes.is_some() {
                fs::rename(&temp.path, &self.path)?;
                change = JournalChange::CheckpointPublished;
                temp.disarm();
            } else {
                let published = crate::publication::publish_new(&temp.path, &self.path)?;
                self.bytes = Some(bytes.clone());
                change = JournalChange::CheckpointPublished;
                if published.staging_remains() {
                    fs::remove_file(&temp.path).map_err(|source| {
                        StorageError::PublishedButNotDurable {
                            path: self.path.clone(),
                            operation: "remove the journal staging file",
                            source,
                        }
                    })?;
                }
                temp.disarm();
            }
            self.bytes = Some(bytes);
            sync_directory(parent(&self.path)).map_err(|source| {
                StorageError::PublishedButNotDurable {
                    path: self.path.clone(),
                    operation: "synchronize the journal directory",
                    source,
                }
            })
        })();
        lock.finish_with(&self.path, change, result, release)
    }

    /// Clear only this exact checkpoint after a durable save; never remove a
    /// competing writer's checkpoint. There is no history sidecar to clean up.
    pub fn clear(&mut self) -> Result<(), StorageError> {
        self.clear_with_release(SlotLock::unlock)
    }

    fn clear_with_release(
        &mut self,
        release: impl FnOnce(&File) -> io::Result<()>,
    ) -> Result<(), StorageError> {
        let lock = lock_slot(&self.path)?;
        let mut change = JournalChange::Unchanged;
        let result = (|| {
            self.check()?;
            if self.bytes.is_some() {
                fs::remove_file(&self.path)?;
                self.bytes = None;
                change = JournalChange::CheckpointCleared;
                replacement::sync_parent(parent(&self.path))?;
            }
            Ok(())
        })();
        lock.finish_with(&self.path, change, result, release)
    }

    /// Explicit user discard of an observed leftover checkpoint (Pulse #3990).
    /// Under the slot lock the exact bytes are copied into a new private
    /// preserved-recovery sibling directory before the slot is removed, so a
    /// discard never deletes recoverable work. Returns the kept copy's path.
    pub fn set_aside(&mut self) -> Result<PathBuf, StorageError> {
        let lock = lock_slot(&self.path)?;
        let mut change = JournalChange::Unchanged;
        let mut kept = None;
        let result = (|| {
            self.check()?;
            let bytes = self
                .bytes
                .as_deref()
                .ok_or(StorageError::Invalid("no recovery journal to set aside"))?;
            kept = Some(restart::preserve(&self.path, bytes)?);
            fs::remove_file(&self.path)?;
            self.bytes = None;
            change = JournalChange::CheckpointCleared;
            replacement::sync_parent(parent(&self.path))?;
            Ok(())
        })();
        lock.finish_with(&self.path, change, result, SlotLock::unlock)?;
        kept.ok_or(StorageError::Invalid("no recovery journal to set aside"))
    }

    fn check(&self) -> Result<(), StorageError> {
        if read(&self.path)? != self.bytes {
            return Err(StorageError::JournalConflict {
                path: self.path.clone(),
            });
        }
        Ok(())
    }
}

// Generation survives clearing the slot so a byte-identical checkpoint from a
// newer untitled project cannot be deleted by an older owner (including ABA).
fn next_generation(lock: &mut File, current: Option<&[u8]>) -> Result<u64, StorageError> {
    let mut previous = [0; 8];
    if lock.metadata()?.len() == 16 {
        lock.seek(SeekFrom::Start(8))?;
        lock.read_exact(&mut previous)?;
    }
    let in_journal = current.map(generation).transpose()?.unwrap_or(0);
    let next = u64::from_le_bytes(previous)
        .max(in_journal)
        .checked_add(1)
        .ok_or(StorageError::Invalid("journal generation exhausted"))?;
    lock.seek(SeekFrom::Start(0))?;
    lock.write_all(LOCK_MAGIC)?;
    lock.write_all(&next.to_le_bytes())?;
    lock.sync_all()?;
    Ok(next)
}
fn generation(bytes: &[u8]) -> Result<u64, StorageError> {
    if bytes.len() < 32 {
        return Err(StorageError::Invalid("invalid journal generation"));
    }
    let generation = u64::from_le_bytes(
        bytes[bytes.len() - 8..]
            .try_into()
            .map_err(|_| StorageError::Invalid("invalid journal generation"))?,
    );
    if generation == 0 {
        return Err(StorageError::Invalid("invalid journal generation"));
    }
    Ok(generation)
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, StorageError> {
    let metadata = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        result => result?,
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StorageError::Invalid(
            "journal must be a regular non-symbolic file",
        ));
    }
    Ok(Some(crate::files::read_document(path)?))
}

fn decode(path: &Path, bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.get(..8) != Some(MAGIC) {
        return Err(StorageError::Invalid("unrecognized recovery journal"));
    }
    let _generation = generation(bytes)?;
    let mut envelope = bytes[..bytes.len() - 8].to_vec();
    envelope[..8].copy_from_slice(b"HEXRCVY\0");
    let (project, _token): (Project, FileToken) =
        replacement::decode_envelope(&envelope, &bound_name(path, &envelope)?)?;
    Ok(project)
}

/// The target name a journal at `path` may carry. A case-only or Unicode-
/// normalization rename leaves the old name inside the checkpoint; adopt it only
/// when that stored name still reaches this same file on this filesystem.
fn bound_name(path: &Path, envelope: &[u8]) -> Result<String, StorageError> {
    let name = basename(path)?;
    let (stored, _) = replacement::parse_envelope(envelope)?;
    let plain_component = Path::new(stored).file_name() == Some(stored.as_ref());
    if stored != name && plain_component && same_file(path, &parent(path).join(stored)) {
        return Ok(stored.to_owned());
    }
    Ok(name)
}

fn same_file(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::symlink_metadata(left), fs::symlink_metadata(right)) {
            (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        false
    }
}

fn basename(path: &Path) -> Result<String, StorageError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or(StorageError::Invalid("journal filename must be utf-8"))
}
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-journal-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("isolated directory");
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Regression for #3915: the first checkpoint on a volume without hard
    /// links or exclusive rename publishes a complete copy, not an error.
    #[cfg(unix)]
    #[test]
    fn first_checkpoint_publishes_on_volumes_without_hard_links() {
        use crate::publication::simulate::{Volume, volume};
        for capability in [Volume::NoHardLinks(libc::EPERM), Volume::NoAtomicCreate] {
            let dir = Temp::new();
            let _volume = volume(capability);
            let path = dir.0.join("usb.hex-journal");
            let project = Project::new(48_000).expect("project");
            let mut journal = Journal::open(path.clone()).expect("open");
            journal.checkpoint(&project).expect("first checkpoint");
            assert_eq!(load_journal(&path).expect("complete checkpoint"), project);
            let newer = Project::new(44_100).expect("newer");
            journal.checkpoint(&newer).expect("replacing checkpoint");
            assert_eq!(load_journal(&path).expect("replaced checkpoint"), newer);
            let leftovers = fs::read_dir(&dir.0)
                .expect("directory")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(".hex-"))
                .count();
            assert_eq!(leftovers, 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn owned_lock_success_and_validation_error_release_inherited_descriptions() {
        let dir = Temp::new();
        let path = dir.0.join("untitled.hex-journal");
        let project = Project::new(48_000).expect("project");
        Journal::open(path.clone())
            .expect("open")
            .checkpoint(&project)
            .expect("checkpoint");
        let checkpoint = fs::read(&path).expect("checkpoint bytes");
        let lock_path = dir.0.join("untitled.hex-journal.lock");
        let valid_lock = fs::read(&lock_path).expect("generation");
        let mut inherited = None;
        let owner = slot_lock::acquire_with(
            &path,
            |file| {
                inherited = Some(file.try_clone()?);
                Ok(())
            },
            SlotLock::unlock,
        )
        .expect("acquired owner");
        assert!(
            matches!(lock_slot(&path), Err(StorageError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock)
        );
        owner
            .finish_with(&path, JournalChange::Unchanged, Ok(()), SlotLock::unlock)
            .expect("checked release");
        lock_slot(&path)
            .expect("reacquire while duplicate lives")
            .finish_with(&path, JournalChange::Unchanged, Ok(()), SlotLock::unlock)
            .expect("release second owner");
        assert!(inherited.is_some());
        assert_eq!(fs::read(&path).expect("unchanged checkpoint"), checkpoint);
        assert_eq!(
            fs::read(&lock_path).expect("unchanged generation"),
            valid_lock
        );
        drop(inherited.take());

        fs::write(&lock_path, b"external sixteen").expect("foreign magic");
        let result = slot_lock::acquire_with(
            &path,
            |file| {
                inherited = Some(file.try_clone()?);
                Ok(())
            },
            SlotLock::unlock,
        );
        assert!(matches!(
            result,
            Err(StorageError::Invalid(
                "unrecognized journal generation lock"
            ))
        ));
        assert_eq!(
            fs::read(&lock_path).expect("foreign preserved"),
            b"external sixteen"
        );
        assert!(inherited.is_some());
        fs::write(&lock_path, &valid_lock).expect("restore own fixture");
        lock_slot(&path)
            .expect("validation error released despite duplicate")
            .finish_with(&path, JournalChange::Unchanged, Ok(()), SlotLock::unlock)
            .expect("release");
        assert_eq!(fs::read(&path).expect("checkpoint preserved"), checkpoint);
        drop(inherited);
    }

    fn injected_release_failure(_: &File) -> io::Result<()> {
        Err(io::Error::other("injected unlock failure"))
    }

    fn assert_release_error(
        error: StorageError,
        path: &Path,
        change: JournalChange,
        original: bool,
    ) {
        let display = error.to_string();
        assert!(display.contains("journal lock release failed"));
        assert!(std::error::Error::source(&error).is_some());
        match error {
            StorageError::JournalLockRelease {
                path: actual,
                change: actual_change,
                operation_error,
                source,
            } => {
                assert_eq!(actual, path);
                assert_eq!(actual_change, change);
                assert_eq!(operation_error.is_some(), original);
                assert_eq!(source.to_string(), "injected unlock failure");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn unlock_failure_reports_actual_state_and_preserves_original_errors() {
        let dir = Temp::new();
        let path = dir.0.join("untitled.hex-journal");
        let first = Project::new(48_000).expect("first");
        let second = Project::new(44_100).expect("second");
        let mut journal = Journal::open(path.clone()).expect("open");
        journal.checkpoint(&first).expect("initial");
        let before = fs::read(&path).expect("checkpoint");
        let error = journal
            .checkpoint_with_release(
                &second,
                false,
                || Err(io::Error::other("original prepublication failure")),
                replacement::sync_parent,
                injected_release_failure,
            )
            .expect_err("release error");
        if let StorageError::JournalLockRelease {
            operation_error: Some(original),
            ..
        } = &error
        {
            assert_eq!(
                original.to_string(),
                "project file error: original prepublication failure"
            );
        } else {
            panic!("original error missing");
        }
        assert_release_error(error, &path, JournalChange::Unchanged, true);
        assert_eq!(fs::read(&path).expect("old checkpoint preserved"), before);

        let error = journal
            .checkpoint_with_release(
                &second,
                false,
                || Ok(()),
                replacement::sync_parent,
                injected_release_failure,
            )
            .expect_err("published release error");
        assert_release_error(error, &path, JournalChange::CheckpointPublished, false);
        assert_eq!(load_journal(&path).expect("published state"), second);
        assert_eq!(journal.bytes, Some(fs::read(&path).expect("adopted bytes")));

        let error = journal
            .checkpoint_with_release(
                &first,
                false,
                || Ok(()),
                |_| Err(io::Error::other("original directory sync failure")),
                injected_release_failure,
            )
            .expect_err("both errors");
        if let StorageError::JournalLockRelease {
            operation_error: Some(original),
            ..
        } = &error
        {
            assert!(matches!(
                original.as_ref(),
                StorageError::PublishedButNotDurable { .. }
            ));
        } else {
            panic!("original sync error missing");
        }
        assert_release_error(error, &path, JournalChange::CheckpointPublished, true);
        assert_eq!(
            load_journal(&path).expect("published despite cleanup"),
            first
        );

        let error = journal
            .clear_with_release(injected_release_failure)
            .expect_err("cleared release error");
        assert_release_error(error, &path, JournalChange::CheckpointCleared, false);
        assert!(journal.bytes.is_none());
        assert!(!path.exists());
        assert!(dir.0.join("untitled.hex-journal.lock").exists());
        let mut fresh = Journal::open(path.clone()).expect("new owner");
        fresh
            .checkpoint_latest(&first)
            .expect("recreated same content");
        let recreated = fs::read(&path).expect("new generation");
        assert!(matches!(
            journal.clear(),
            Err(StorageError::JournalConflict { .. })
        ));
        assert_eq!(
            fs::read(&path).expect("retry preserved new owner"),
            recreated
        );

        let lock_path = dir.0.join("untitled.hex-journal.lock");
        fs::write(&lock_path, b"external sixteen").expect("foreign lock");
        let result = slot_lock::acquire_with(&path, |_| Ok(()), injected_release_failure);
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("invalid magic accepted"),
        };
        if let StorageError::JournalLockRelease {
            operation_error: Some(original),
            ..
        } = &error
        {
            assert!(matches!(
                original.as_ref(),
                StorageError::Invalid("unrecognized journal generation lock")
            ));
        } else {
            panic!("validation error missing");
        }
        assert_release_error(error, &path, JournalChange::Unchanged, true);
        assert_eq!(
            fs::read(&lock_path).expect("foreign remains"),
            b"external sixteen"
        );
        assert_eq!(fs::read(&path).expect("checkpoint remains"), recreated);
    }

    #[test]
    fn set_aside_keeps_exact_bytes_and_refuses_a_changed_slot() {
        let dir = Temp::new();
        let path = dir.0.join("untitled.hex-journal");
        let crashed = Project::new(48_000).expect("crashed");
        Journal::open(path.clone())
            .expect("open")
            .checkpoint_latest(&crashed)
            .expect("checkpoint");
        let bytes = fs::read(&path).expect("leftover");
        let mut stale = Journal::open(path.clone()).expect("observe");
        let mut leftover = Journal::open(path.clone()).expect("observe");
        let kept = leftover.set_aside().expect("set aside");
        assert!(!path.exists(), "slot cleared");
        assert_eq!(fs::read(&kept).expect("kept"), bytes);
        assert_eq!(load_journal(&kept).expect("kept loads"), crashed);
        assert!(stale.set_aside().is_err(), "a changed slot refuses");
        assert_eq!(fs::read(&kept).expect("kept unchanged"), bytes);
    }

    #[test]
    fn latest_slot_has_no_history_and_old_owner_cannot_delete_newer_project() {
        let dir = Temp::new();
        let path = dir.0.join("untitled.hex-journal");
        let first = Project::new(48_000).expect("first");
        let second = Project::new(44_100).expect("second");
        let mut older = Journal::open(path.clone()).expect("open");
        older.checkpoint_latest(&first).expect("first checkpoint");
        let mut newer = Journal::open(path.clone()).expect("new process");
        newer.checkpoint_latest(&second).expect("latest checkpoint");
        assert_eq!(load_journal(&path).expect("latest"), second);
        let stale = older.clear();
        assert!(matches!(&stale, Err(StorageError::JournalConflict { path: p }) if *p == path));
        // Journal conflicts keep their recovery wording; project save-as guidance
        // belongs only to document conflicts.
        let message = stale
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(message.contains("changed outside hex") && !message.contains("save as"));
        assert!(matches!(
            older.checkpoint(&first),
            Err(StorageError::JournalConflict { .. })
        ));
        older
            .checkpoint_latest(&first)
            .expect("new latest edit supersedes recognized slot");
        assert_eq!(load_journal(&path).expect("latest again"), first);
        assert_eq!(
            fs::read_dir(&dir.0).expect("list").count(),
            2,
            "one checkpoint plus a generation lock, no history"
        );
        let mut same = Journal::open(path.clone()).expect("newer identical project");
        same.checkpoint_latest(&first)
            .expect("identical project gets distinct ownership");
        assert!(matches!(
            older.clear(),
            Err(StorageError::JournalConflict { .. })
        ));
        same.clear().expect("durable saved checkpoint clears slot");
        let mut after_clear = Journal::open(path.clone()).expect("new project after clear");
        after_clear
            .checkpoint_latest(&first)
            .expect("same content after clear");
        assert!(matches!(
            older.clear(),
            Err(StorageError::JournalConflict { .. })
        ));
        after_clear.clear().expect("new owner can clear");
        assert!(!path.exists());
    }

    #[test]
    fn unrecognized_truncated_oversized_wrong_binding_and_symlinks_are_preserved() {
        let dir = Temp::new();
        let path = dir.0.join("untitled.hex-journal");
        let project = Project::new(48_000).expect("project");
        let mut journal = Journal::open(path.clone()).expect("open");
        journal.checkpoint(&project).expect("valid checkpoint");
        let valid = fs::read(&path).expect("valid bytes");
        let mut future = valid.clone();
        future[8..12].copy_from_slice(&2_u32.to_le_bytes());
        let mut wrong_binding = valid.clone();
        wrong_binding[24] ^= 1;
        for bytes in [
            b"unrelated data".to_vec(),
            valid[..valid.len() - 1].to_vec(),
            vec![0; 32],
            wrong_binding,
            future,
        ] {
            fs::write(&path, &bytes).expect("fixture");
            assert!(Journal::open(path.clone()).is_err());
            assert!(journal.checkpoint_latest(&project).is_err());
            assert_eq!(fs::read(&path).expect("preserved"), bytes);
        }
        #[cfg(unix)]
        {
            fs::remove_file(&path).expect("fixture removal");
            let other = dir.0.join("other");
            fs::write(&other, &valid).expect("external bytes");
            std::os::unix::fs::symlink(&other, &path).expect("fixture symlink");
            assert!(journal.checkpoint_latest(&project).is_err());
            assert_eq!(fs::read_link(&path).expect("link preserved"), other);
            assert_eq!(fs::read(&other).expect("external preserved"), valid);
        }
    }

    #[test]
    fn foreign_lock_files_and_cross_process_contention_preserve_checkpoint() {
        let dir = Temp::new();
        let path = dir.0.join("untitled.hex-journal");
        let lock_path = dir.0.join("untitled.hex-journal.lock");
        let first = Project::new(48_000).expect("first");
        let mut journal = Journal::open(path.clone()).expect("open");
        journal.checkpoint(&first).expect("initial");
        let valid_lock = fs::read(&lock_path).expect("generation lock");
        fs::write(&lock_path, b"external sixteen").expect("unknown lock fixture");
        assert!(journal.checkpoint_latest(&first).is_err());
        assert_eq!(
            fs::read(&lock_path).expect("foreign lock preserved"),
            b"external sixteen"
        );
        fs::write(&lock_path, &valid_lock).expect("restore own fixture");
        let lock = lock_slot(&path).expect("parent holds exclusive lock");
        let status = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "journal::tests::crash_child", "--nocapture"])
            .env("HEX_JOURNAL_CRASH_PATH", &path)
            .env("HEX_JOURNAL_CRASH_STAGE", "contended")
            .status()
            .expect("child contender");
        assert_eq!(status.code(), Some(74));
        assert_eq!(load_journal(&path).expect("checkpoint preserved"), first);
        drop(lock);
        journal
            .checkpoint(&first)
            .expect("released lock permits retry");
    }

    #[test]
    fn prepublication_failure_preserves_old_checkpoint_and_postpublication_warning_adopts_new_bytes()
     {
        let dir = Temp::new();
        let path = dir.0.join("untitled.hex-journal");
        let first = Project::new(48_000).expect("first");
        let second = Project::new(44_100).expect("second");
        let mut journal = Journal::open(path.clone()).expect("open");
        journal.checkpoint(&first).expect("initial");
        assert!(
            journal
                .checkpoint_with_hooks(
                    &second,
                    false,
                    || Err(io::Error::other("injected disk failure")),
                    |_| Ok(())
                )
                .is_err()
        );
        assert_eq!(load_journal(&path).expect("old checkpoint"), first);
        assert!(matches!(
            journal.checkpoint_with_hooks(
                &second,
                false,
                || Ok(()),
                |_| Err(io::Error::other("injected sync failure"))
            ),
            Err(StorageError::PublishedButNotDurable { .. })
        ));
        assert_eq!(
            load_journal(&path).expect("new complete checkpoint"),
            second
        );
        journal
            .checkpoint(&first)
            .expect("published bytes were adopted despite warning");
    }

    #[test]
    fn abrupt_child_exit_during_staging_and_after_publication_leaves_loadable_checkpoint() {
        let dir = Temp::new();
        let path = dir.0.join("untitled.hex-journal");
        let first = Project::new(48_000).expect("first");
        let second = Project::new(44_100).expect("second");
        Journal::open(path.clone())
            .expect("open")
            .checkpoint(&first)
            .expect("initial");
        for (stage, expected) in [("before", &first), ("after", &second)] {
            let status = Command::new(std::env::current_exe().expect("test binary"))
                .args(["--exact", "journal::tests::crash_child", "--nocapture"])
                .env("HEX_JOURNAL_CRASH_PATH", &path)
                .env("HEX_JOURNAL_CRASH_STAGE", stage)
                .status()
                .expect("child process");
            assert_eq!(status.code(), Some(73));
            assert_eq!(
                &load_journal(&path).expect("restart reads complete checkpoint"),
                expected
            );
            Journal::open(path.clone())
                .expect("restart owner")
                .checkpoint(expected)
                .expect("OS released crash lock");
        }
    }

    #[test]
    fn crash_child() {
        let Some(path) = std::env::var_os("HEX_JOURNAL_CRASH_PATH") else {
            return;
        };
        let stage = std::env::var("HEX_JOURNAL_CRASH_STAGE").expect("stage");
        let mut journal = Journal::open(path.into()).expect("child journal");
        let second = Project::new(44_100).expect("second");
        if stage == "contended" {
            assert!(journal.checkpoint(&second).is_err());
            std::process::exit(74);
        }
        let _ = journal.checkpoint_with_hooks(
            &second,
            false,
            || {
                if stage == "before" {
                    std::process::exit(73);
                }
                Ok(())
            },
            |_| {
                std::process::exit(73);
            },
        );
        panic!("child must terminate at injected boundary");
    }

    #[cfg(unix)]
    #[test]
    fn journal_reached_through_an_equivalent_name_is_adopted() {
        let dir = Temp::new();
        let old = dir.0.join(".song.hex.hex-journal");
        let renamed = dir.0.join(".Song.hex.hex-journal");
        let first = Project::new(48_000).expect("project");
        Journal::open(old.clone())
            .expect("open")
            .checkpoint(&first)
            .expect("checkpoint");

        let copy = dir.0.join(".other.hex.hex-journal");
        fs::copy(&old, &copy).expect("copied journal");
        let error = load_journal(&copy).expect_err("foreign journal refused");
        assert!(error.to_string().contains("use save as"));
        if fs::symlink_metadata(&renamed).is_err() {
            eprintln!("skipped: temp volume is case-sensitive");
            return;
        }
        assert_eq!(load_journal(&renamed).expect("adopted journal"), first);
        let second = Project::new(44_100).expect("project");
        Journal::open(renamed.clone())
            .expect("adopt")
            .checkpoint(&second)
            .expect("checkpoint under new name");
        assert_eq!(load_journal(&renamed).expect("new checkpoint"), second);
    }
}
