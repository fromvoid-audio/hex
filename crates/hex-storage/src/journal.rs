//! One complete checkpoint per slot; no backup history or implicit recovery.
use crate::{FileToken, MAX_DOCUMENT_BYTES, StorageError, encode, replacement};
use hex_project::Project;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"HEXJRNL\0";
const LOCK_MAGIC: &[u8; 8] = b"HEXJLOC\0";
const MAX_BYTES: usize = MAX_DOCUMENT_BYTES + 287;

/// Journal beside a saved project. Untitled uses a caller-supplied application slot.
pub fn journal_path(target: &Path) -> Result<PathBuf, StorageError> {
    let name = basename(target)?;
    let name = format!(".{name}.hex-journal");
    if name.len() > 250 {
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
    /// Adopt only a recognized, bounded, valid journal. Unknown files are preserved.
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
        let mut lock = lock_slot(&self.path)?;
        if supersede {
            let bytes = read(&self.path)?;
            if let Some(bytes) = &bytes {
                let _ = decode(&self.path, bytes)?;
            }
            self.bytes = bytes;
        }
        self.check()?;
        let mut bytes = replacement::encode_envelope(&basename(&self.path)?, &encode(project)?)?;
        bytes[..8].copy_from_slice(MAGIC);
        let generation = next_generation(&mut lock, self.bytes.as_deref())?;
        bytes.extend_from_slice(&generation.to_le_bytes());
        let mut temp = replacement::prepare_temp(parent(&self.path), &bytes, None)?;
        self.check()?;
        before_publish()?;
        if self.bytes.is_some() {
            fs::rename(&temp.path, &self.path)?;
            temp.disarm();
        } else {
            fs::hard_link(&temp.path, &self.path)?;
            self.bytes = Some(bytes.clone());
            fs::remove_file(&temp.path).map_err(|source| StorageError::PublishedButNotDurable {
                path: self.path.clone(),
                operation: "remove the journal staging file",
                source,
            })?;
            temp.disarm();
        }
        self.bytes = Some(bytes);
        sync_directory(parent(&self.path)).map_err(|source| StorageError::PublishedButNotDurable {
            path: self.path.clone(),
            operation: "synchronize the journal directory",
            source,
        })
    }

    /// Clear only this exact checkpoint after a durable save; never remove a
    /// competing writer's checkpoint. There is no history sidecar to clean up.
    pub fn clear(&mut self) -> Result<(), StorageError> {
        let _lock = lock_slot(&self.path)?;
        self.check()?;
        if self.bytes.is_some() {
            fs::remove_file(&self.path)?;
            self.bytes = None;
            replacement::sync_parent(parent(&self.path))?;
        }
        Ok(())
    }

    fn check(&self) -> Result<(), StorageError> {
        if read(&self.path)? != self.bytes {
            return Err(StorageError::Conflict {
                path: self.path.clone(),
            });
        }
        Ok(())
    }
}

// Stable lock inode retains only magic and an eight-byte generation, never project history. The OS
// releases the exclusive advisory lock on crash. Never unlink a live lock inode.
fn lock_slot(path: &Path) -> Result<File, StorageError> {
    let lock = parent(path).join(format!("{}.lock", basename(path)?));
    match fs::symlink_metadata(&lock) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || !matches!(metadata.len(), 0 | 16) =>
        {
            return Err(StorageError::Invalid(
                "journal lock must be a regular non-symbolic file",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(lock)?;
    fs2::FileExt::try_lock_exclusive(&file)?;
    if file.metadata()?.len() == 16 {
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        if &magic != LOCK_MAGIC {
            return Err(StorageError::Invalid(
                "unrecognized journal generation lock",
            ));
        }
    }
    Ok(file)
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
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(StorageError::Invalid("journal exceeds byte limit"));
    }
    Ok(Some(bytes))
}

fn decode(path: &Path, bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.get(..8) != Some(MAGIC) {
        return Err(StorageError::Invalid("unrecognized recovery journal"));
    }
    let _generation = generation(bytes)?;
    let mut envelope = bytes[..bytes.len() - 8].to_vec();
    envelope[..8].copy_from_slice(b"HEXRCVY\0");
    let (project, _token): (Project, FileToken) =
        replacement::decode_envelope(&envelope, &basename(path)?)?;
    Ok(project)
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
        assert!(matches!(older.clear(), Err(StorageError::Conflict { .. })));
        assert!(matches!(
            older.checkpoint(&first),
            Err(StorageError::Conflict { .. })
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
        assert!(matches!(older.clear(), Err(StorageError::Conflict { .. })));
        same.clear().expect("durable saved checkpoint clears slot");
        let mut after_clear = Journal::open(path.clone()).expect("new project after clear");
        after_clear
            .checkpoint_latest(&first)
            .expect("same content after clear");
        assert!(matches!(older.clear(), Err(StorageError::Conflict { .. })));
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
            vec![0; MAX_BYTES + 1],
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
}
