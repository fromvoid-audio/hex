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
//!    a replaced one. A failed write preserves the current destination pathname
//!    entry (because even an exclusively created pathname can have been
//!    replaced before failure cleanup) and returns the error.
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

/// Atomically claim an absent `destination` directory and publish `staging`'s
/// contents without replacing any existing file or directory.
pub(crate) fn rename_directory_exclusive(staging: &Path, destination: &Path) -> io::Result<()> {
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
            "exclusive directory rename is unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
fn publish_directory_exclusive_windows(staging: &Path, destination: &Path) -> io::Result<()> {
    use std::path::PathBuf;
    const MAX_DEPTH: usize = 32;
    const MAX_ENTRIES: usize = 16_384;

    let meta = fs::symlink_metadata(staging)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "staged project folder must be a real directory",
        ));
    }

    // Collect the staged tree before touching `destination` so symlinks or
    // malformed entries fail before creating any destination directory.
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut marker: Option<PathBuf> = None;
    let mut pending = vec![(PathBuf::new(), 0usize)];
    let mut entries_seen = 0usize;
    while let Some((relative, depth)) = pending.pop() {
        let current = if relative.as_os_str().is_empty() {
            staging.to_path_buf()
        } else {
            staging.join(&relative)
        };
        for entry in fs::read_dir(&current)? {
            entries_seen += 1;
            if entries_seen > MAX_ENTRIES {
                return Err(io::Error::other(
                    "staged project folder entry bound exceeded",
                ));
            }
            let entry = entry?;
            let name = entry.file_name();
            let child_rel = if relative.as_os_str().is_empty() {
                PathBuf::from(&name)
            } else {
                relative.join(&name)
            };
            let child_path = entry.path();
            let kind = fs::symlink_metadata(&child_path)?.file_type();
            if kind.is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "staged project folder cannot contain symbolic links",
                ));
            }
            if kind.is_dir() {
                if depth >= MAX_DEPTH {
                    return Err(io::Error::other(
                        "staged project folder depth bound exceeded",
                    ));
                }
                dirs.push(child_rel.clone());
                pending.push((child_rel, depth + 1));
            } else if kind.is_file() {
                if relative.as_os_str().is_empty() && name == ".hex-project" {
                    marker = Some(child_rel);
                } else {
                    files.push(child_rel);
                }
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "staged project folder contains an unsupported entry",
                ));
            }
        }
    }

    // `create_dir` on Windows atomically fails with `AlreadyExists` if any
    // file, symlink, empty directory, or non-empty directory already exists at
    // `destination` (unlike `fs::rename`, which passes MOVEFILE_REPLACE_EXISTING).
    fs::create_dir(destination)?;
    for rel_dir in &dirs {
        fs::create_dir(destination.join(rel_dir))?;
    }
    let mut staged_to_unlink = Vec::with_capacity(files.len() + usize::from(marker.is_some()));
    for rel_file in &files {
        let src = staging.join(rel_file);
        let dst = destination.join(rel_file);
        let published = publish_new(&src, &dst)?;
        if published.staging_remains() {
            staged_to_unlink.push(src);
        }
    }
    if let Some(rel_marker) = &marker {
        let src = staging.join(rel_marker);
        let dst = destination.join(rel_marker);
        let published = publish_new(&src, &dst)?;
        if published.staging_remains() {
            staged_to_unlink.push(src);
        }
    }
    // Destination is now completely published. Remove only the staged entries
    // we created (never `remove_dir_all`).
    for path in staged_to_unlink {
        let _ = fs::remove_file(path);
    }
    for rel_dir in dirs.iter().rev() {
        let _ = fs::remove_dir(staging.join(rel_dir));
    }
    let _ = fs::remove_dir(staging);
    Ok(())
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
    // Even an exclusively created pathname can have been replaced since opening
    // it. Preserve the current entry on copy/sync failure instead of unlinking
    // by path.
    #[cfg(test)]
    if let Some(error) = simulate::on_copy_after_open(destination) {
        return Err(error);
    }
    io::copy(&mut source, &mut target)?;
    target.sync_all()
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
    use std::{
        cell::{Cell, RefCell},
        io,
        path::Path,
    };

    type CopyHook = Box<dyn FnOnce(&Path) -> io::Error>;

    #[derive(Clone, Copy)]
    pub(crate) enum Volume {
        /// Hard links fail with this errno; exclusive rename works.
        #[cfg_attr(not(unix), allow(dead_code))]
        NoHardLinks(i32),
        /// Neither hard links nor exclusive rename exist (copy fallback).
        NoAtomicCreate,
    }

    thread_local!(static VOLUME: Cell<Option<Volume>> = const { Cell::new(None) });
    thread_local!(static COPY_HOOK: RefCell<Option<CopyHook>> = const { RefCell::new(None) });

    /// Restores native behaviour on drop, including after a failed assertion.
    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            VOLUME.with(|volume| volume.set(None));
            COPY_HOOK.with(|hook| *hook.borrow_mut() = None);
        }
    }

    pub(crate) fn volume(volume: Volume) -> Guard {
        VOLUME.with(|cell| cell.set(Some(volume)));
        Guard
    }

    #[cfg(unix)]
    pub(super) fn fail_copy_after_open(hook: impl FnOnce(&Path) -> io::Error + 'static) -> Guard {
        VOLUME.with(|cell| cell.set(Some(Volume::NoAtomicCreate)));
        COPY_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        Guard
    }

    pub(super) fn on_copy_after_open(destination: &Path) -> Option<io::Error> {
        COPY_HOOK
            .with(|slot| slot.borrow_mut().take())
            .map(|hook| hook(destination))
    }

    fn unsupported_error() -> io::Error {
        #[cfg(unix)]
        {
            io::Error::from_raw_os_error(libc::ENOTSUP)
        }
        #[cfg(not(unix))]
        {
            io::Error::new(io::ErrorKind::Unsupported, "simulated unsupported volume")
        }
    }

    pub(super) fn link_error() -> Option<io::Error> {
        match VOLUME.with(Cell::get)? {
            Volume::NoHardLinks(code) => Some(io::Error::from_raw_os_error(code)),
            Volume::NoAtomicCreate => Some(unsupported_error()),
        }
    }

    #[cfg_attr(
        not(any(target_os = "linux", target_os = "macos")),
        expect(dead_code, reason = "rename is natively unsupported here")
    )]
    pub(super) fn rename_error() -> Option<io::Error> {
        match VOLUME.with(Cell::get)? {
            Volume::NoHardLinks(_) => None,
            Volume::NoAtomicCreate => Some(unsupported_error()),
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

    #[test]
    fn failed_copy_exclusive_preserves_concurrently_replaced_destination() {
        let dir = Dir::new();
        let staging = dir.staged(b"staged-project-bytes");
        let destination = dir.0.join("saved.hex");
        let replacement = dir.staged(b"competing-user-file");
        let _guard = simulate::fail_copy_after_open(move |dest| {
            fs::rename(&replacement, dest).expect("concurrent replacement");
            io::Error::other("simulated disk full during copy")
        });

        let error = publish_new(&staging, &destination).expect_err("copy must fail");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            fs::read(&destination).expect("replaced destination must survive"),
            b"competing-user-file"
        );
        assert_eq!(
            fs::read(&staging).expect("staging file preserved"),
            b"staged-project-bytes"
        );
    }
}

#[cfg(all(test, not(unix)))]
mod cross_platform_tests {
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
                std::env::temp_dir().join(format!("hex-publish-xplat-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("test directory");
            Self(path)
        }
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
    fn publish_new_links_or_copies_without_replacing_and_atomic_refuses_copy() {
        for (capability, expected) in [
            (None, Published::Linked),
            (Some(Volume::NoAtomicCreate), Published::Copied),
        ] {
            let dir = Dir::new();
            let _guard = capability.map(volume);
            let staging = dir.staged(b"complete");
            let destination = dir.0.join("saved");
            let published = publish_new(&staging, &destination).expect("published");
            assert_eq!(published, expected);
            assert_eq!(fs::read(&destination).expect("destination"), b"complete");
            assert_eq!(staging.exists(), published.staging_remains());

            let newer = dir.staged(b"newer");
            let error = publish_new(&newer, &destination).expect_err("existing destination");
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&destination).expect("untouched"), b"complete");
        }

        let dir = Dir::new();
        let _guard = volume(Volume::NoAtomicCreate);
        let staging = dir.staged(b"complete");
        let destination = dir.0.join("draft");
        let error = publish_new_atomic(&staging, &destination).expect_err("refused");
        assert!(rename_unsupported(&error));
        assert!(!destination.exists());
        assert!(staging.exists());
    }

    #[cfg(windows)]
    #[test]
    fn rename_directory_exclusive_refuses_existing_empty_dir_nonempty_dir_and_file() {
        let dir = Dir::new();
        let stage = dir.0.join(".stage-dir");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("marker"), b"staged").unwrap();

        let existing_file = dir.0.join("existing-file");
        fs::write(&existing_file, b"keep-file").unwrap();
        assert!(rename_directory_exclusive(&stage, &existing_file).is_err());
        assert_eq!(fs::read(&existing_file).unwrap(), b"keep-file");
        assert!(stage.join("marker").is_file());

        let existing_empty = dir.0.join("existing-empty");
        fs::create_dir(&existing_empty).unwrap();
        assert!(rename_directory_exclusive(&stage, &existing_empty).is_err());
        assert!(fs::read_dir(&existing_empty).unwrap().next().is_none());
        assert!(stage.join("marker").is_file());

        let existing_nonempty = dir.0.join("existing-nonempty");
        fs::create_dir(&existing_nonempty).unwrap();
        fs::write(existing_nonempty.join("keep"), b"keep-dir").unwrap();
        assert!(rename_directory_exclusive(&stage, &existing_nonempty).is_err());
        assert_eq!(
            fs::read(existing_nonempty.join("keep")).unwrap(),
            b"keep-dir"
        );
        assert!(stage.join("marker").is_file());

        // Non-directory source must be refused so file-over-file rename cannot occur.
        let regular_source = dir.staged(b"not-a-dir");
        assert!(rename_directory_exclusive(&regular_source, &existing_file).is_err());
        assert_eq!(fs::read(&existing_file).unwrap(), b"keep-file");

        // Absent destination succeeds atomically.
        let target = dir.0.join("published-dir");
        rename_directory_exclusive(&stage, &target).unwrap();
        assert!(!stage.exists());
        assert_eq!(fs::read(target.join("marker")).unwrap(), b"staged");
    }
}
