//! Safe OS publication operations, isolated from rendering and UI consent.
use super::*;

/// Maximum file size hashed completely by [`OverwriteConsent`]. Files larger
/// than this ceiling hash bounded head, middle, and tail windows plus file
/// length so interactive save confirmation stays bounded in memory and time.
const FULL_DIGEST_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const SAMPLED_WINDOW_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContentDigest {
    sha256: [u8; 32],
    full_file: bool,
}

/// Concrete regular destination accepted for replacement. Inspection performs
/// no write; callers obtain explicit user consent before passing this policy.
/// In addition to file metadata (`len`, `mtime`, Unix `dev`/`ino`), consent
/// captures a content digest so in-place same-length, restored-mtime mutations
/// are detected instead of silently accepted (`#8086`).
#[derive(Clone, Debug)]
pub struct OverwriteConsent {
    path: PathBuf,
    metadata: fs::Metadata,
    digest: std::sync::Arc<ContentDigest>,
}

#[derive(Clone, Debug, Default)]
pub enum Publication {
    #[default]
    CreateNew,
    Replace(OverwriteConsent),
}

impl Publication {
    /// Capture the policy as soon as the caller confirms this concrete destination,
    /// before queuing or rendering. An existing regular file binds replacement to
    /// its observed identity. An absent path retains no-replace semantics.
    pub fn confirmed_save(path: &Path) -> Result<Self, ExportError> {
        match OverwriteConsent::inspect(path) {
            Ok(consent) => Ok(Self::Replace(consent)),
            Err(ExportError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                Ok(Self::CreateNew)
            }
            Err(error) => Err(error),
        }
    }
}

impl OverwriteConsent {
    pub fn inspect(path: &Path) -> Result<Self, ExportError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| ExportError::Io {
            operation: "inspect the confirmed overwrite destination",
            source,
        })?;
        if !metadata.file_type().is_file() {
            return Err(ExportError::Invalid(
                "overwrite requires an existing regular file",
            ));
        }
        let digest =
            digest_regular_file(path, metadata.len()).map_err(|source| ExportError::Io {
                operation: "fingerprint the confirmed overwrite destination",
                source,
            })?;
        Ok(Self {
            path: path.to_owned(),
            metadata,
            digest: std::sync::Arc::new(digest),
        })
    }

    pub(super) fn matches(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return false;
        };
        if !metadata.file_type().is_file()
            || metadata.len() != self.metadata.len()
            || metadata.modified().ok() != self.metadata.modified().ok()
        {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.dev() != self.metadata.dev() || metadata.ino() != self.metadata.ino() {
                return false;
            }
        }
        match digest_regular_file(path, metadata.len()) {
            Ok(actual) => actual == *self.digest,
            Err(_) => false,
        }
    }

    pub(super) fn validate(&self, path: &Path) -> Result<(), ExportError> {
        if path != self.path || !self.matches(path) {
            return Err(ExportError::Invalid(
                "confirmed overwrite destination changed",
            ));
        }
        Ok(())
    }
}

pub(super) fn replace_staged(
    source: &Path,
    destination: &Path,
    consent: &OverwriteConsent,
    before_exchange: impl FnOnce(),
) -> Result<(), ExportError> {
    consent.validate(destination)?;
    before_exchange();
    exchange(source, destination).map_err(|source| ExportError::Io {
        operation: "exchange the confirmed overwrite destination",
        source,
    })?;
    if !consent.matches(source) {
        return Err(ExportError::PublicationConflict {
            path: destination.to_owned(),
            recovery_path: source.to_owned(),
        });
    }
    Ok(())
}

/// How a staged export became visible at its create-new destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Published {
    /// The destination is a second link; the staging name still exists.
    #[cfg_attr(not(test), allow(dead_code))]
    Linked,
    /// The staging file moved to the destination; its name no longer exists.
    Renamed,
    /// The destination is a descriptor-bound CoW clone; the staging name remains.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Cloned,
    /// The destination is an exclusive synced copy; the staging name remains.
    Copied,
}

impl Published {
    pub(super) fn staging_remains(self) -> bool {
        !matches!(self, Self::Renamed)
    }
}

/// Whether a failure occurred before or after exclusive destination creation.
#[derive(Debug)]
pub(super) enum NewPublicationError {
    BeforeCopy(io::Error),
    IncompleteCopy(io::Error),
}
impl From<io::Error> for NewPublicationError {
    fn from(error: io::Error) -> Self {
        Self::BeforeCopy(error)
    }
}

use std::io::{Read, Seek, SeekFrom};

fn digest_regular_file(path: &Path, expected_len: u64) -> io::Result<ContentDigest> {
    let mut file = File::open(path)?;
    let mut hasher = crate::stale_temporary::Sha256::new();
    hasher.update(&expected_len.to_le_bytes());
    let mut buf = [0_u8; 16 * 1024];
    if expected_len <= FULL_DIGEST_LIMIT_BYTES {
        let mut total = 0_u64;
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total = total.saturating_add(n as u64);
            if total > expected_len {
                return Err(io::Error::other("file grew during consent fingerprint"));
            }
            hasher.update(&buf[..n]);
        }
        if total != expected_len {
            return Err(io::Error::other("file shrank during consent fingerprint"));
        }
        return Ok(ContentDigest {
            sha256: hasher.finalize(),
            full_file: true,
        });
    }
    // Bounded head, middle, and tail windows for files > 64 MiB.
    let offsets = [
        0_u64,
        expected_len / 2 - (SAMPLED_WINDOW_BYTES as u64 / 2),
        expected_len.saturating_sub(SAMPLED_WINDOW_BYTES as u64),
    ];
    for offset in offsets {
        file.seek(SeekFrom::Start(offset))?;
        let mut remaining = SAMPLED_WINDOW_BYTES;
        while remaining > 0 {
            let to_read = remaining.min(buf.len());
            let n = file.read(&mut buf[..to_read])?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            remaining -= n;
        }
    }
    Ok(ContentDigest {
        sha256: hasher.finalize(),
        full_file: false,
    })
}

/// Outcome of source-descriptor-bound create-new WAV publication (`#8105` / `#8094`).
///
/// # Verified platform contract (macOS arm64 / Linux)
///
/// 1. **APFS (`Cloned`)**: Uses macOS `fclonefileat(source_fd, CWD, destination, 0)`
///    via pinned `rustix` (zero unsafe workspace code). Atomic, no-replace
///    (`EEXIST`), bound to the open rendered file descriptor (immune to staging
///    pathname substitution), and yields an independent CoW inode (`nlink == 1`).
///    The staging pathname remains on disk; callers must NOT unlink it blindly
///    by pathname (post-publication stage custody is tracked by `#8119`/`#8120`).
/// 2. **Non-clone, rename-capable volumes (`RenamedAfterIdentityCheck` — e.g.
///    macOS HFS+, FAT32, Linux ext4/XFS/btrfs)**: Authoritative Apple CoreOS DTS
///    guidance (Developer Forums thread 850377), Apple Technical Q&A QA1113
///    (`/.vol` is an unsupported debugging detail), and disposable HFS+ image
///    controls confirm macOS exposes no supported public API to hard-link or
///    atomically rename directly from an open file descriptor when `fclonefileat`
///    returns `ENOTSUP`. This path verifies `fstat(source)` matches
///    `symlink_metadata(staging)` `(dev, ino)` before calling
///    `renameatx_np(RENAME_EXCL)` / `renameat2(RENAME_NOREPLACE)`. That check
///    rejects prior substitutions without touching foreign or destination files,
///    and exclusive rename is atomic/no-replace and consumes the staging name,
///    but **check-then-rename is NOT atomic** against a same-user non-cooperating
///    process racing the final syscall window, and advisory `flock` does not
///    prevent in-place writes through another handle. Full same-user adversary
///    immunity on non-clone volumes would require an OS/entitlement boundary
///    decision (not a userspace workaround).
/// 3. **Non-clone, non-rename volumes (`CopiedFromDescriptor` — e.g. macOS
///    ExFAT/FSKit)**: Reads strictly from the held `source` descriptor into an
///    exclusively created destination (`O_CREAT | O_EXCL`). Preserves source-fd
///    binding, no-replace, and foreign files at the staging path, at the
///    documented trade-off of non-atomic creation on crash mid-copy.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceBoundOutcome {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Cloned,
    RenamedAfterIdentityCheck,
    CopiedFromDescriptor,
}

/// Publish a complete, synced staging file using the strongest source-bound
/// primitive supported by the destination volume (`#8105`).
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn publish_new_source_bound(
    source: &mut File,
    staging: &Path,
    destination: &Path,
) -> Result<SourceBoundOutcome, NewPublicationError> {
    match clone_from_file(source, destination) {
        Ok(()) => return Ok(SourceBoundOutcome::Cloned),
        Err(error) if !clone_unsupported(&error) => return Err(error.into()),
        Err(_) => {}
    }
    verify_staging_matches_descriptor(source, staging)?;
    match exclusive_rename_step(staging, destination) {
        Ok(()) => Ok(SourceBoundOutcome::RenamedAfterIdentityCheck),
        Err(error) if rename_unsupported(&error) => {
            copy_exclusive_from_file(source, destination)?;
            Ok(SourceBoundOutcome::CopiedFromDescriptor)
        }
        Err(error) => Err(error.into()),
    }
}

fn verify_staging_matches_descriptor(source: &File, staging: &Path) -> io::Result<()> {
    let staged_meta = fs::symlink_metadata(staging)?;
    if staged_meta.file_type().is_symlink() || !staged_meta.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "staging pathname was substituted with a non-regular entry before publication",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let open_meta = source.metadata()?;
        if open_meta.dev() != staged_meta.dev() || open_meta.ino() != staged_meta.ino() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staging pathname identity changed before exclusive rename",
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = source;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn clone_from_file(source: &File, destination: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = simulate::clone_error() {
        return Err(error);
    }
    rustix::fs::fclonefileat(
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::CloneFlags::empty(),
    )
    .map_err(io::Error::from)
}

#[cfg(not(target_os = "macos"))]
fn clone_from_file(_: &File, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "descriptor clone is unsupported on this platform",
    ))
}

fn clone_unsupported(error: &io::Error) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use rustix::io::Errno;
        if errno_in(
            error,
            &[
                Errno::NOTSUP,
                Errno::OPNOTSUPP,
                Errno::INVAL,
                Errno::NOSYS,
                Errno::XDEV,
            ],
        ) {
            return true;
        }
    }
    error.kind() == io::ErrorKind::Unsupported
}

/// Publish a complete, synced staging file at an absent `destination` bound to
/// the open rendered `source` descriptor, without creating a hard-link alias or
/// unlinking `staging` by pathname afterwards (`#8094` / `#8119`).
///
/// 1. If `staging` still resolves to `source`'s exact `(dev, ino)`, attempt an
///    exclusive no-replace rename (atomic, zero leftover staging file). Note:
///    check-then-rename is not kernel-atomic against a concurrent same-user
///    replacer in the final syscall window.
/// 2. If `staging` was substituted before publication or exclusive rename is
///    unsupported on the volume, publish directly from the held `source`
///    descriptor via macOS `fclonefileat` (atomic, no-replace, isolated CoW)
///    or descriptor-bound exclusive copy (`O_CREAT | O_EXCL`). Any foreign file
///    at `staging` survives untouched and the caller reports `retained_stage`.
pub(super) fn publish_new_from_file(
    source: &mut File,
    staging: &Path,
    destination: &Path,
) -> Result<Published, NewPublicationError> {
    if verify_staging_matches_descriptor(source, staging).is_ok() {
        match exclusive_rename_step(staging, destination) {
            Ok(()) => return Ok(Published::Renamed),
            Err(error) if !rename_unsupported(&error) => return Err(error.into()),
            Err(_) => {}
        }
    }
    match clone_from_file(source, destination) {
        Ok(()) => return Ok(Published::Cloned),
        Err(error) if !clone_unsupported(&error) => return Err(error.into()),
        Err(_) => {}
    }
    copy_exclusive_from_file(source, destination)?;
    Ok(Published::Copied)
}

/// Publish a complete, synced staging file at an absent destination without
/// ever replacing an existing file. Tries, in order:
///
/// 1. A hard link (atomic; the staging name remains).
/// 2. On volumes without hard links (exFAT/FAT and many SMB shares report
///    `ENOTSUP` or `EPERM`), an exclusive no-replace rename (atomic; consumes
///    the staging name).
/// 3. When exclusive rename is also unsupported, an exclusively created,
///    written and synced copy. It is not atomic: a crash during the copy can
///    leave an incomplete *new* file, never a replaced one. On failure the
///    destination pathname is retained for inspection: another writer may
///    have replaced it after creation.
///
/// Every step refuses an existing destination with `AlreadyExists`; other
/// link failures (`EACCES`, `ENOSPC`, ...) propagate without fallback. This
/// mirrors hex-storage's create-new publication locally because the
/// dependency policy permits hex-export to depend only on hex-engine.
#[cfg_attr(not(any(test, windows)), allow(dead_code))]
pub(super) fn publish_new(
    staging: &Path,
    destination: &Path,
) -> Result<Published, NewPublicationError> {
    match link(staging, destination) {
        Ok(()) => return Ok(Published::Linked),
        Err(error) if !link_unsupported(&error) => return Err(error.into()),
        Err(_) => {}
    }
    match exclusive_rename_step(staging, destination) {
        Ok(()) => Ok(Published::Renamed),
        Err(error) if rename_unsupported(&error) => {
            let mut source = File::open(staging)?;
            copy_exclusive_from_file(&mut source, destination)?;
            Ok(Published::Copied)
        }
        Err(error) => Err(error.into()),
    }
}

fn link(staging: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = simulate::link_error() {
        return Err(error);
    }
    fs::hard_link(staging, destination)
}

fn exclusive_rename_step(staging: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = simulate::rename_error() {
        return Err(error);
    }
    rename_exclusive(staging, destination)
}

fn copy_exclusive_from_file(
    source: &mut File,
    destination: &Path,
) -> Result<(), NewPublicationError> {
    source.seek(SeekFrom::Start(0))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // Umask can only narrow the staging file's permissions.
        options.mode(source.metadata()?.permissions().mode() & 0o777);
    }
    let mut target = options.open(destination)?;
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    let result = match simulate::copy_failure() {
        Some(simulate::CopyFailure::Write) => target.write_all(b"partial").and_then(|()| {
            super::after_exchange_hook::run(destination);
            Err(io::Error::other("injected copy failure"))
        }),
        Some(simulate::CopyFailure::Sync) => io::copy(source, &mut target).and_then(|_| {
            super::after_exchange_hook::run(destination);
            Err(io::Error::other("injected sync failure"))
        }),
        None => io::copy(source, &mut target).and_then(|_| target.sync_all()),
    };
    #[cfg(not(all(test, any(target_os = "linux", target_os = "macos"))))]
    let result = io::copy(source, &mut target).and_then(|_| target.sync_all());
    if let Err(error) = result {
        // Even an exclusively created pathname can have been replaced since
        // opening it. Preserve the current entry and report it for inspection.
        return Err(NewPublicationError::IncompleteCopy(error));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn errno_in(error: &io::Error, codes: &[rustix::io::Errno]) -> bool {
    error
        .raw_os_error()
        .is_some_and(|raw| codes.iter().any(|code| code.raw_os_error() == raw))
}

fn link_unsupported(error: &io::Error) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use rustix::io::Errno;
        if errno_in(
            error,
            &[Errno::NOTSUP, Errno::OPNOTSUPP, Errno::PERM, Errno::NOSYS],
        ) {
            return true;
        }
    }
    error.kind() == io::ErrorKind::Unsupported
}

fn rename_unsupported(error: &io::Error) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use rustix::io::Errno;
        // Linux reports renameat2 flags a filesystem lacks as EINVAL.
        if errno_in(
            error,
            &[Errno::NOTSUP, Errno::OPNOTSUPP, Errno::INVAL, Errno::NOSYS],
        ) {
            return true;
        }
    }
    error.kind() == io::ErrorKind::Unsupported
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn exchange(source: &Path, destination: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::EXCHANGE,
    )
    .map_err(io::Error::from)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn rename_exclusive(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic exclusive publication is unsupported on this platform",
    ))
}

/// Atomically claim an absent `destination` directory and publish `staging`'s
/// stem files without replacing any existing file or directory.
pub(super) fn rename_directory_exclusive(staging: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        rename_exclusive(staging, destination)
    }
    #[cfg(windows)]
    {
        publish_directory_exclusive_windows(staging, destination)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (staging, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic exclusive publication is unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
fn publish_directory_exclusive_windows(staging: &Path, destination: &Path) -> io::Result<()> {
    const MAX_ENTRIES: usize = 4096;
    let meta = fs::symlink_metadata(staging)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "staged stem batch must be a real directory",
        ));
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(staging)? {
        if files.len() >= MAX_ENTRIES {
            return Err(io::Error::other("staged stem batch entry bound exceeded"));
        }
        let entry = entry?;
        let path = entry.path();
        let kind = fs::symlink_metadata(&path)?.file_type();
        if kind.is_symlink() || !kind.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "staged stem batch must contain only regular files",
            ));
        }
        files.push(entry.file_name());
    }
    // `create_dir` atomically refuses any existing file, symlink, or directory.
    fs::create_dir(destination)?;
    let mut staged_to_unlink = Vec::with_capacity(files.len());
    for name in &files {
        let src = staging.join(name);
        let dst = destination.join(name);
        let published = publish_new(&src, &dst).map_err(|error| match error {
            NewPublicationError::BeforeCopy(e) | NewPublicationError::IncompleteCopy(e) => e,
        })?;
        if published.staging_remains() {
            staged_to_unlink.push(src);
        }
    }
    for path in staged_to_unlink {
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_dir(staging);
    Ok(())
}

#[cfg(windows)]
pub(super) fn exchange(source: &Path, destination: &Path) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_BACKUP: AtomicU64 = AtomicU64::new(0);

    let src_meta = fs::symlink_metadata(source)?;
    let dst_meta = fs::symlink_metadata(destination)?;
    if src_meta.file_type().is_symlink()
        || !src_meta.is_file()
        || dst_meta.file_type().is_symlink()
        || !dst_meta.is_file()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "exchange requires two existing regular files",
        ));
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut backup = None;
    for _ in 0..32 {
        let serial = NEXT_BACKUP.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".hex-exchange-{}-{serial}.tmp", std::process::id()));
        match fs::hard_link(destination, &candidate) {
            Ok(()) => {
                backup = Some(candidate);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let backup = backup.ok_or_else(|| {
        io::Error::other("could not reserve a sibling backup link for confirmed overwrite")
    })?;
    if let Err(error) = fs::rename(source, destination) {
        let _ = fs::remove_file(&backup);
        return Err(error);
    }
    if let Err(error) = fs::rename(&backup, source) {
        let _ = fs::remove_file(&backup);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn exchange(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic replacement is unsupported on this platform",
    ))
}

/// Test-only per-thread volume capabilities injected at this boundary.
#[cfg(test)]
pub(super) mod simulate {
    use std::{cell::Cell, io};

    #[derive(Clone, Copy)]
    pub(crate) enum Volume {
        /// Hard links fail with this raw errno; exclusive rename works.
        #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
        NoHardLinks(i32),
        /// Descriptor clone (`fclonefileat`) is unavailable; exclusive rename works (HFS+/FAT32).
        #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
        RenameOnly,
        /// Neither hard links nor exclusive rename nor clone (copy fallback).
        NoAtomicCreate,
    }

    thread_local!(static VOLUME: Cell<Option<Volume>> = const { Cell::new(None) });
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[derive(Clone, Copy)]
    pub(crate) enum CopyFailure {
        Write,
        Sync,
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    thread_local!(static COPY_FAILURE: Cell<Option<CopyFailure>> = const { Cell::new(None) });

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) struct CopyFailureGuard;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl Drop for CopyFailureGuard {
        fn drop(&mut self) {
            COPY_FAILURE.with(|flag| flag.set(None));
        }
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn fail_copy(failure: CopyFailure) -> CopyFailureGuard {
        COPY_FAILURE.with(|flag| flag.set(Some(failure)));
        CopyFailureGuard
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn copy_failure() -> Option<CopyFailure> {
        COPY_FAILURE.with(Cell::get)
    }

    /// Restores native behaviour on drop, including after a failed assertion.
    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            VOLUME.with(|volume| volume.set(None));
        }
    }

    pub(crate) fn volume(volume: Volume) -> Guard {
        VOLUME.with(|cell| cell.set(Some(volume)));
        Guard
    }

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "simulated unsupported volume")
    }

    pub(super) fn link_error() -> Option<io::Error> {
        match VOLUME.with(Cell::get)? {
            Volume::NoHardLinks(code) => Some(io::Error::from_raw_os_error(code)),
            Volume::RenameOnly | Volume::NoAtomicCreate => Some(unsupported()),
        }
    }

    pub(super) fn rename_error() -> Option<io::Error> {
        match VOLUME.with(Cell::get)? {
            Volume::NoHardLinks(_) | Volume::RenameOnly => None,
            Volume::NoAtomicCreate => Some(unsupported()),
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn clone_error() -> Option<io::Error> {
        match VOLUME.with(Cell::get)? {
            Volume::NoHardLinks(_) | Volume::RenameOnly | Volume::NoAtomicCreate => {
                Some(unsupported())
            }
        }
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod source_bound_tests {
    use super::simulate::{Volume, volume};
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("hex-export-sb-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn open_stage(&self, name: &str, bytes: &[u8]) -> (File, PathBuf) {
            let path = self.0.join(name);
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
            (file, path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn source_bound_apfs_clone_publishes_rendered_fd_despite_stage_swap() {
        let dir = Dir::new();
        let (mut fd, stage) = dir.open_stage(".stage-apfs", b"rendered-apfs-wav");
        let displaced = dir.0.join("displaced-apfs");
        fs::rename(&stage, &displaced).unwrap();
        fs::write(&stage, b"foreign-stage-bytes").unwrap();

        let destination = dir.0.join("out.wav");
        let outcome = publish_new_source_bound(&mut fd, &stage, &destination).unwrap();
        assert_eq!(outcome, SourceBoundOutcome::Cloned);
        assert_eq!(fs::read(&destination).unwrap(), b"rendered-apfs-wav");
        assert_eq!(fs::read(&stage).unwrap(), b"foreign-stage-bytes");
        assert_eq!(fs::read(&displaced).unwrap(), b"rendered-apfs-wav");
        // Occupied destination is refused without replacing existing bytes.
        let error = publish_new_source_bound(&mut fd, &stage, &destination).unwrap_err();
        assert!(matches!(
            error,
            NewPublicationError::BeforeCopy(e) if e.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"rendered-apfs-wav");
    }

    #[test]
    fn source_bound_rename_only_volume_publishes_atomically_and_refuses_detected_swap() {
        let dir = Dir::new();
        let _guard = volume(Volume::RenameOnly);

        // Normal rename-only path publishes atomically and consumes staging.
        let (mut fd, stage) = dir.open_stage(".stage-ok", b"rendered-hfs-wav");
        let dest = dir.0.join("ok.wav");
        let outcome = publish_new_source_bound(&mut fd, &stage, &dest).unwrap();
        assert_eq!(outcome, SourceBoundOutcome::RenamedAfterIdentityCheck);
        assert_eq!(fs::read(&dest).unwrap(), b"rendered-hfs-wav");
        assert!(!stage.exists());

        // Occupied destination refuses without replacing existing bytes.
        let (mut fd2, stage2) = dir.open_stage(".stage-dup", b"newer-wav");
        let error = publish_new_source_bound(&mut fd2, &stage2, &dest).unwrap_err();
        assert!(matches!(
            error,
            NewPublicationError::BeforeCopy(e) if e.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&dest).unwrap(), b"rendered-hfs-wav");
        assert_eq!(fs::read(&stage2).unwrap(), b"newer-wav");

        // Swapping staging pathname before publication is detected and refused,
        // preserving both foreign bytes and displaced rendered stage.
        let (mut fd3, stage3) = dir.open_stage(".stage-swap", b"rendered-swapped");
        let displaced = dir.0.join("displaced-hfs");
        fs::rename(&stage3, &displaced).unwrap();
        fs::write(&stage3, b"foreign-substituted").unwrap();
        let blocked_dest = dir.0.join("blocked.wav");
        let error = publish_new_source_bound(&mut fd3, &stage3, &blocked_dest).unwrap_err();
        assert!(matches!(
            error,
            NewPublicationError::BeforeCopy(e) if e.kind() == io::ErrorKind::InvalidData
        ));
        assert!(!blocked_dest.exists());
        assert_eq!(fs::read(&stage3).unwrap(), b"foreign-substituted");
        assert_eq!(fs::read(&displaced).unwrap(), b"rendered-swapped");
    }

    #[test]
    fn source_bound_copy_fallback_reads_held_descriptor_despite_stage_swap() {
        let dir = Dir::new();
        let _guard = volume(Volume::NoAtomicCreate);
        let (mut fd, stage) = dir.open_stage(".stage-exfat", b"rendered-exfat-wav");
        let displaced = dir.0.join("displaced-exfat");
        fs::rename(&stage, &displaced).unwrap();
        fs::write(&stage, b"foreign-exfat").unwrap();
        let dest = dir.0.join("copied.wav");
        // Note: copy_exclusive_from_file itself reads strictly from the held fd.
        copy_exclusive_from_file(&mut fd, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"rendered-exfat-wav");
        assert_eq!(fs::read(&stage).unwrap(), b"foreign-exfat");
        assert_eq!(fs::read(&displaced).unwrap(), b"rendered-exfat-wav");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::simulate::{Volume, volume};
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("hex-export-pub-win-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn windows_publish_new_directory_exclusive_and_exchange_preserve_data() {
        let dir = Dir::new();
        // Copy fallback when hard links are unavailable.
        {
            let _guard = volume(Volume::NoAtomicCreate);
            let staged = dir.0.join(".staged.wav");
            let dst = dir.0.join("copied.wav");
            fs::write(&staged, b"new-wav").unwrap();
            assert_eq!(publish_new(&staged, &dst).unwrap(), Published::Copied);
            assert_eq!(fs::read(&dst).unwrap(), b"new-wav");
            assert!(publish_new(&staged, &dst).is_err());
        }
        // Directory exclusive publication refuses existing file, empty dir, and non-empty dir.
        let stage_dir = dir.0.join(".stage-stems");
        fs::create_dir(&stage_dir).unwrap();
        fs::write(stage_dir.join("01-Track.wav"), b"stem-data").unwrap();
        let existing_file = dir.0.join("existing-file");
        fs::write(&existing_file, b"keep-file").unwrap();
        assert!(rename_directory_exclusive(&stage_dir, &existing_file).is_err());
        assert_eq!(fs::read(&existing_file).unwrap(), b"keep-file");

        let existing_dir = dir.0.join("existing-dir");
        fs::create_dir(&existing_dir).unwrap();
        assert!(rename_directory_exclusive(&stage_dir, &existing_dir).is_err());

        let target_dir = dir.0.join("published-stems");
        rename_directory_exclusive(&stage_dir, &target_dir).unwrap();
        assert!(!stage_dir.exists());
        assert_eq!(
            fs::read(target_dir.join("01-Track.wav")).unwrap(),
            b"stem-data"
        );

        // Confirmed exchange swaps source and destination and preserves displaced bytes at source.
        let old = dir.0.join("mix.wav");
        let staged = dir.0.join(".stage-mix.tmp");
        fs::write(&old, b"previous-mix").unwrap();
        fs::write(&staged, b"replacement-mix").unwrap();
        let consent = OverwriteConsent::inspect(&old).unwrap();
        replace_staged(&staged, &old, &consent, || {}).unwrap();
        assert_eq!(fs::read(&old).unwrap(), b"replacement-mix");
        assert_eq!(fs::read(&staged).unwrap(), b"previous-mix");
    }
}
