//! Safe OS publication operations, isolated from rendering and UI consent.
use super::*;

/// Concrete regular destination accepted for replacement. Inspection performs
/// no write; callers obtain explicit user consent before passing this policy.
#[derive(Clone, Debug)]
pub struct OverwriteConsent {
    path: PathBuf,
    metadata: fs::Metadata,
}

#[derive(Clone, Debug, Default)]
pub enum Publication {
    #[default]
    CreateNew,
    Replace(OverwriteConsent),
}

impl Publication {
    /// The caller has accepted this concrete path through a native save dialog.
    /// Existing regular files may be replaced; absent paths still publish with
    /// no-replace semantics, so a later collision never truncates another file.
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
        Ok(Self {
            path: path.to_owned(),
            metadata,
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
        true
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
    Linked,
    /// The staging file moved to the destination; its name no longer exists.
    Renamed,
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
            copy_exclusive(staging, destination)?;
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

fn copy_exclusive(staging: &Path, destination: &Path) -> Result<(), NewPublicationError> {
    let mut source = File::open(staging)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // Umask can only narrow the staging file's permissions.
        options.mode(source.metadata()?.permissions().mode() & 0o777);
    }
    let mut target = options.open(destination)?;
    #[cfg(test)]
    let fault = simulate::copy_error();
    #[cfg(not(test))]
    let fault: Option<io::Error> = None;
    let result = match fault {
        Some(error) => {
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            {
                match target.write_all(b"partial") {
                    Ok(()) => super::after_exchange_hook::run(destination),
                    Err(error) => return Err(NewPublicationError::IncompleteCopy(error)),
                }
            }
            Err(error)
        }
        None => io::copy(&mut source, &mut target).and_then(|_| target.sync_all()),
    };
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
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
        NoHardLinks(i32),
        /// Neither hard links nor exclusive rename (copy fallback).
        NoAtomicCreate,
    }

    thread_local!(static VOLUME: Cell<Option<Volume>> = const { Cell::new(None) });
    thread_local!(static COPY_FAILURE: Cell<bool> = const { Cell::new(false) });

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) struct CopyFailureGuard;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl Drop for CopyFailureGuard {
        fn drop(&mut self) {
            COPY_FAILURE.with(|flag| flag.set(false));
        }
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn fail_copy() -> CopyFailureGuard {
        COPY_FAILURE.with(|flag| flag.set(true));
        CopyFailureGuard
    }
    pub(super) fn copy_error() -> Option<io::Error> {
        COPY_FAILURE.with(|flag| {
            flag.get()
                .then(|| io::Error::other("injected copy failure"))
        })
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
            Volume::NoAtomicCreate => Some(unsupported()),
        }
    }

    pub(super) fn rename_error() -> Option<io::Error> {
        match VOLUME.with(Cell::get)? {
            Volume::NoHardLinks(_) => None,
            Volume::NoAtomicCreate => Some(unsupported()),
        }
    }
}
