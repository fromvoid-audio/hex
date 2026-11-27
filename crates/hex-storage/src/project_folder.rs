//! Filesystem layout and publication for desktop project folders. The marker
//! distinguishes a project root from a general directory of standalone sets.
//! Project documents keep their existing encoding and relative asset paths.
//!
//! # Staging directory custody and platform boundaries (`#7972` / `#7683`)
//!
//! On macOS and Linux, [`Destination::prepare`] pins the parent directory with
//! `O_RDONLY | O_DIRECTORY | O_NOFOLLOW`, reserves a fresh sibling staging
//! directory via `mkdirat(parent_fd, stage_name, 0700)`, immediately opens that
//! entry with `openat(parent_fd, stage_name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)`,
//! verifies via the opened descriptor that the directory is completely empty,
//! and creates `.hex-project` (`O_CREAT | O_EXCL | O_NOFOLLOW`) and `samples/`
//! strictly through that pinned directory descriptor while recording `(dev, ino)`.
//!
//! ## Verified platform limitation (macOS / POSIX)
//!
//! Neither macOS Darwin/XNU nor POSIX provides a single-syscall atomic
//! `mkdir`-and-return-descriptor primitive:
//! - `mkdir(2)` / `mkdirat(2)` return status `0`, not a file descriptor.
//! - `open(2)` / `openat(2)` with `O_CREAT | O_DIRECTORY` fail with `EINVAL` in
//!   XNU `bsd/vfs/vfs_vnops.c`, and `O_CREAT | O_EXCL` with `S_IFDIR` creates a
//!   regular file rather than a directory.
//! - Private `openbyid_np` requires a preexisting object ID that `mkdirat` does
//!   not return; advisory `flock(2)` and user `UF_IMMUTABLE` flags do not block
//!   a non-cooperating same-user process from renaming entries.
//!
//! Consequently, `mkdirat` followed by `openat` plus an emptiness check is
//! **not atomic** against a same-user process replacing the empty staging
//! directory with another *empty* directory in that window; this module does
//! not claim check-then-open or pre-rename `statat` is atomic. What this
//! boundary *does* guarantee:
//! - Symlinks, regular files, and non-empty unrelated directories substituted
//!   between `mkdirat` and `openat` are never acquired or modified.
//! - Initialization of `.hex-project` and `samples/`, generated sample
//!   collection via [`NewFolder::collect_sample`], and project document staging
//!   during [`NewFolder::publish`] occur strictly through the pinned `stage_fd`,
//!   never by re-traversing staging pathnames or following symbolic links.
//! - Before sample collection and before final folder publication,
//!   `statat(parent_fd, stage_name, AT_SYMLINK_NOFOLLOW)` verifies that the
//!   staging directory entry still matches the recorded `(dev, ino)` identity.
//!   Because macOS APFS rejects `renameat(stage_fd, ".", ...)` with `EINVAL`,
//!   no kernel-atomic fd-to-directory rename exists on macOS; `statat` plus
//!   `renameat_with(RENAME_EXCL)` narrows substitution without claiming
//!   atomicity across the final syscall window.
//! - Any failure or cancellation preserves the staging path untouched (never
//!   running `remove_dir_all`) and reports it via [`StageRetained`].
//! - Normal Save As with collected generated media (`samples/`) is preserved.
//! - On platforms without `openat`/`mkdirat` (e.g., Windows), staging falls
//!   back to path-based creation and [`publication::rename_directory_exclusive`].

use crate::{ReplaceWarning, SaveNewOutcome, StorageError, files, publication, replacement};
use hex_project::Project;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::fs::OpenOptions;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::fd::OwnedFd;
use std::{
    fmt,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering::Relaxed},
};

const MARKER: &str = ".hex-project";
pub const SAMPLES: &str = "samples";
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct StageRetained {
    path: PathBuf,
    source: StorageError,
}

impl fmt::Display for StageRetained {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} · staging path {} was not removed by hex · inspect it before using or removing it",
            self.source,
            crate::text::verbatim(self.path.display())
        )
    }
}

impl std::error::Error for StageRetained {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn stage_retained(path: PathBuf, source: StorageError) -> StorageError {
    StorageError::Io(io::Error::other(StageRetained { path, source }))
}

/// Return the marked parent of a project document, if it has one.
pub fn root_for_document(path: &Path) -> io::Result<Option<&Path>> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    Ok(is_root(parent)?.then_some(parent))
}

pub fn is_root(root: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(root.join(MARKER)) {
        Ok(meta) if meta.is_file() && meta.len() == 0 => Ok(true),
        Ok(_) => Err(io::Error::other("project folder marker is invalid")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub enum Destination {
    File(PathBuf),
    Folder(NewFolder),
}

impl Destination {
    /// An exact existing file is the path the desktop confirmed replacing.
    /// All other names outside a marked folder reserve a new project folder.
    pub fn prepare(chosen: PathBuf, replace_existing: bool) -> Result<Self, StorageError> {
        Self::prepare_with_hook(chosen, replace_existing, |_| {})
    }

    fn prepare_with_hook(
        chosen: PathBuf,
        replace_existing: bool,
        after_reserve: impl FnOnce(&Path),
    ) -> Result<Self, StorageError> {
        if replace_existing || root_for_document(&chosen)?.is_some() {
            return Ok(Self::File(chosen));
        }
        match fs::symlink_metadata(&chosen) {
            Ok(_) => return Err(StorageError::Invalid("project destination already exists")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let parent = chosen.parent().ok_or(StorageError::Invalid(
            "project target needs a parent directory",
        ))?;
        let name = chosen
            .file_name()
            .ok_or(StorageError::Invalid("project target needs a filename"))?;
        let stem = chosen
            .file_stem()
            .ok_or(StorageError::Invalid("project target needs a name"))?;
        let root = parent.join(stem);
        match fs::symlink_metadata(&root) {
            Ok(_) => {
                return Err(StorageError::Invalid(
                    "project folder name already exists; open the folder in the save dialog or choose a new name",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let path = root.join(name);
        let path_bytes = path.as_os_str().len();
        if path_bytes > crate::project_name::MAX_PROJECT_PATH_BYTES {
            return Err(StorageError::Invalid(
                "project folder path exceeds the 4096-byte limit",
            ));
        }
        #[cfg(unix)]
        if path_bytes >= libc::PATH_MAX as usize {
            return Err(StorageError::Invalid(
                "project folder would make the saved path too long; choose a shorter name or location",
            ));
        }
        let parent_dir = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let folder = {
            let (parent_fd, stage_name, staging) = reserve_stage_at(parent_dir)?;
            after_reserve(&staging);
            match initialize_stage_at(parent_fd, stage_name, staging.clone()) {
                Ok(custody) => NewFolder {
                    staging,
                    path,
                    custody,
                },
                Err(error) => return Err(stage_retained(staging, error)),
            }
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let folder = {
            let folder = NewFolder {
                staging: reserve_stage(parent_dir)?,
                path,
            };
            let stage = folder.root();
            after_reserve(stage);
            let initialized = (|| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(stage.join(MARKER))?
                    .sync_all()?;
                fs::create_dir(stage.join(SAMPLES))
            })();
            if let Err(error) = initialized {
                return Err(stage_retained(folder.staging, error.into()));
            }
            folder
        };
        Ok(Self::Folder(folder))
    }
}

/// Pinned `(device, inode)` identity of an acquired project staging directory.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageIdentity {
    pub device: u64,
    pub inode: u64,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct StageCustody {
    parent_fd: OwnedFd,
    stage_fd: OwnedFd,
    stage_name: String,
    identity: StageIdentity,
}

/// A private sibling staging folder, published only after all files are ready.
/// Unpublished staging paths are retained because their contents or identity
/// may have changed; callers must report the path on pre-publication errors.
pub struct NewFolder {
    staging: PathBuf,
    path: PathBuf,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    custody: StageCustody,
}

impl NewFolder {
    pub fn root(&self) -> &Path {
        &self.staging
    }

    /// Return the pinned `(device, inode)` identity observed when the staging
    /// directory descriptor was acquired.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn stage_identity(&self) -> StageIdentity {
        self.custody.identity
    }

    /// Collect one generated sample into this staged project folder anchored
    /// strictly to the pinned staging directory descriptor (on macOS/Linux)
    /// without re-traversing the staging pathname.
    pub fn collect_sample(
        &self,
        relative: &Path,
        source: &Path,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), StorageError> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            verify_stage_entry_identity(&self.custody)?;
            collect_sample_at(&self.custody.stage_fd, relative, source, cancelled)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            collect_sample(&self.staging, relative, source, cancelled)
        }
    }

    pub fn publish(
        self,
        project: &Project,
        cancelled: impl FnOnce() -> bool,
    ) -> Result<SaveNewOutcome, StorageError> {
        let NewFolder {
            staging,
            path,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            custody,
        } = self;
        let result = (|| {
            let name = path
                .file_name()
                .ok_or(StorageError::Invalid("project target needs a filename"))?;
            let root = path
                .parent()
                .ok_or(StorageError::Invalid("project target needs a folder"))?;
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            let mut outcome = {
                verify_stage_entry_identity(&custody)?;
                save_document_at(&custody.stage_fd, &path, name, project)?
            };
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let mut outcome = {
                let stage = staging.as_path();
                crate::save_new_with_outcome(stage.join(name), project)?
            };
            // At this point the document is private. If its stage could not be
            // synchronized, leave the chosen destination absent.
            if let Some(warning) = outcome.warning.take() {
                return Err(warning.source.into());
            }
            if cancelled() {
                return Err(StorageError::Invalid("project save cancelled"));
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                let root_name = root
                    .file_name()
                    .ok_or(StorageError::Invalid("project target needs a folder name"))?;
                // Pre-rename identity recheck narrows accidental substitution
                // without claiming `statat` + `renameat_with` is atomic against
                // a same-user process racing the final syscall window.
                verify_stage_entry_identity(&custody)?;
                publication::rename_directory_exclusive_at(
                    &custody.parent_fd,
                    custody.stage_name.as_str(),
                    root_name,
                )?;
                outcome.path = path.clone();
                if let Some(retained) = outcome.retained_stage.take()
                    && let Some(file_name) = retained.file_name()
                {
                    outcome.retained_stage = Some(root.join(file_name));
                }
                outcome.warning = rustix::fs::fsync(&custody.parent_fd)
                    .map_err(io::Error::from)
                    .err()
                    .map(|source| ReplaceWarning {
                        operation: "synchronize the parent of the project folder",
                        source,
                    });
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let stage = staging.as_path();
                publication::rename_directory_exclusive(stage, root)?;
                outcome.path = path.clone();
                if let Some(retained) = outcome.retained_stage.take()
                    && let Some(file_name) = retained.file_name()
                {
                    outcome.retained_stage = Some(root.join(file_name));
                }
                outcome.warning = root
                    .parent()
                    .and_then(|parent| sync_directory(parent).err())
                    .map(|source| ReplaceWarning {
                        operation: "synchronize the parent of the project folder",
                        source,
                    });
            }
            Ok(outcome)
        })();
        result.map_err(|source| stage_retained(staging, source))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn stat_identity(stat: &rustix::fs::Stat) -> StageIdentity {
    #[allow(clippy::unnecessary_cast)]
    StageIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn reserve_stage_at(parent: &Path) -> io::Result<(OwnedFd, String, PathBuf)> {
    use rustix::fs::{Mode, OFlags};
    let parent_fd = rustix::fs::openat(
        rustix::fs::CWD,
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let dir_mode = Mode::from_bits_truncate(0o700);
    for _ in 0..32 {
        let serial = NEXT_STAGE.fetch_add(1, Relaxed);
        let stage_name = format!(".hex-project-{}-{serial}", std::process::id());
        match rustix::fs::mkdirat(&parent_fd, stage_name.as_str(), dir_mode) {
            Ok(()) => {
                let staging = parent.join(&stage_name);
                return Ok((parent_fd, stage_name, staging));
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
    }
    Err(io::Error::other(
        "could not reserve a project staging folder",
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn verify_dir_fd_empty(stage_fd: &OwnedFd) -> Result<(), StorageError> {
    use rustix::fs::{Dir, Mode, OFlags};
    let scan_fd = rustix::fs::openat(
        stage_fd,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let mut dir = Dir::new(scan_fd).map_err(io::Error::from)?;
    for entry in dir.by_ref() {
        let entry = entry.map_err(io::Error::from)?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            return Err(StorageError::Invalid(
                "project staging directory is not empty; refusing initialization to preserve existing contents",
            ));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn initialize_stage_at(
    parent_fd: OwnedFd,
    stage_name: String,
    _staging: PathBuf,
) -> Result<StageCustody, StorageError> {
    use rustix::fs::{Mode, OFlags};
    let stage_fd = rustix::fs::openat(
        &parent_fd,
        stage_name.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let stat = rustix::fs::fstat(&stage_fd).map_err(io::Error::from)?;
    let identity = stat_identity(&stat);
    // Reject any non-empty directory substituted between `mkdirat` and `openat`
    // before writing `.hex-project` or `samples/`.
    verify_dir_fd_empty(&stage_fd)?;
    let marker_fd = rustix::fs::openat(
        &stage_fd,
        MARKER,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(io::Error::from)?;
    File::from(marker_fd).sync_all()?;
    rustix::fs::mkdirat(&stage_fd, SAMPLES, Mode::from_bits_truncate(0o700))
        .map_err(io::Error::from)?;
    rustix::fs::fsync(&stage_fd).map_err(io::Error::from)?;
    Ok(StageCustody {
        parent_fd,
        stage_fd,
        stage_name,
        identity,
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_sample_directory_at(stage_fd: &OwnedFd, subpath: &Path) -> io::Result<OwnedFd> {
    use rustix::fs::{Mode, OFlags};
    let mut current = rustix::fs::openat(
        stage_fd,
        SAMPLES,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let dir_mode = Mode::from_bits_truncate(0o700);
    for component in subpath.components() {
        let Component::Normal(component) = component else {
            return Err(io::Error::other("invalid project sample path"));
        };
        match rustix::fs::mkdirat(&current, component, dir_mode) {
            Ok(()) => rustix::fs::fsync(&current).map_err(io::Error::from)?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
        current = rustix::fs::openat(
            &current,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
    }
    Ok(current)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn collect_sample_at(
    stage_fd: &OwnedFd,
    relative: &Path,
    source: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<(), StorageError> {
    use rustix::fs::{Mode, OFlags};
    let name = relative
        .file_name()
        .ok_or(StorageError::Invalid("project sample needs a filename"))?;
    let parent = relative
        .parent()
        .ok_or(StorageError::Invalid("project sample needs a parent"))?;
    let dir_fd = open_sample_directory_at(stage_fd, parent)?;
    let source_meta = fs::symlink_metadata(source)?;
    if source_meta.file_type().is_symlink() || !source_meta.is_file() {
        return Err(StorageError::Invalid(
            "generated sample is not a regular file",
        ));
    }
    let mut input = File::open(source)?;
    let len = input.metadata()?.len();
    match rustix::fs::openat(
        &dir_fd,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(existing_fd) => {
            let existing_file = File::from(existing_fd);
            let existing_meta = existing_file.metadata()?;
            if existing_meta.is_file()
                && existing_meta.len() == len
                && same_bytes(&mut input, existing_file, &cancelled)?
            {
                return Ok(());
            }
            return Err(StorageError::Invalid(
                "project sample name is already in use",
            ));
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(rustix::io::Errno::LOOP | rustix::io::Errno::ISDIR) => {
            return Err(StorageError::Invalid(
                "project sample name is already in use",
            ));
        }
        Err(error) => return Err(io::Error::from(error).into()),
    }
    let mut staged = None;
    for _ in 0..32 {
        let serial = NEXT_STAGE.fetch_add(1, Relaxed);
        let temp_name = format!(".hex-r-{}-{serial}", std::process::id());
        match rustix::fs::openat(
            &dir_fd,
            temp_name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        ) {
            Ok(fd) => {
                staged = Some((temp_name, File::from(fd)));
                break;
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(io::Error::from(error).into()),
        }
    }
    let (temp_name, mut file) = staged.ok_or(StorageError::Invalid(
        "could not reserve sample staging filename",
    ))?;
    {
        let mut reader = BufReader::new(input.take(len));
        loop {
            check_cancelled(&cancelled)?;
            let bytes = reader.fill_buf()?;
            if bytes.is_empty() {
                break;
            }
            file.write_all(bytes)?;
            let count = bytes.len();
            reader.consume(count);
        }
        if reader.into_inner().limit() != 0 {
            return Err(io::Error::other("generated sample changed while saving").into());
        }
    }
    file.sync_all()?;
    check_cancelled(&cancelled)?;
    let _published = publication::publish_new_isolated_at(&dir_fd, &mut file, &temp_name, name)?;
    drop(file);
    rustix::fs::fsync(&dir_fd).map_err(io::Error::from)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn save_document_at(
    stage_fd: &OwnedFd,
    final_path: &Path,
    doc_name: &std::ffi::OsStr,
    project: &Project,
) -> Result<SaveNewOutcome, StorageError> {
    use rustix::fs::{Mode, OFlags};
    if crate::project_name::check(final_path).is_err() {
        return Err(StorageError::Invalid(
            "project name is too long to keep recovery files beside it; choose a shorter name",
        ));
    }
    let bytes = crate::encode(project)?;
    let mut staged = None;
    for _ in 0..32 {
        let serial = NEXT_STAGE.fetch_add(1, Relaxed);
        let temp_name = format!(".hex-pending-{}-{serial}", std::process::id());
        match rustix::fs::openat(
            stage_fd,
            temp_name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        ) {
            Ok(fd) => {
                staged = Some((temp_name, File::from(fd)));
                break;
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(io::Error::from(error).into()),
        }
    }
    let (temp_name, mut file) = staged.ok_or(StorageError::Invalid(
        "could not reserve project temporary file",
    ))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    let published =
        publication::publish_new_isolated_at(stage_fd, &mut file, &temp_name, doc_name)?;
    drop(file);
    let retained_stage = published
        .staging_remains()
        .then(|| PathBuf::from(&temp_name));
    let warning = rustix::fs::fsync(stage_fd)
        .map_err(io::Error::from)
        .err()
        .map(|source| ReplaceWarning {
            operation: "synchronize the project staging directory",
            source,
        });
    Ok(SaveNewOutcome {
        path: final_path.to_path_buf(),
        token: crate::files::FileToken::from_validated_bytes(std::sync::Arc::from(bytes)),
        warning,
        retained_stage,
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn verify_stage_entry_identity(custody: &StageCustody) -> Result<(), StorageError> {
    use rustix::fs::{AtFlags, FileType};
    let stat = rustix::fs::statat(
        &custody.parent_fd,
        custody.stage_name.as_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat_identity(&stat) != custody.identity
    {
        return Err(StorageError::Invalid(
            "project staging directory identity changed; refusing publication to preserve unrelated contents",
        ));
    }
    Ok(())
}

#[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
fn reserve_stage(parent: &Path) -> io::Result<PathBuf> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    for _ in 0..32 {
        let serial = NEXT_STAGE.fetch_add(1, Relaxed);
        let stage = parent.join(format!(".hex-project-{}-{serial}", std::process::id()));
        match builder.create(&stage) {
            Ok(()) => return Ok(stage),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(
        "could not reserve a project staging folder",
    ))
}

/// Create the real subdirectories of a project sample path. Reject symlinks
/// and traversal so collecting or generating samples cannot escape the root.
pub fn sample_directory(root: &Path, subpath: &Path) -> io::Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(SAMPLES).components().chain(subpath.components()) {
        let Component::Normal(component) = component else {
            return Err(io::Error::other("invalid project sample path"));
        };
        let parent = path.clone();
        path.push(component);
        match fs::create_dir(&path) {
            Ok(()) => sync_directory(&parent)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&path)?.is_dir() {
                    return Err(io::Error::other(
                        "project sample folder is not a real directory",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(path)
}

/// Place immutable generated audio in the project without replacing another
/// sample. A repeated save can reuse an exact copy. Stream through a sibling
/// staging file so failure leaves any existing sample intact.
pub fn collect_sample(
    root: &Path,
    relative: &Path,
    source: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<(), StorageError> {
    let name = relative
        .file_name()
        .ok_or(StorageError::Invalid("project sample needs a filename"))?;
    let parent = relative
        .parent()
        .ok_or(StorageError::Invalid("project sample needs a parent"))?;
    let directory = sample_directory(root, parent)?;
    let target = directory.join(name);
    if target == source {
        return Ok(());
    }
    if !fs::symlink_metadata(source)?.is_file() {
        return Err(StorageError::Invalid(
            "generated sample is not a regular file",
        ));
    }
    let mut input = File::open(source)?;
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_file() && meta.len() == input.metadata()?.len() => {
            if same_bytes(&mut input, File::open(&target)?, &cancelled)? {
                return Ok(());
            }
            return Err(StorageError::Invalid(
                "project sample name is already in use",
            ));
        }
        Ok(_) => {
            return Err(StorageError::Invalid(
                "project sample name is already in use",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let len = input.metadata()?.len();
    let mut temporary = replacement::prepare_temp_with(&directory, None, |file| {
        let mut input = BufReader::new(input.take(len));
        loop {
            check_cancelled(&cancelled)?;
            let bytes = input.fill_buf()?;
            if bytes.is_empty() {
                break;
            }
            file.write_all(bytes)?;
            let count = bytes.len();
            input.consume(count);
        }
        if input.into_inner().limit() != 0 {
            return Err(io::Error::other("generated sample changed while saving"));
        }
        Ok(())
    })?;
    check_cancelled(&cancelled)?;
    let mut staged = File::open(&temporary.path)?;
    let published =
        publication::publish_new_isolated_from_file(&mut staged, &temporary.path, &target)?;
    drop(staged);
    // Disarm Drop cleanup: exclusive rename already consumed the staging name,
    // and non-consuming modes retain an isolated stage rather than unlinking
    // by pathname.
    temporary.disarm();
    let _ = published;
    files::finish_publication(&temporary.path, &target).map_err(|(_, error)| error.into())
}

fn same_bytes(a: &mut File, b: File, cancelled: &impl Fn() -> bool) -> io::Result<bool> {
    let mut a = BufReader::new(a);
    let mut b = BufReader::new(b);
    loop {
        check_cancelled(cancelled)?;
        let a_buf = a.fill_buf()?;
        let b_buf = b.fill_buf()?;
        let len = a_buf.len().min(b_buf.len());
        if len == 0 {
            return Ok(a_buf.is_empty() && b_buf.is_empty());
        }
        if a_buf[..len] != b_buf[..len] {
            return Ok(false);
        }
        a.consume(len);
        b.consume(len);
    }
}

fn check_cancelled(cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::other("project save cancelled"))
    } else {
        Ok(())
    }
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux", windows)))]
mod tests {
    use super::*;

    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let path = reserve_stage(&std::env::temp_dir()).unwrap();
            Self(path)
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn new_folder_publishes_complete_set_and_accepts_sibling_sets() {
        let files = Files::new();
        let chosen = files.0.join("My Set.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen.clone(), false).unwrap()
        else {
            panic!("a new set should reserve a folder");
        };
        let original = files.0.join("original.wav");
        fs::write(&original, b"sample bytes").unwrap();
        collect_sample(
            folder.root(),
            Path::new("bounces/audio.wav"),
            &original,
            || false,
        )
        .unwrap();
        assert!(!chosen.exists());
        assert!(!files.0.join("My Set").exists());
        let project = Project::new(48_000).unwrap();
        let saved = folder.publish(&project, || false).unwrap();
        let root = files.0.join("My Set");
        assert_eq!(saved.path, root.join("My Set.hex"));
        assert_eq!(crate::load(&saved.path).unwrap(), project);
        assert_eq!(
            fs::read(root.join("samples/bounces/audio.wav")).unwrap(),
            b"sample bytes"
        );
        let version = root.join("Version 2.hex");
        assert!(matches!(
            Destination::prepare(version.clone(), false).unwrap(),
            Destination::File(path) if path == version
        ));
        assert!(Destination::prepare(chosen, false).is_err());
    }

    #[test]
    fn replaced_staging_path_does_not_delete_other_files_on_drop() {
        let files = Files::new();
        let Destination::Folder(folder) =
            Destination::prepare(files.0.join("Song.hex"), false).unwrap()
        else {
            panic!("expected a new folder");
        };
        let stage = folder.root().to_path_buf();
        let original_stage = files.0.join("original-stage");
        fs::rename(&stage, &original_stage).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("other-work"), b"preserve me").unwrap();
        drop(folder);
        assert_eq!(fs::read(stage.join("other-work")).unwrap(), b"preserve me");
        assert!(original_stage.join(MARKER).exists());
    }

    #[test]
    fn failed_stage_initialization_preserves_existing_marker_and_reports_path() {
        let files = Files::new();
        let mut stage = None;
        let result = Destination::prepare_with_hook(files.0.join("Song.hex"), false, |path| {
            stage = Some(path.to_path_buf());
            fs::write(path.join(MARKER), b"other work").unwrap();
        });
        let error = result.err().expect("occupied marker must fail");
        let stage = stage.expect("staging folder reserved");
        assert_eq!(fs::read(stage.join(MARKER)).unwrap(), b"other work");
        assert!(
            error
                .to_string()
                .contains(&stage.to_string_lossy().to_string())
        );
        assert!(!files.0.join("Song").exists());
    }

    #[test]
    fn a_folder_race_never_replaces_the_existing_destination() {
        let files = Files::new();
        let chosen = files.0.join("Song.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen, false).unwrap() else {
            panic!("expected a new folder");
        };
        let stage = folder.root().to_path_buf();
        let occupied = files.0.join("Song");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("existing"), b"keep").unwrap();
        let error = folder
            .publish(&Project::new(48_000).unwrap(), || false)
            .expect_err("occupied destination must fail");
        assert!(
            error
                .to_string()
                .contains(&stage.to_string_lossy().to_string())
        );
        assert!(stage.join("Song.hex").is_file());
        assert_eq!(fs::read(occupied.join("existing")).unwrap(), b"keep");
        assert!(!occupied.join("Song.hex").exists());

        let chosen = files.0.join("Cancelled.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen, false).unwrap() else {
            panic!("expected a new folder");
        };
        let stage = folder.root().to_path_buf();
        let error = folder
            .publish(&Project::new(48_000).unwrap(), || true)
            .expect_err("cancelled save must fail");
        assert!(
            error
                .to_string()
                .contains(&stage.to_string_lossy().to_string())
        );
        assert!(stage.join("Cancelled.hex").is_file());
        assert!(!files.0.join("Cancelled").exists());
    }

    #[cfg(unix)]
    #[test]
    fn derived_path_must_fit_even_when_the_chosen_filename_fits() {
        let files = Files::new();
        let mut parent = files.0.clone();
        let parent_length = libc::PATH_MAX as usize - 200;
        while parent.as_os_str().len() < parent_length {
            let count = (parent_length - parent.as_os_str().len() - 1).min(100);
            parent.push("p".repeat(count.max(1)));
            fs::create_dir(&parent).unwrap();
        }
        let chosen = parent.join(format!("{}.hex", "x".repeat(120)));
        assert!(chosen.as_os_str().len() < libc::PATH_MAX as usize);
        let error = Destination::prepare(chosen, false).err().unwrap();
        assert!(error.to_string().contains("project folder"), "{error}");
        assert!(fs::read_dir(parent).unwrap().next().is_none());
    }

    #[test]
    fn collection_reuses_exact_bytes_and_refuses_collisions_and_cancelled_copies() {
        let files = Files::new();
        let source = files.0.join("original.wav");
        let name = Path::new("recorded/take.wav");
        let root = files.0.join("project");
        fs::create_dir(&root).unwrap();
        fs::write(&source, b"same").unwrap();
        assert!(collect_sample(&root, name, &source, || true).is_err());
        assert!(!root.join(SAMPLES).join(name).exists());
        for _ in 0..2 {
            collect_sample(&root, name, &source, || false).unwrap();
        }
        fs::write(&source, b"new!").unwrap();
        assert!(collect_sample(&root, name, &source, || false).is_err());
        assert_eq!(fs::read(root.join(SAMPLES).join(name)).unwrap(), b"same");
        assert!(collect_sample(&root, Path::new("../escape.wav"), &source, || false).is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn substituted_nonempty_dir_symlink_or_file_between_mkdir_and_open_is_never_modified() {
        // 1. Non-empty unrelated directory swapped into the reserved staging pathname.
        let files = Files::new();
        let foreign_dir = files.0.join("foreign-dir");
        fs::create_dir(&foreign_dir).unwrap();
        fs::write(foreign_dir.join("keep.txt"), b"unrelated user data").unwrap();
        let displaced = files.0.join("displaced-stage");
        let mut stage_path = None;
        let error = Destination::prepare_with_hook(files.0.join("Song.hex"), false, |stage| {
            stage_path = Some(stage.to_path_buf());
            fs::rename(stage, &displaced).unwrap();
            fs::rename(&foreign_dir, stage).unwrap();
        })
        .err()
        .expect("substituted non-empty directory must be refused");
        let stage_path = stage_path.expect("stage reserved");
        assert!(
            error
                .to_string()
                .contains(&stage_path.to_string_lossy().to_string())
        );
        assert_eq!(
            fs::read(stage_path.join("keep.txt")).unwrap(),
            b"unrelated user data"
        );
        assert!(!stage_path.join(MARKER).exists());
        assert!(!stage_path.join(SAMPLES).exists());
        assert!(!files.0.join("Song").exists());

        // 2. Symlink swapped into the reserved staging pathname must not be followed.
        let files = Files::new();
        let link_target = files.0.join("symlink-target");
        fs::create_dir(&link_target).unwrap();
        let displaced = files.0.join("displaced-stage");
        let error = Destination::prepare_with_hook(files.0.join("Link.hex"), false, |stage| {
            fs::rename(stage, &displaced).unwrap();
            std::os::unix::fs::symlink(&link_target, stage).unwrap();
        })
        .err()
        .expect("substituted symlink must be refused");
        assert!(!link_target.join(MARKER).exists());
        assert!(!link_target.join(SAMPLES).exists());
        assert!(error.to_string().contains(".hex-project-"));

        // 3. Regular file swapped into the reserved staging pathname must remain intact.
        let files = Files::new();
        let regular = files.0.join("regular-file");
        fs::write(&regular, b"do not touch").unwrap();
        let displaced = files.0.join("displaced-stage");
        let mut stage_path = None;
        let error = Destination::prepare_with_hook(files.0.join("File.hex"), false, |stage| {
            stage_path = Some(stage.to_path_buf());
            fs::rename(stage, &displaced).unwrap();
            fs::rename(&regular, stage).unwrap();
        })
        .err()
        .expect("substituted regular file must be refused");
        let stage_path = stage_path.expect("stage reserved");
        assert_eq!(fs::read(&stage_path).unwrap(), b"do not touch");
        assert!(
            error
                .to_string()
                .contains(&stage_path.to_string_lossy().to_string())
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn substituted_stage_before_publish_refuses_and_preserves_both_directories() {
        let files = Files::new();
        let chosen = files.0.join("Guarded.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen, false).unwrap() else {
            panic!("expected new project folder");
        };
        let identity = folder.stage_identity();
        assert!(identity.inode > 0);
        let stage = folder.root().to_path_buf();
        let displaced = files.0.join("original-staged");
        fs::rename(&stage, &displaced).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("foreign.txt"), b"foreign bytes").unwrap();

        let error = folder
            .publish(&Project::new(48_000).unwrap(), || false)
            .expect_err("substituted stage identity must be refused before publication");
        assert!(
            error
                .to_string()
                .contains(&stage.to_string_lossy().to_string())
        );
        assert!(!files.0.join("Guarded").exists());
        assert_eq!(
            fs::read(stage.join("foreign.txt")).unwrap(),
            b"foreign bytes"
        );
        assert!(!stage.join("Guarded.hex").exists());
        assert!(displaced.join(MARKER).is_file());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn substitution_before_sample_collection_refuses_and_preserves_foreign_folder() {
        let files = Files::new();
        let chosen = files.0.join("Collected.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen, false).unwrap() else {
            panic!("expected new project folder");
        };
        let stage = folder.root().to_path_buf();
        let displaced = files.0.join("displaced-original");
        let source = files.0.join("take.wav");
        fs::write(&source, b"recorded-take-pcm").unwrap();

        // Adversarial swap before sample collection: replace stage path with a
        // foreign directory containing existing user media.
        fs::rename(&stage, &displaced).unwrap();
        fs::create_dir_all(stage.join(SAMPLES).join("recorded")).unwrap();
        fs::write(
            stage.join(SAMPLES).join("recorded/foreign.wav"),
            b"foreign-audio",
        )
        .unwrap();

        let error = folder
            .collect_sample(Path::new("recorded/take.wav"), &source, || false)
            .expect_err("substituted stage must refuse sample collection");
        assert!(error.to_string().contains("identity changed"));
        assert!(!stage.join(SAMPLES).join("recorded/take.wav").exists());
        assert_eq!(
            fs::read(stage.join(SAMPLES).join("recorded/foreign.wav")).unwrap(),
            b"foreign-audio"
        );
        assert!(displaced.join(MARKER).is_file());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn anchored_sample_collection_publish_and_cancellation_preserve_data() {
        let files = Files::new();
        let source = files.0.join("bounce.wav");
        fs::write(&source, b"bounced-wav-data").unwrap();

        // Normal Save As with generated media through anchored `NewFolder::collect_sample`.
        let chosen = files.0.join("Anchored.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen, false).unwrap() else {
            panic!("expected new project folder");
        };
        folder
            .collect_sample(Path::new("bounces/bounce.wav"), &source, || false)
            .expect("anchored sample collection");
        // Repeated collection of identical bytes succeeds idempotently.
        folder
            .collect_sample(Path::new("bounces/bounce.wav"), &source, || false)
            .expect("idempotent anchored sample collection");
        // Path escape is rejected at the descriptor boundary.
        assert!(
            folder
                .collect_sample(Path::new("../escape.wav"), &source, || false)
                .is_err()
        );
        let project = Project::new(48_000).unwrap();
        let outcome = folder
            .publish(&project, || false)
            .expect("anchored publish");
        assert_eq!(outcome.path, files.0.join("Anchored/Anchored.hex"));
        assert_eq!(crate::load(&outcome.path).unwrap(), project);
        assert_eq!(
            fs::read(files.0.join("Anchored/samples/bounces/bounce.wav")).unwrap(),
            b"bounced-wav-data"
        );

        // Cancellation after anchored collection retains the stage and leaves
        // the target destination absent.
        let cancelled_target = files.0.join("Cancelled.hex");
        let Destination::Folder(folder2) = Destination::prepare(cancelled_target, false).unwrap()
        else {
            panic!("expected new project folder");
        };
        let stage2 = folder2.root().to_path_buf();
        folder2
            .collect_sample(Path::new("bounces/bounce.wav"), &source, || false)
            .expect("collect before cancel");
        let err = folder2
            .publish(&project, || true)
            .expect_err("cancelled publish must fail");
        assert!(err.to_string().contains("cancelled"));
        assert!(!files.0.join("Cancelled").exists());
        assert_eq!(
            fs::read(stage2.join("samples/bounces/bounce.wav")).unwrap(),
            b"bounced-wav-data"
        );
    }
}
