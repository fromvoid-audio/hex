//! Create-new publication of a fully written, synced staging file.
//!
//! [`publish_new`] makes a staging file visible at a destination that must not
//! exist and never replaces an existing file. It tries, in order:
//!
//! 1. A hard link. It is atomic and leaves the staging name in place.
//! 2. On volumes without hard links (exFAT/FAT drives and many SMB shares
//!    return `ENOTSUP` or `EPERM`), an exclusive no-replace rename
//!    (`RENAME_EXCL` on macOS, `RENAME_NOREPLACE` on Linux). It is atomic and
//!    consumes the staging name.
//! 3. When the volume supports neither, an exclusively created, fully written
//!    and synced copy. The copy is not atomic: a crash or power loss while it
//!    is written can leave an incomplete *new* file at the destination, never
//!    a replaced one. A failed write removes the incomplete copy it created
//!    and returns the error.
//!
//! Every step refuses an existing destination with `AlreadyExists`. Other link
//! failures (for example `EACCES`, `ENOSPC`, `EXDEV`) propagate unchanged
//! instead of falling back. Callers keep removing a remaining staging name
//! (see [`Published::staging_remains`]) and synchronizing the parent directory.
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

/// How a staging file became visible at its destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Published {
    /// The destination is a second link; the staging name still exists.
    Linked,
    /// The staging file moved to the destination; its name no longer exists.
    Renamed,
    /// The destination is an exclusive synced copy; the staging name remains.
    Copied,
}

impl Published {
    pub(crate) fn staging_remains(self) -> bool {
        !matches!(self, Self::Renamed)
    }
}

/// Publish `staging` at the absent `destination`, falling back to a
/// non-atomic exclusive copy only when no atomic create-new step exists.
pub(crate) fn publish_new(staging: &Path, destination: &Path) -> io::Result<Published> {
    match publish_new_atomic(staging, destination) {
        Err(error) if rename_unsupported(&error) => {
            copy_exclusive(staging, destination)?;
            Ok(Published::Copied)
        }
        result => result,
    }
}

/// Publish using only atomic steps, for owners whose open staging handle must
/// remain bound to the published file. Volumes without either step refuse.
pub(crate) fn publish_new_atomic(staging: &Path, destination: &Path) -> io::Result<Published> {
    match link(staging, destination) {
        Ok(()) => Ok(Published::Linked),
        Err(error) if link_unsupported(&error) => {
            rename_exclusive(staging, destination)?;
            Ok(Published::Renamed)
        }
        Err(error) => Err(error),
    }
}

fn link(staging: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = simulate::link_error() {
        return Err(error);
    }
    fs::hard_link(staging, destination)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn rename_exclusive(staging: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = simulate::rename_error() {
        return Err(error);
    }
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        staging,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn rename_exclusive(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "exclusive rename is unsupported on this platform",
    ))
}

fn copy_exclusive(staging: &Path, destination: &Path) -> io::Result<()> {
    let mut source = File::open(staging)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // Keep private staging files private; umask can only narrow this.
        options.mode(source.metadata()?.permissions().mode() & 0o777);
    }
    let mut target = options.open(destination)?;
    if let Err(error) = io::copy(&mut source, &mut target).and_then(|_| target.sync_all()) {
        drop(target);
        // This exclusive create made the incomplete file; nothing else is removed.
        let _ = fs::remove_file(destination);
        return Err(error);
    }
    Ok(())
}

fn link_unsupported(error: &io::Error) -> bool {
    #[cfg(unix)]
    if let Some(code) = error.raw_os_error() {
        return [libc::ENOTSUP, libc::EOPNOTSUPP, libc::EPERM, libc::ENOSYS].contains(&code);
    }
    error.kind() == io::ErrorKind::Unsupported
}

fn rename_unsupported(error: &io::Error) -> bool {
    #[cfg(unix)]
    if let Some(code) = error.raw_os_error() {
        // Linux reports renameat2 flags a filesystem lacks as EINVAL.
        return [libc::ENOTSUP, libc::EOPNOTSUPP, libc::EINVAL, libc::ENOSYS].contains(&code);
    }
    error.kind() == io::ErrorKind::Unsupported
}

/// Test-only per-thread volume capabilities, injected at this owner boundary
/// so every publishing owner can be exercised without a real exFAT mount.
#[cfg(test)]
pub(crate) mod simulate {
    use std::{cell::Cell, io};

    #[derive(Clone, Copy)]
    pub(crate) enum Volume {
        /// Hard links fail with this errno; exclusive rename works.
        NoHardLinks(i32),
        /// Neither hard links nor exclusive rename exist (copy fallback).
        NoAtomicCreate,
    }

    thread_local!(static VOLUME: Cell<Option<Volume>> = const { Cell::new(None) });

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

    pub(super) fn link_error() -> Option<io::Error> {
        match VOLUME.with(Cell::get)? {
            Volume::NoHardLinks(code) => Some(io::Error::from_raw_os_error(code)),
            Volume::NoAtomicCreate => Some(io::Error::from_raw_os_error(libc::ENOTSUP)),
        }
    }

    #[cfg_attr(
        not(any(target_os = "linux", target_os = "macos")),
        expect(dead_code, reason = "rename is natively unsupported here")
    )]
    pub(super) fn rename_error() -> Option<io::Error> {
        match VOLUME.with(Cell::get)? {
            Volume::NoHardLinks(_) => None,
            Volume::NoAtomicCreate => Some(io::Error::from_raw_os_error(libc::ENOTSUP)),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::simulate::{Volume, volume};
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("hex-publish-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("test directory");
            Self(path)
        }
        /// A fresh staging name each call: a linked destination shares its inode.
        fn staged(&self, bytes: &[u8]) -> PathBuf {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = self.0.join(format!(".staging-{id}"));
            fs::write(&path, bytes).expect("staging file");
            path
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn every_volume_capability_publishes_complete_bytes_without_replacing() {
        let cases: [(Option<Volume>, Published); 4] = [
            (None, Published::Linked),
            (Some(Volume::NoHardLinks(libc::ENOTSUP)), Published::Renamed),
            (Some(Volume::NoHardLinks(libc::EPERM)), Published::Renamed),
            (Some(Volume::NoAtomicCreate), Published::Copied),
        ];
        for (capability, expected) in cases {
            let dir = Dir::new();
            let _guard = capability.map(volume);
            let staging = dir.staged(b"complete");
            let destination = dir.0.join("saved");
            let published = publish_new(&staging, &destination).expect("published");
            assert_eq!(published, expected);
            assert_eq!(fs::read(&destination).expect("destination"), b"complete");
            assert_eq!(staging.exists(), published.staging_remains());

            let staging = dir.staged(b"newer");
            let error = publish_new(&staging, &destination).expect_err("existing destination");
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&destination).expect("untouched"), b"complete");
            assert_eq!(fs::read(&staging).expect("staging kept"), b"newer");
        }
    }

    #[test]
    fn other_link_failures_propagate_without_fallback() {
        let dir = Dir::new();
        let _guard = volume(Volume::NoHardLinks(libc::EACCES));
        let staging = dir.staged(b"complete");
        let destination = dir.0.join("saved");
        let error = publish_new(&staging, &destination).expect_err("refused");
        assert_eq!(error.raw_os_error(), Some(libc::EACCES));
        assert!(!destination.exists());
        assert!(staging.exists());
    }

    #[test]
    fn atomic_only_publication_refuses_instead_of_copying() {
        let dir = Dir::new();
        let _guard = volume(Volume::NoAtomicCreate);
        let staging = dir.staged(b"complete");
        let destination = dir.0.join("draft");
        let error = publish_new_atomic(&staging, &destination).expect_err("refused");
        assert!(rename_unsupported(&error));
        assert!(!destination.exists());
        assert!(staging.exists());
    }

    #[test]
    fn copy_keeps_private_staging_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Dir::new();
        let _guard = volume(Volume::NoAtomicCreate);
        let staging = dir.staged(b"private");
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o600)).expect("chmod");
        let destination = dir.0.join("recovery");
        publish_new(&staging, &destination).expect("copied");
        let mode = fs::metadata(&destination)
            .expect("copy")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }
}
