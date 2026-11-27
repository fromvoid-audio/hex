//! Explicit read-only inspection, safe recovery, and digest-verified discard
//! for abandoned project-save staging files (`.hex-pending-*`, `.hex-r-*`).
//!
//! # Custody & platform limits
//! - Normal `save_new`, `replace_existing`, and `journal` operations NEVER
//!   automatically sweep or delete matching filenames: filename, mtime, and PID
//!   do not prove ownership and could destroy arbitrary user files or valid
//!   interrupted recovery copies.
//! - [`inspect_abandoned_stages`] classifies regular staging-named files in one
//!   directory read-only (skipping symbolic links and subdirectories) without
//!   mutating or deleting anything.
//! - [`recover_abandoned_stage_as`] decodes a recoverable project or recovery
//!   envelope stage and publishes it safely to a fresh destination via
//!   [`crate::save_new_with_outcome`], leaving the original stage intact.
//! - [`discard_verified_stage`] performs an explicit, user-authorized single-file
//!   removal only when the entry is still a regular file (never a symlink) whose
//!   bounded content matches the caller's exact inspected SHA-256 digest.
//!   **Platform limitation**: POSIX/macOS has no kernel-atomic
//!   `unlink-if-inode-and-content-unchanged` syscall; a concurrent same-user
//!   pathname replacement inside the final `lstat`/`unlink` syscall window cannot
//!   be prevented at the kernel boundary.

use crate::{SaveNewOutcome, StorageError};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Classification of a regular file matching hex's staging naming convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbandonedStageKind {
    /// Complete valid hex project document (e.g. an interrupted new save or
    /// replacement stage) that can be recovered with [`recover_abandoned_stage_as`].
    ProjectDocument,
    /// Complete valid hex recovery envelope containing a decodable backup
    /// project bound to `target_basename`.
    RecoveryBackup { target_basename: String },
    /// Arbitrary user file, truncated write, or unsupported format. Never
    /// assumed to be app-owned solely from its filename, mtime, or PID.
    Unrecognized,
}

/// Read-only inspection report for one regular file matching `.hex-pending-*`
/// or `.hex-r-*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbandonedStage {
    pub path: PathBuf,
    pub file_name: String,
    pub byte_len: u64,
    pub sha256: [u8; 32],
    pub kind: AbandonedStageKind,
}

fn is_stage_filename(name: &str) -> bool {
    name.starts_with(".hex-pending-") || name.starts_with(".hex-r-")
}

fn classify_bytes(bytes: &[u8]) -> AbandonedStageKind {
    if crate::decode(bytes).is_ok() {
        return AbandonedStageKind::ProjectDocument;
    }
    if let Ok((basename, payload)) = crate::replacement::parse_envelope(bytes)
        && crate::decode(payload).is_ok()
    {
        return AbandonedStageKind::RecoveryBackup {
            target_basename: basename.to_owned(),
        };
    }
    AbandonedStageKind::Unrecognized
}

/// Read-only, non-recursive inspection of regular files matching
/// `.hex-pending-*` or `.hex-r-*` inside `directory`.
///
/// Symbolic links, subdirectories, and non-regular entries are skipped without
/// following or removing them. Files exceeding the bounded project document
/// ceiling are skipped rather than loading unbounded memory.
pub fn inspect_abandoned_stages(
    directory: impl AsRef<Path>,
) -> Result<Vec<AbandonedStage>, StorageError> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory.as_ref())? {
        let entry = entry?;
        let os_name = entry.file_name();
        let Some(file_name) = os_name.to_str() else {
            continue;
        };
        if !is_stage_filename(file_name) {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let Ok(bytes) = crate::files::read_document(&path) else {
            continue;
        };
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let kind = classify_bytes(&bytes);
        found.push(AbandonedStage {
            path,
            file_name: file_name.to_owned(),
            byte_len: bytes.len() as u64,
            sha256: digest,
            kind,
        });
    }
    found.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    Ok(found)
}

/// Recover a valid abandoned project or recovery-backup stage into a fresh
/// `destination` path using [`crate::save_new_with_outcome`].
///
/// Never mutates or deletes `stage_path`, refuses symbolic links and
/// unrecognized files, and never overwrites an existing `destination`.
pub fn recover_abandoned_stage_as(
    stage_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<SaveNewOutcome, StorageError> {
    let stage_path = stage_path.as_ref();
    let metadata = fs::symlink_metadata(stage_path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StorageError::Invalid(
            "abandoned stage must be a regular file, not a symbolic link or directory",
        ));
    }
    let bytes = crate::files::read_document(stage_path)?;
    let project = match crate::decode(&bytes) {
        Ok(project) => project,
        Err(_) => {
            let (_, payload) = crate::replacement::parse_envelope(&bytes).map_err(|_| {
                StorageError::Invalid(
                    "staging file is not a recoverable hex project or recovery backup",
                )
            })?;
            crate::decode(payload)?
        }
    };
    crate::save_new_with_outcome(destination, &project)
}

/// Explicitly discard a single inspected staging file only when it is still a
/// regular file (never a symbolic link) whose exact bounded contents match
/// `expected_sha256`.
///
/// Returns [`StorageError::Conflict`] without deleting the file if its contents
/// changed since inspection. Does not rely on filename, mtime, or PID.
pub fn discard_verified_stage(
    stage_path: impl AsRef<Path>,
    expected_sha256: &[u8; 32],
) -> Result<(), StorageError> {
    let stage_path = stage_path.as_ref();
    let metadata = fs::symlink_metadata(stage_path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StorageError::Invalid(
            "refusing to discard non-regular or symbolic-link staging entry",
        ));
    }
    let bytes = crate::files::read_document(stage_path)?;
    let actual: [u8; 32] = Sha256::digest(&bytes).into();
    if &actual != expected_sha256 {
        return Err(StorageError::Conflict {
            path: stage_path.to_path_buf(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = fs::symlink_metadata(stage_path)?;
        if current.file_type().is_symlink()
            || !current.is_file()
            || (current.dev(), current.ino()) != (metadata.dev(), metadata.ino())
        {
            return Err(StorageError::Conflict {
                path: stage_path.to_path_buf(),
            });
        }
    }
    fs::remove_file(stage_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, SystemTime},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-temporaries-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("test directory");
            Self(path)
        }

        fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, contents).expect("fixture file");
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn save_and_replace_preserve_old_matching_user_files() {
        let dir = Dir::new();
        let target = dir.0.join("song.hex");
        let project = hex_project::Project::new(48_000).expect("project");
        let mut retained = Vec::new();
        for name in [".hex-pending-1-1", ".hex-r-1-2"] {
            let path = dir.file(name, b"copy");
            fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(SystemTime::now() - Duration::from_secs(7200)))
                .expect("age fixture");
            retained.push(path);
        }
        crate::save_new(&target, &project).expect("save new");
        for path in &retained {
            assert_eq!(fs::read(path).expect("preserve old user file"), b"copy");
        }

        let again = dir.file(".hex-r-1-3", b"copy");
        fs::File::options()
            .write(true)
            .open(&again)
            .and_then(|file| file.set_modified(SystemTime::now() - Duration::from_secs(7200)))
            .expect("age fixture");
        let (_, token) = crate::load_with_token(&target).expect("token");
        crate::replace_existing(&target, &token, &project).expect("replace");
        assert_eq!(
            fs::read(&again).expect("preserve another user file"),
            b"copy"
        );
        for path in &retained {
            assert_eq!(
                fs::read(path).expect("preserve across replacement"),
                b"copy"
            );
        }
        assert!(crate::load(&target).is_ok());
        assert!(crate::recovery_path(&target).expect("sidecar").exists());
    }

    #[cfg(unix)]
    #[test]
    fn substituted_active_replacement_stage_is_preserved_on_drop() {
        let dir = Dir::new();
        let temp = crate::replacement::prepare_temp(&dir.0, b"staged-project-bytes", None)
            .expect("prepare temp");
        let stage_path = temp.path.clone();

        // Simulate a concurrent noncooperating process replacing the staging
        // pathname with an unrelated user file before `Temporary::drop`.
        let foreign = dir.file("foreign.txt", b"precious-user-data");
        fs::rename(&foreign, &stage_path).expect("substitute staging path");
        drop(temp);

        assert_eq!(
            fs::read(&stage_path).expect("substituted file must survive Temporary::drop"),
            b"precious-user-data"
        );
    }

    #[test]
    fn inspect_and_recover_abandoned_stages_classify_and_preserve_originals() {
        let dir = Dir::new();
        let project = hex_project::Project::new(48_000).expect("project");
        let encoded = crate::encode(&project).expect("encode project");
        let envelope =
            crate::replacement::encode_envelope("song.hex", &encoded).expect("encode envelope");

        let pending_project = dir.file(".hex-pending-999-1", &encoded);
        let recovery_stage = dir.file(".hex-r-999-2", &envelope);
        let user_file = dir.file(".hex-pending-42-7", b"not a hex project; user notes");
        let normal_file = dir.file("notes.txt", b"ignored non-staging file");

        let stages = inspect_abandoned_stages(&dir.0).expect("inspect");
        assert_eq!(stages.len(), 3);

        let by_name = |name: &str| {
            stages
                .iter()
                .find(|stage| stage.file_name == name)
                .unwrap_or_else(|| panic!("missing {name}"))
        };
        assert_eq!(
            by_name(".hex-pending-999-1").kind,
            AbandonedStageKind::ProjectDocument
        );
        assert_eq!(
            by_name(".hex-r-999-2").kind,
            AbandonedStageKind::RecoveryBackup {
                target_basename: "song.hex".to_owned(),
            }
        );
        assert_eq!(
            by_name(".hex-pending-42-7").kind,
            AbandonedStageKind::Unrecognized
        );

        // Recover both valid stages to fresh destinations without modifying originals.
        let recovered_a = dir.0.join("recovered-a.hex");
        let recovered_b = dir.0.join("recovered-b.hex");
        recover_abandoned_stage_as(&pending_project, &recovered_a).expect("recover project stage");
        recover_abandoned_stage_as(&recovery_stage, &recovered_b).expect("recover envelope stage");
        assert_eq!(crate::load(&recovered_a).expect("load a"), project);
        assert_eq!(crate::load(&recovered_b).expect("load b"), project);

        // Unrecognized user file is refused by recovery and preserved untouched.
        assert!(recover_abandoned_stage_as(&user_file, dir.0.join("bad.hex")).is_err());
        assert_eq!(fs::read(&pending_project).expect("original a"), encoded);
        assert_eq!(fs::read(&recovery_stage).expect("original b"), envelope);
        assert_eq!(
            fs::read(&user_file).expect("user file preserved"),
            b"not a hex project; user notes"
        );
        assert_eq!(
            fs::read(&normal_file).expect("normal file preserved"),
            b"ignored non-staging file"
        );
    }

    #[test]
    fn discard_verified_stage_requires_exact_digest_and_refuses_symlinks_or_changed_files() {
        let dir = Dir::new();
        let project = hex_project::Project::new(48_000).expect("project");
        let encoded = crate::encode(&project).expect("encode");
        let stage = dir.file(".hex-pending-888-1", &encoded);

        let inspected = inspect_abandoned_stages(&dir.0).expect("inspect");
        assert_eq!(inspected.len(), 1);
        let digest = inspected[0].sha256;

        // Wrong digest must refuse and preserve the file.
        let wrong = [0_u8; 32];
        assert!(matches!(
            discard_verified_stage(&stage, &wrong),
            Err(StorageError::Conflict { .. })
        ));
        assert!(stage.exists());

        // Post-inspection modification must refuse and preserve the new content.
        fs::write(&stage, b"user replaced contents").expect("modify");
        assert!(matches!(
            discard_verified_stage(&stage, &digest),
            Err(StorageError::Conflict { .. })
        ));
        assert_eq!(
            fs::read(&stage).expect("preserved"),
            b"user replaced contents"
        );

        // Symbolic link must always be refused and target preserved.
        #[cfg(unix)]
        {
            let target = dir.file("target.txt", b"do not delete");
            let link = dir.0.join(".hex-pending-888-2");
            std::os::unix::fs::symlink(&target, &link).expect("symlink");
            let target_digest: [u8; 32] = Sha256::digest(b"do not delete").into();
            assert!(matches!(
                discard_verified_stage(&link, &target_digest),
                Err(StorageError::Invalid(_))
            ));
            assert!(target.exists());
        }

        // Exact inspected digest of unchanged file succeeds.
        let stage2 = dir.file(".hex-r-888-3", &encoded);
        let inspected2 = inspect_abandoned_stages(&dir.0).expect("inspect 2");
        let entry2 = inspected2
            .iter()
            .find(|item| item.file_name == ".hex-r-888-3")
            .expect("entry2");
        discard_verified_stage(&stage2, &entry2.sha256).expect("verified discard");
        assert!(!stage2.exists());
    }
}
