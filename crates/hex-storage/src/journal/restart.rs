//! Preserve exact old journal bytes before replacing a captured slot.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Bounded exact-byte observation, not permission to replace a journal. For an
/// unreadable slot the caller must obtain explicit confirmation and validate its
/// working-project context. A readable leftover may be preserved before a new edit.
#[derive(Clone)]
pub struct JournalRestartToken {
    path: PathBuf,
    bytes: Arc<[u8]>,
}
impl std::fmt::Debug for JournalRestartToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalRestartToken")
            .field("path", &self.path)
            .field("byte_count", &self.bytes.len())
            .finish()
    }
}
impl JournalRestartToken {
    /// Capture only a recognized but unreadable regular journal. No file
    /// is changed; readable checkpoints, missing slots and symbolic links refuse.
    pub fn capture(path: PathBuf) -> Result<Self, StorageError> {
        let bytes =
            read(&path)?.ok_or(StorageError::Invalid("no unavailable journal to preserve"))?;
        if bytes.get(..8) != Some(MAGIC) {
            return Err(StorageError::Invalid("unrecognized recovery journal"));
        }
        generation(&bytes)?;
        if decode(&path, &bytes).is_ok() {
            return Err(StorageError::Invalid(
                "recovery journal is already readable",
            ));
        }
        Ok(Self {
            path,
            bytes: bytes.into(),
        })
    }

    /// Observe a readable leftover checkpoint for preservation before replacement.
    /// The publication rechecks these exact bytes while holding the slot lock.
    pub fn capture_readable(path: PathBuf) -> Result<Option<Self>, StorageError> {
        let Some(bytes) = read(&path)? else {
            return Ok(None);
        };
        decode(&path, &bytes)?;
        Ok(Some(Self {
            path,
            bytes: bytes.into(),
        }))
    }
}

/// Publication and diagnostics are separate: after a visible replacement, retain
/// this exact Journal even if final directory sync or lock release reports failure.
/// A preserved path may also exist after a prepublication failure; it is never removed.
pub struct JournalRestartOutcome {
    pub journal: Option<Journal>,
    pub preserved_path: Option<PathBuf>,
    pub result: Result<(), StorageError>,
}

/// Fresh current-format checkpoint under the existing slot lock, preserving the
/// captured bytes. Unreadable-slot callers must obtain explicit confirmation.
/// The old journal's bytes and basename survive in a new private sibling directory;
/// no legacy decoding/conversion, overwriting of preservation files, or cleanup.
/// Cooperating writers serialize; as with checkpoint, a noncooperating writer
/// can still race the final exact-byte check and filesystem replacement.
pub fn restart_journal(token: &JournalRestartToken, project: &Project) -> JournalRestartOutcome {
    restart_with_hooks(token, project, || Ok(()), replacement::sync_parent)
}

fn restart_with_hooks(
    token: &JournalRestartToken,
    project: &Project,
    before_publish: impl FnOnce() -> io::Result<()>,
    sync_directory: impl FnOnce(&Path) -> io::Result<()>,
) -> JournalRestartOutcome {
    let mut outcome = JournalRestartOutcome {
        journal: None,
        preserved_path: None,
        result: Ok(()),
    };
    let mut lock = match lock_slot(&token.path) {
        Ok(lock) => lock,
        Err(error) => {
            outcome.result = Err(error);
            return outcome;
        }
    };
    let mut change = JournalChange::Unchanged;
    let result = (|| {
        let check = || -> Result<(), StorageError> {
            if read(&token.path)?.as_deref() != Some(token.bytes.as_ref()) {
                return Err(StorageError::JournalConflict {
                    path: token.path.clone(),
                });
            }
            Ok(())
        };
        check()?;
        let mut bytes = replacement::encode_envelope(&basename(&token.path)?, &encode(project)?)?;
        bytes[..8].copy_from_slice(MAGIC);
        let next = next_generation(lock.file_mut(), Some(&token.bytes))?;
        bytes.extend_from_slice(&next.to_le_bytes());
        let mut temp = replacement::prepare_temp(parent(&token.path), &bytes, None)?;
        outcome.preserved_path = Some(preserve(&token.path, &token.bytes)?);
        check()?;
        before_publish()?;
        fs::rename(&temp.path, &token.path)?;
        temp.disarm();
        change = JournalChange::CheckpointPublished;
        outcome.journal = Some(Journal {
            path: token.path.clone(),
            bytes: Some(bytes),
        });
        sync_directory(parent(&token.path)).map_err(|source| StorageError::PublishedButNotDurable {
            path: token.path.clone(),
            operation: "synchronize the restarted journal directory",
            source,
        })
    })();
    outcome.result = lock.finish_with(&token.path, change, result, SlotLock::unlock);
    outcome
}

pub(super) fn preserve(path: &Path, bytes: &[u8]) -> Result<PathBuf, StorageError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for _ in 0..64 {
        let directory = parent(path).join(format!(
            "preserved-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
            Ok(()) => {}
        }
        let preserved = directory.join(basename(path)?);
        let result = (|| -> Result<(), StorageError> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&preserved)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            replacement::sync_parent(&directory)?;
            replacement::sync_parent(parent(path))?;
            Ok(())
        })();
        if let Err(error) = result {
            // Only this operation's incomplete preservation copy is disposable.
            // The original slot has not been touched.
            let _ = fs::remove_file(&preserved);
            let _ = fs::remove_dir(&directory);
            return Err(error);
        }
        return Ok(preserved);
    }
    Err(StorageError::Invalid(
        "could not reserve a unique recovery preservation directory",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "hex-restart-journal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn unsupported(&self) -> (PathBuf, Vec<u8>, JournalRestartToken) {
            let path = self.0.join("untitled.hex-journal");
            let mut bytes = replacement::encode_envelope(
                "untitled.hex-journal",
                br#"{"format":"hex-project","version":0}"#,
            )
            .unwrap();
            bytes[..8].copy_from_slice(MAGIC);
            bytes.extend_from_slice(&1_u64.to_le_bytes());
            fs::write(&path, &bytes).unwrap();
            assert!(
                Journal::open(path.clone())
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("unsupported older project schema")
            );
            let token = JournalRestartToken::capture(path.clone()).unwrap();
            (path, bytes, token)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn readable_checkpoint_is_preserved_and_stale_observation_refuses_replacement() {
        let fixture = Fixture::new();
        let path = fixture.0.join("untitled.hex-journal");
        let mut journal = Journal::open_empty(path.clone()).unwrap();
        let original = Project::new(48_000).unwrap();
        journal.checkpoint(&original).unwrap();
        let bytes = fs::read(&path).unwrap();
        let token = JournalRestartToken::capture_readable(path.clone())
            .unwrap()
            .unwrap();
        let replacement = Project::new(44_100).unwrap();
        let outcome = restart_journal(&token, &replacement);
        outcome.result.unwrap();
        let preserved = outcome.preserved_path.unwrap();
        assert_eq!(fs::read(&preserved).unwrap(), bytes);
        assert_eq!(load_journal(&preserved).unwrap(), original);
        assert_eq!(load_journal(&path).unwrap(), replacement);
        let stale = restart_journal(&token, &original);
        assert!(matches!(
            stale.result,
            Err(StorageError::JournalConflict { .. })
        ));
        assert!(stale.preserved_path.is_none());
        assert_eq!(load_journal(&path).unwrap(), replacement);
        assert!(Journal::open_empty(path).is_err());
    }

    #[test]
    fn confirmed_restart_preserves_unsupported_bytes_and_resumes_current_checkpoints() {
        let fixture = Fixture::new();
        let (path, original, token) = fixture.unsupported();
        let project = Project::new(48_000).unwrap();
        let outcome = restart_journal(&token, &project);
        outcome.result.unwrap();
        let preserved = outcome.preserved_path.unwrap();
        assert_eq!(
            preserved.file_name(),
            path.file_name(),
            "preserve envelope binding"
        );
        assert_eq!(fs::read(&preserved).unwrap(), original);
        assert!(
            load_journal(&preserved)
                .unwrap_err()
                .to_string()
                .contains("unsupported older project schema")
        );
        assert_eq!(load_journal(&path).unwrap(), project);
        let mut journal = outcome.journal.unwrap();
        let next = Project::new(44_100).unwrap();
        journal.checkpoint_latest(&next).unwrap();
        assert_eq!(load_journal(&path).unwrap(), next);
        journal.clear().unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(&preserved).unwrap(), original);
    }

    #[test]
    fn stale_restart_token_cannot_preserve_or_replace_a_foreign_slot() {
        let fixture = Fixture::new();
        let (path, _, token) = fixture.unsupported();
        let foreign = b"different external journal bytes";
        fs::write(&path, foreign).unwrap();
        let outcome = restart_journal(&token, &Project::new(48_000).unwrap());
        assert!(matches!(
            outcome.result,
            Err(StorageError::JournalConflict { .. })
        ));
        assert!(outcome.journal.is_none() && outcome.preserved_path.is_none());
        assert_eq!(fs::read(&path).unwrap(), foreign);
    }

    #[test]
    fn restart_prepublication_failure_keeps_original_and_durable_preservation() {
        let fixture = Fixture::new();
        let (path, original, token) = fixture.unsupported();
        let outcome = restart_with_hooks(
            &token,
            &Project::new(48_000).unwrap(),
            || Err(io::Error::other("injected publication refusal")),
            replacement::sync_parent,
        );
        assert!(
            outcome
                .result
                .unwrap_err()
                .to_string()
                .contains("injected publication refusal")
        );
        assert!(outcome.journal.is_none());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(outcome.preserved_path.unwrap()).unwrap(), original);
    }

    #[test]
    fn restart_postpublication_warning_retains_actual_new_ownership() {
        let fixture = Fixture::new();
        let (path, original, token) = fixture.unsupported();
        let project = Project::new(48_000).unwrap();
        let outcome = restart_with_hooks(
            &token,
            &project,
            || Ok(()),
            |_| Err(io::Error::other("injected final directory sync failure")),
        );
        assert!(matches!(
            outcome.result,
            Err(StorageError::PublishedButNotDurable { .. })
        ));
        assert_eq!(load_journal(&path).unwrap(), project);
        assert_eq!(fs::read(outcome.preserved_path.unwrap()).unwrap(), original);
        let mut journal = outcome.journal.unwrap();
        journal.checkpoint(&project).unwrap();
        assert_eq!(load_journal(&path).unwrap(), project);
        assert!(
            JournalRestartToken::capture(path).is_err(),
            "readable slots cannot be reset"
        );
    }
}
