//! Optimistic existing-project replacement with one validated recovery generation.
use crate::{FileToken, StorageError, decode, encode, files::read_document};
use hex_project::Project;
use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

const RECOVERY_MAGIC: &[u8; 8] = b"HEXRCVY\0";
const RECOVERY_VERSION: u32 = 1;
const RECOVERY_HEADER_BYTES: usize = 24;
const MAX_COMPONENT_BYTES: usize = 255;
const RECOVERY_SUFFIX: &str = ".hex-recovery";
const MAX_TEMP_ATTEMPTS: usize = 32;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Validated previous contents available for explicit recovery.
#[derive(Debug)]
pub struct RecoveryDocument {
    pub target: PathBuf,
    pub recovery_path: PathBuf,
    pub project: Project,
    pub token: FileToken,
}

/// A durability warning after the replacement is already visible at `path`.
#[derive(Debug)]
pub struct ReplaceWarning {
    pub operation: &'static str,
    pub source: io::Error,
}

/// Successful replacement state, including the token for the newly published file.
#[derive(Debug)]
pub struct ReplaceOutcome {
    pub path: PathBuf,
    pub recovery_path: PathBuf,
    pub token: FileToken,
    pub warning: Option<ReplaceWarning>,
}

/// Return the deterministic hidden recovery sidecar path for a UTF-8 target name.
pub fn recovery_path(target: impl AsRef<Path>) -> Result<PathBuf, StorageError> {
    let target = target.as_ref();
    let basename = target_basename(target)?;
    let name = format!(".{basename}{RECOVERY_SUFFIX}");
    if name.len() > MAX_COMPONENT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "recovery filename exceeds the filesystem component limit",
        )
        .into());
    }
    Ok(target_parent(target).join(name))
}

/// Validate and read the previous project stored beside `target`.
pub fn load_recovery(target: impl AsRef<Path>) -> Result<RecoveryDocument, StorageError> {
    let target = target.as_ref();
    let basename = target_basename(target)?;
    let recovery = recovery_path(target)?;
    let bytes = read_recovery_file(&recovery)?;
    let (project, token) = decode_envelope(&bytes, &basename)?;
    Ok(RecoveryDocument {
        target: target.to_path_buf(),
        recovery_path: recovery,
        project,
        token,
    })
}

/// Replace an existing regular project only if its exact bytes still match `expected`.
///
/// The old bytes are copied into a validated, target-bound sidecar before the target
/// is atomically renamed. This narrows accidental-overwrite risk but is not a
/// filesystem compare-and-swap: an uncooperative writer can race the final check.
pub fn replace_existing(
    target: impl AsRef<Path>,
    expected: &FileToken,
    project: &Project,
) -> Result<ReplaceOutcome, StorageError> {
    let target = target.as_ref();
    replace_with_hooks(target, expected, project, sync_parent, sync_parent, || {
        Ok(())
    })
}

fn replace_with_hooks(
    target: &Path,
    expected: &FileToken,
    project: &Project,
    sync_recovery_directory: impl FnOnce(&Path) -> io::Result<()>,
    sync_project_directory: impl FnOnce(&Path) -> io::Result<()>,
    before_project_rename: impl FnOnce() -> io::Result<()>,
) -> Result<ReplaceOutcome, StorageError> {
    let basename = target_basename(target)?;
    let recovery = recovery_path(target)?;
    let target_permissions = check_target(target, expected)?;
    let parent = target_parent(target);
    let old_recovery = match inspect_recovery(&recovery, &basename)? {
        SidecarState::Absent => None,
        SidecarState::Owned(bytes) => Some(bytes),
        // A case-only or Unicode-normalization rename makes this path resolve to
        // the sidecar of the old name. Preserve it and start a fresh generation.
        SidecarState::Foreign { stored_basename } => {
            set_aside_recovery(&recovery, &parent, &stored_basename)?;
            None
        }
    };
    let new_bytes = encode(project)?;
    let envelope = encode_envelope(&basename, &expected.0)?;

    let mut project_temp = prepare_temp(&parent, &new_bytes, Some(target_permissions.clone()))?;
    let mut recovery_temp = prepare_temp(&parent, &envelope, Some(target_permissions.clone()))?;

    // Refresh both paths before committing the recovery copy. A valid prior
    // generation may be rolled; anything unrecognized remains untouched.
    check_target_permissions(target, expected, &target_permissions)?;
    let current_recovery = match inspect_recovery(&recovery, &basename)? {
        SidecarState::Absent => None,
        SidecarState::Owned(bytes) => Some(bytes),
        SidecarState::Foreign { .. } => {
            return Err(StorageError::Invalid(
                "recovery sidecar changed during project replacement",
            ));
        }
    };
    if current_recovery.as_deref() != old_recovery.as_deref() {
        return Err(StorageError::Invalid(
            "recovery sidecar changed during project replacement",
        ));
    }
    publish_recovery(
        &mut recovery_temp,
        &recovery,
        old_recovery.is_some(),
        target,
    )?;
    sync_recovery_directory(&parent).map_err(|source| StorageError::RecoveryBackupPublished {
        path: target.to_path_buf(),
        recovery_path: recovery.clone(),
        operation: "synchronize the recovery directory",
        source,
    })?;

    // Recheck immediately before the target rename. This is optimistic validation,
    // not a lock against a noncooperating writer in the final syscall window.
    check_target_permissions(target, expected, &target_permissions)?;
    before_project_rename().map_err(|source| StorageError::RecoveryBackupPublished {
        path: target.to_path_buf(),
        recovery_path: recovery.clone(),
        operation: "prepare the replacement project",
        source,
    })?;
    fs::rename(&project_temp.path, target).map_err(|source| {
        StorageError::RecoveryBackupPublished {
            path: target.to_path_buf(),
            recovery_path: recovery.clone(),
            operation: "publish the replacement project",
            source,
        }
    })?;
    project_temp.disarm();

    let token = FileToken::from_validated_bytes(Arc::from(new_bytes));
    let warning = sync_project_directory(&parent)
        .err()
        .map(|source| ReplaceWarning {
            operation: "synchronize the project directory",
            source,
        });
    Ok(ReplaceOutcome {
        path: target.to_path_buf(),
        recovery_path: recovery,
        token,
        warning,
    })
}

fn target_parent(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn target_basename(path: &Path) -> Result<String, StorageError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or(StorageError::Invalid(
            "project target filename must be utf-8",
        ))
}

fn check_target(path: &Path, expected: &FileToken) -> Result<Permissions, StorageError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(StorageError::Conflict {
                path: path.to_path_buf(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(StorageError::Invalid(
            "project replacement refuses symbolic-link targets",
        ));
    }
    if !metadata.is_file() {
        return Err(StorageError::Invalid(
            "project replacement target must be a regular file",
        ));
    }
    if read_document(path)?.as_slice() != expected.0.as_ref() {
        return Err(StorageError::Conflict {
            path: path.to_path_buf(),
        });
    }
    Ok(metadata.permissions())
}

fn check_target_permissions(
    path: &Path,
    expected: &FileToken,
    initial: &Permissions,
) -> Result<(), StorageError> {
    let current = check_target(path, expected)?;
    if !same_basic_permissions(&current, initial) {
        return Err(StorageError::Invalid(
            "project permissions changed during replacement",
        ));
    }
    Ok(())
}

fn same_basic_permissions(left: &Permissions, right: &Permissions) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        left.mode() & 0o7777 == right.mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        left.readonly() == right.readonly()
    }
}

/// What occupies a project's recovery sidecar path.
enum SidecarState {
    Absent,
    /// A valid envelope bound to this exact target name.
    Owned(Vec<u8>),
    /// A valid envelope bound to another target name, such as the pre-rename
    /// name after a case-only rename on a case-insensitive volume.
    Foreign {
        stored_basename: String,
    },
}

fn inspect_recovery(path: &Path, basename: &str) -> Result<SidecarState, StorageError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(SidecarState::Absent),
        Err(error) => Err(error.into()),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(StorageError::Invalid(
                    "recovery sidecar is a symbolic link; refusing replacement",
                ));
            }
            if !metadata.is_file() {
                return Err(StorageError::Invalid(
                    "recovery sidecar is not a regular file; refusing replacement",
                ));
            }
            let bytes = read_document(path)?;
            let (stored_basename, payload) = parse_envelope(&bytes)?;
            // Validate the payload either way: unrecognized files stay untouched.
            let _ = decode(payload)?;
            if stored_basename == basename {
                Ok(SidecarState::Owned(bytes))
            } else {
                Ok(SidecarState::Foreign {
                    stored_basename: stored_basename.to_owned(),
                })
            }
        }
    }
}

/// Move a validated foreign sidecar to an unused `.moved-N` name, never replacing
/// an existing file, so its previous project stays available for manual recovery.
fn set_aside_recovery(
    recovery: &Path,
    parent: &Path,
    stored_basename: &str,
) -> Result<(), StorageError> {
    for attempt in 1..=MAX_TEMP_ATTEMPTS {
        let name = format!(".{stored_basename}{RECOVERY_SUFFIX}.moved-{attempt}");
        if name.len() > MAX_COMPONENT_BYTES {
            return Err(StorageError::Invalid(
                "recovery sidecar of a renamed project cannot be set aside; use save as",
            ));
        }
        let destination = parent.join(name);
        match fs::hard_link(recovery, &destination) {
            Ok(()) => {
                fs::remove_file(recovery)?;
                sync_parent(parent)?;
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(StorageError::Invalid(
        "no free name to set aside the recovery sidecar of a renamed project; use save as",
    ))
}

fn read_recovery_file(path: &Path) -> Result<Vec<u8>, StorageError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StorageError::Invalid(
            "recovery sidecar must be a regular non-symbolic file",
        ));
    }
    read_document(path)
}

pub(crate) fn encode_envelope(basename: &str, payload: &[u8]) -> Result<Vec<u8>, StorageError> {
    let basename_bytes = basename.as_bytes();
    if basename_bytes.len() > MAX_COMPONENT_BYTES {
        return Err(StorageError::Invalid(
            "project target filename exceeds filesystem component limit",
        ));
    }
    let capacity = RECOVERY_HEADER_BYTES
        .checked_add(basename_bytes.len())
        .and_then(|size| size.checked_add(payload.len()))
        .ok_or(StorageError::Invalid("recovery envelope size overflow"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| StorageError::Invalid("could not reserve recovery envelope memory"))?;
    bytes.extend_from_slice(RECOVERY_MAGIC);
    bytes.extend_from_slice(&RECOVERY_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(basename_bytes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(basename_bytes);
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

pub(crate) fn decode_envelope(
    bytes: &[u8],
    expected_basename: &str,
) -> Result<(Project, FileToken), StorageError> {
    let (basename, payload) = parse_envelope(bytes)?;
    if basename != expected_basename {
        return Err(StorageError::Invalid(
            "recovery sidecar belongs to a different project target; use save as",
        ));
    }
    let project = decode(payload)?;
    Ok((project, FileToken::from_validated_bytes(Arc::from(payload))))
}

/// Validate envelope framing and return its bound target name and payload.
pub(crate) fn parse_envelope(bytes: &[u8]) -> Result<(&str, &[u8]), StorageError> {
    if bytes.len() < RECOVERY_HEADER_BYTES {
        return Err(StorageError::Invalid("invalid recovery envelope length"));
    }
    if &bytes[..8] != RECOVERY_MAGIC {
        return Err(StorageError::Invalid("unrecognized recovery sidecar"));
    }
    let version = u32::from_le_bytes(
        bytes[8..12]
            .try_into()
            .map_err(|_| StorageError::Invalid("invalid recovery envelope version"))?,
    );
    if version != RECOVERY_VERSION {
        return Err(StorageError::Invalid(
            "unsupported recovery envelope version",
        ));
    }
    let basename_length = u32::from_le_bytes(
        bytes[12..16]
            .try_into()
            .map_err(|_| StorageError::Invalid("invalid recovery target name length"))?,
    ) as usize;
    let payload_length = u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| StorageError::Invalid("invalid recovery payload length"))?,
    );
    if basename_length == 0 || basename_length > MAX_COMPONENT_BYTES {
        return Err(StorageError::Invalid("invalid recovery target name length"));
    }
    let payload_length = usize::try_from(payload_length)
        .map_err(|_| StorageError::Invalid("recovery payload length overflow"))?;
    let expected_length = RECOVERY_HEADER_BYTES
        .checked_add(basename_length)
        .and_then(|length| length.checked_add(payload_length))
        .ok_or(StorageError::Invalid("recovery envelope length overflow"))?;
    if expected_length != bytes.len() {
        return Err(StorageError::Invalid(
            "recovery envelope length does not match its payload",
        ));
    }
    let basename_end = RECOVERY_HEADER_BYTES + basename_length;
    let basename = std::str::from_utf8(&bytes[RECOVERY_HEADER_BYTES..basename_end])
        .map_err(|_| StorageError::Invalid("recovery target name is not utf-8"))?;
    Ok((basename, &bytes[basename_end..]))
}

pub(crate) struct Temporary {
    pub(crate) path: PathBuf,
    armed: bool,
}

impl Temporary {
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn prepare_temp(
    parent: &Path,
    bytes: &[u8],
    permissions: Option<Permissions>,
) -> Result<Temporary, StorageError> {
    let mut created = None;
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".hex-r-{}-{id}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => {
                created = Some((Temporary { path, armed: true }, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let (temporary, mut file) = created.ok_or(StorageError::Invalid(
        "could not reserve short recovery temporary filename",
    ))?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    Ok(temporary)
}

fn publish_recovery(
    temporary: &mut Temporary,
    recovery: &Path,
    replacing_owned: bool,
    target: &Path,
) -> Result<(), StorageError> {
    if replacing_owned {
        // The caller validated this exact owned sidecar immediately before this
        // atomic rename. A remaining race with external writers is not CAS.
        fs::rename(&temporary.path, recovery)?;
        temporary.disarm();
    } else {
        match fs::symlink_metadata(recovery) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(StorageError::Invalid(
                    "recovery sidecar appeared during project replacement",
                ));
            }
            Err(error) => return Err(error.into()),
        }
        fs::hard_link(&temporary.path, recovery)?;
        if let Err(source) = fs::remove_file(&temporary.path) {
            return Err(StorageError::RecoveryBackupPublished {
                path: target.to_path_buf(),
                recovery_path: recovery.to_path_buf(),
                operation: "remove the recovery temporary file",
                source,
            });
        }
        temporary.disarm();
    }
    Ok(())
}

pub(crate) fn sync_parent(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(parent)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load, load_with_token, save_new};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("hex-replace-test-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("create unique test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(rate: u32) -> Project {
        Project::new(rate).expect("project")
    }

    #[test]
    fn replacement_publishes_exact_recovery_and_returns_new_token() {
        let directory = TestDir::new();
        let target = directory.0.join("MiXeD Project.hex");
        let previous = project(48_000);
        let replacement = project(44_100);
        save_new(&target, &previous).expect("initial project");
        let (_, token) = load_with_token(&target).expect("capture token");

        let outcome = replace_existing(&target, &token, &replacement).expect("replace project");
        assert_eq!(outcome.path, target);
        assert_eq!(
            outcome.recovery_path,
            recovery_path(&target).expect("sidecar path")
        );
        assert!(outcome.warning.is_none());
        assert_eq!(load(&target).expect("load replacement"), replacement);
        outcome
            .token
            .check_unchanged(&target)
            .expect("new token matches published bytes");

        let recovered = load_recovery(&target).expect("validated old project");
        assert_eq!(recovered.target, target);
        assert_eq!(recovered.project, previous);
        assert_eq!(recovered.token.0.as_ref(), token.0.as_ref());
        assert_eq!(recovered.recovery_path, outcome.recovery_path);
        assert_eq!(fs::read_dir(&directory.0).expect("list files").count(), 2);
    }

    #[test]
    fn repeated_replacement_retains_only_the_most_recent_prior_generation() {
        let directory = TestDir::new();
        let target = directory.0.join("roll.hex");
        let first = project(48_000);
        let second = project(44_100);
        let third = project(96_000);
        save_new(&target, &first).expect("initial project");
        let (_, first_token) = load_with_token(&target).expect("first token");
        let second_outcome = replace_existing(&target, &first_token, &second).expect("first roll");
        let third_outcome =
            replace_existing(&target, &second_outcome.token, &third).expect("second roll");

        assert_eq!(load(&target).expect("latest project"), third);
        let recovered = load_recovery(&target).expect("latest prior generation");
        assert_eq!(recovered.project, second);
        assert_eq!(recovered.token.0.as_ref(), second_outcome.token.0.as_ref());
        third_outcome
            .token
            .check_unchanged(&target)
            .expect("latest token");
        assert_eq!(fs::read_dir(&directory.0).expect("list files").count(), 2);
    }

    #[test]
    fn formatting_change_conflicts_without_touching_target_or_creating_recovery() {
        let directory = TestDir::new();
        let target = directory.0.join("conflict.hex");
        let original = project(48_000);
        save_new(&target, &original).expect("initial project");
        let (_, token) = load_with_token(&target).expect("token");
        let mut changed = fs::read(&target).expect("read original");
        changed.push(b'\n');
        fs::write(&target, &changed).expect("external formatting edit");

        assert!(matches!(
            replace_existing(&target, &token, &project(44_100)),
            Err(StorageError::Conflict { path }) if path == target
        ));
        assert_eq!(
            fs::read(&target).expect("unchanged external content"),
            changed
        );
        assert!(!recovery_path(&target).expect("sidecar path").exists());
        assert_eq!(fs::read_dir(&directory.0).expect("list files").count(), 1);
    }

    #[test]
    fn unknown_or_malformed_recovery_files_are_preserved_byte_for_byte() {
        let directory = TestDir::new();
        let target = directory.0.join("owned.hex");
        let sidecar = recovery_path(&target).expect("sidecar path");
        let original = project(48_000);
        save_new(&target, &original).expect("initial project");
        let (_, token) = load_with_token(&target).expect("token");
        for bytes in [
            b"unrelated contents".to_vec(),
            vec![0_u8; RECOVERY_HEADER_BYTES],
            {
                let mut future = encode_envelope("owned.hex", &token.0).expect("envelope");
                future[8..12].copy_from_slice(&2_u32.to_le_bytes());
                future
            },
            {
                let mut overflow = encode_envelope("owned.hex", &token.0).expect("envelope");
                overflow[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
                overflow
            },
            {
                let mut truncated = encode_envelope("owned.hex", &token.0).expect("envelope");
                truncated.pop();
                truncated
            },
        ] {
            fs::write(&sidecar, &bytes).expect("unrecognized sidecar");
            assert!(replace_existing(&target, &token, &project(44_100)).is_err());
            assert_eq!(fs::read(&target).expect("old target"), token.0.as_ref());
            assert_eq!(fs::read(&sidecar).expect("preserved sidecar"), bytes);
        }
        fs::remove_file(&sidecar).expect("remove own fixture");
        fs::create_dir(&sidecar).expect("directory at sidecar path");
        assert!(replace_existing(&target, &token, &project(44_100)).is_err());
        assert!(sidecar.is_dir());
        assert_eq!(fs::read(&target).expect("old target"), token.0.as_ref());
        fs::remove_dir(&sidecar).expect("remove own directory fixture");
        fs::write(&target, vec![b' '; crate::LEGACY_READER_DOCUMENT_BYTES + 1])
            .expect("oversized current file");
        assert!(matches!(
            replace_existing(&target, &token, &project(44_100)),
            Err(StorageError::Conflict { .. })
        ));
        assert!(!sidecar.exists());
    }

    #[test]
    fn foreign_bound_sidecar_is_set_aside_and_long_sidecar_name_is_refused() {
        let directory = TestDir::new();
        let target = directory.0.join("target.hex");
        let original = project(48_000);
        save_new(&target, &original).expect("initial project");
        let (_, token) = load_with_token(&target).expect("token");
        let envelope = encode_envelope("other.hex", &token.0).expect("valid envelope");
        let sidecar = recovery_path(&target).expect("sidecar path");
        fs::write(&sidecar, &envelope).expect("foreign recovery fixture");
        let replacement = project(44_100);
        replace_existing(&target, &token, &replacement).expect("foreign sidecar set aside");
        assert_eq!(load(&target).expect("replaced target"), replacement);
        assert_eq!(
            load_recovery(&target).expect("fresh generation").project,
            original
        );
        let moved = directory.0.join(".other.hex.hex-recovery.moved-1");
        assert_eq!(
            fs::read(&moved).expect("preserved foreign sidecar"),
            envelope
        );
        let (_, token) = load_with_token(&target).expect("token after replacement");
        let original_bytes = token.0.to_vec();

        let too_long = directory.0.join(format!("{}.hex", "x".repeat(245)));
        assert!(matches!(
            recovery_path(&too_long),
            Err(StorageError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput
        ));
        assert_eq!(fs::read(&target).expect("target unchanged"), original_bytes);
    }

    /// True when `renamed` resolves to the existing file `original` in `directory`.
    #[cfg(unix)]
    fn names_fold(directory: &Path, original: &str, renamed: &str) -> bool {
        use std::os::unix::fs::MetadataExt;
        let (Ok(left), Ok(right)) = (
            fs::metadata(directory.join(original)),
            fs::metadata(directory.join(renamed)),
        ) else {
            return false;
        };
        left.ino() == right.ino()
    }

    #[cfg(unix)]
    fn rename_by_equivalent_name_keeps_saving(original: &str, renamed: &str) {
        let directory = TestDir::new();
        let old_target = directory.0.join(original);
        let first = project(48_000);
        let second = project(44_100);
        save_new(&old_target, &first).expect("initial project");
        let (_, token) = load_with_token(&old_target).expect("first token");
        replace_existing(&old_target, &token, &second).expect("sidecar bound to old name");
        if !names_fold(&directory.0, original, renamed) {
            eprintln!("skipped: temp volume does not fold {original:?} and {renamed:?}");
            return;
        }
        let old_sidecar = fs::read(recovery_path(&old_target).expect("old sidecar path"))
            .expect("old sidecar bytes");

        let new_target = directory.0.join(renamed);
        fs::rename(&old_target, &new_target).expect("equivalent-name rename");
        let (_, token) = load_with_token(&new_target).expect("token after rename");
        let third = project(96_000);
        replace_existing(&new_target, &token, &third).expect("save after rename");

        assert_eq!(load(&new_target).expect("saved project"), third);
        assert_eq!(
            load_recovery(&new_target).expect("fresh sidecar").project,
            second
        );
        let moved = directory
            .0
            .join(format!(".{original}{RECOVERY_SUFFIX}.moved-1"));
        assert_eq!(fs::read(moved).expect("old sidecar preserved"), old_sidecar);
        assert_eq!(fs::read_dir(&directory.0).expect("list files").count(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn case_only_rename_sets_aside_old_sidecar_and_saves() {
        rename_by_equivalent_name_keeps_saving("song.hex", "Song.hex");
    }

    #[cfg(unix)]
    #[test]
    fn normalization_only_rename_sets_aside_old_sidecar_and_saves() {
        rename_by_equivalent_name_keeps_saving("caf\u{e9}.hex", "cafe\u{301}.hex");
    }

    #[test]
    fn injected_precommit_failure_leaves_old_target_and_published_recovery() {
        let directory = TestDir::new();
        let target = directory.0.join("precommit.hex");
        let previous = project(48_000);
        save_new(&target, &previous).expect("initial project");
        let (_, token) = load_with_token(&target).expect("token");
        let error = replace_with_hooks(
            &target,
            &token,
            &project(44_100),
            |_| Ok(()),
            |_| Ok(()),
            || Err(io::Error::other("injected precommit failure")),
        )
        .expect_err("injected failure");
        assert!(matches!(
            &error,
            StorageError::RecoveryBackupPublished { path, .. } if path == &target
        ));
        assert!(error.to_string().contains("recovery copy"));
        assert_eq!(load(&target).expect("old target remains"), previous);
        assert_eq!(
            load_recovery(&target).expect("duplicate recovery").project,
            previous
        );
    }

    #[test]
    fn recovery_directory_sync_failure_preserves_original_and_allows_retry() {
        let directory = TestDir::new();
        let target = directory.0.join("recovery-sync.hex");
        let sidecar = recovery_path(&target).expect("sidecar path");
        let previous = project(48_000);
        let replacement = project(44_100);
        save_new(&target, &previous).expect("initial project");
        let (_, token) = load_with_token(&target).expect("original token");
        let original_bytes = fs::read(&target).expect("original bytes");

        let error = replace_with_hooks(
            &target,
            &token,
            &replacement,
            |parent| {
                assert_eq!(parent, directory.0);
                Err(io::Error::other("injected recovery sync failure"))
            },
            |_| panic!("project directory sync must not run before commit"),
            || panic!("project rename must not run after recovery sync failure"),
        )
        .expect_err("recovery directory sync fails");
        assert!(matches!(
            error,
            StorageError::RecoveryBackupPublished {
                path, recovery_path, operation: "synchronize the recovery directory", source,
            } if path == target && recovery_path == sidecar
                && source.to_string() == "injected recovery sync failure"
        ));
        assert_eq!(fs::read(&target).expect("original file"), original_bytes);
        token
            .check_unchanged(&target)
            .expect("original token remains valid");
        let recovered = load_recovery(&target).expect("published recovery is valid");
        assert_eq!(recovered.project, previous);
        assert_eq!(recovered.token.0.as_ref(), original_bytes);
        let recovery_bytes = fs::read(&sidecar).expect("published recovery bytes");
        let mut paths: Vec<_> = fs::read_dir(&directory.0)
            .expect("list files after failure")
            .map(|entry| entry.expect("directory entry").path())
            .collect();
        paths.sort();
        let mut expected_paths = vec![target.clone(), sidecar.clone()];
        expected_paths.sort();
        assert_eq!(paths, expected_paths, "temporary files are removed");

        let outcome = replace_existing(&target, &token, &replacement).expect("normal retry");
        assert!(outcome.warning.is_none());
        assert_eq!(load(&target).expect("replacement after retry"), replacement);
        outcome.token.check_unchanged(&target).expect("new token");
        assert_eq!(
            fs::read(&sidecar).expect("recovery after retry"),
            recovery_bytes
        );
        assert_eq!(
            load_recovery(&target)
                .expect("recovery after retry")
                .project,
            previous
        );
        assert_eq!(fs::read_dir(&directory.0).expect("list files").count(), 2);
    }

    #[test]
    fn postcommit_sync_failure_returns_adoptable_token_and_recovery_path() {
        let directory = TestDir::new();
        let target = directory.0.join("postcommit.hex");
        let previous = project(48_000);
        let replacement = project(44_100);
        save_new(&target, &previous).expect("initial project");
        let (_, token) = load_with_token(&target).expect("token");
        let outcome = replace_with_hooks(
            &target,
            &token,
            &replacement,
            |_| Ok(()),
            |_| Err(io::Error::other("injected directory sync failure")),
            || Ok(()),
        )
        .expect("commit returns an outcome with a warning");

        assert_eq!(load(&target).expect("published project"), replacement);
        outcome
            .token
            .check_unchanged(&target)
            .expect("new token adopts committed file");
        assert_eq!(
            outcome.recovery_path,
            recovery_path(&target).expect("recovery path")
        );
        let warning = outcome.warning.expect("postcommit warning");
        assert_eq!(warning.operation, "synchronize the project directory");
        assert_eq!(
            warning.source.to_string(),
            "injected directory sync failure"
        );
        assert_eq!(
            load_recovery(&target).expect("previous project").project,
            previous
        );
    }

    #[cfg(unix)]
    #[test]
    fn target_and_sidecar_symlinks_are_refused_without_following_or_replacing() {
        let directory = TestDir::new();
        let actual = directory.0.join("actual.hex");
        let target_link = directory.0.join("target-link.hex");
        let original = project(48_000);
        save_new(&actual, &original).expect("actual project");
        std::os::unix::fs::symlink(&actual, &target_link).expect("target symlink");
        let (_, token) = load_with_token(&target_link).expect("token from linked load");
        assert!(matches!(
            replace_existing(&target_link, &token, &project(44_100)),
            Err(StorageError::Invalid(
                "project replacement refuses symbolic-link targets"
            ))
        ));
        assert_eq!(fs::read_link(&target_link).expect("link stays"), actual);
        assert_eq!(load(&actual).expect("real target unchanged"), original);

        let normal = directory.0.join("normal.hex");
        save_new(&normal, &original).expect("normal project");
        let (_, normal_token) = load_with_token(&normal).expect("normal token");
        let sidecar = recovery_path(&normal).expect("recovery path");
        std::os::unix::fs::symlink("external", &sidecar).expect("sidecar symlink");
        assert!(matches!(
            replace_existing(&normal, &normal_token, &project(44_100)),
            Err(StorageError::Invalid(reason)) if reason.contains("recovery sidecar is a symbolic link")
        ));
        assert_eq!(
            fs::read_link(&sidecar).expect("sidecar link stays"),
            Path::new("external")
        );
        assert_eq!(load(&normal).expect("normal project unchanged"), original);
    }

    #[cfg(unix)]
    #[test]
    fn replacement_and_recovery_preserve_existing_basic_mode() {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let directory = TestDir::new();
        let target = directory.0.join("private.hex");
        let previous = project(48_000);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)
            .expect("create private project");
        file.write_all(&encode(&previous).expect("encode project"))
            .expect("write project");
        file.sync_all().expect("sync project");
        drop(file);
        let (_, token) = load_with_token(&target).expect("private token");

        let outcome = replace_existing(&target, &token, &project(44_100)).expect("replace");
        let target_mode = fs::metadata(&target).expect("target metadata").mode() & 0o777;
        let recovery_mode = fs::metadata(&outcome.recovery_path)
            .expect("recovery metadata")
            .mode()
            & 0o777;
        assert_eq!(target_mode, 0o600);
        assert_eq!(recovery_mode, 0o600);
    }
}
