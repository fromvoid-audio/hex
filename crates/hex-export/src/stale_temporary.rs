//! Ownership and cleanup of the hidden `.hex-export-{pid}-{n}.tmp` staging
//! files written beside a WAV destination.
//!
//! A writer holds a non-blocking exclusive advisory lock (`flock`) on its
//! temporary for the whole render-and-publish operation. A later export sweeps
//! the destination directory before staging. It removes an exactly named
//! temporary only when the name carries a different process ID and the sweep
//! itself can take that lock, which proves no live writer still owns it. Lock
//! ownership, not PID probing, is the liveness proof, so a reused PID cannot
//! keep a stale file alive or expose a live one. Locking needs only the `fs`
//! APIs already used by this crate.
//!
//! Limits: a file locked by a live writer, a file from this process, a
//! non-regular file and every other name are kept. A temporary staged by an
//! older hex build that never took the lock is indistinguishable from an
//! abandoned one. A failed lock or removal is counted and never blocks the
//! export. Platforms without `flock` do not sweep.
use std::{
    fs::{self, File},
    io,
    path::Path,
};

const PREFIX: &str = ".hex-export-";
const SUFFIX: &str = ".tmp";
/// Bounds the directory scan so one export never walks an unbounded listing.
const MAX_SCANNED_ENTRIES: usize = 65_536;

/// Outcome of the pre-export sweep, reported with a successful export.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaleTemporaries {
    /// Abandoned temporaries removed from the destination directory.
    pub removed: u32,
    /// Matching temporaries that could not be inspected, locked or removed.
    pub failed: u32,
}

pub(crate) fn name(pid: u32, identifier: u64) -> String {
    format!("{PREFIX}{pid}-{identifier}{SUFFIX}")
}

/// The owning process ID of an exact temporary name, or `None` for any other name.
fn owner(name: &str) -> Option<u32> {
    let (pid, identifier) = name
        .strip_prefix(PREFIX)?
        .strip_suffix(SUFFIX)?
        .split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(pid) || !digits(identifier) {
        return None;
    }
    identifier.parse::<u64>().ok()?;
    pid.parse().ok()
}

/// Keep the writer's lock for as long as the returned handle lives. Without
/// `flock` support the temporary stays unlocked, and a sweep on that file
/// system cannot take the lock either, so it never removes the file.
pub(crate) fn hold(file: &File) -> Option<File> {
    let held = file.try_clone().ok()?;
    lock(&held).ok().filter(|locked| *locked).map(|_| held)
}

/// Remove abandoned temporaries in the destination's directory.
pub(crate) fn sweep(destination: &Path) -> StaleTemporaries {
    let directory = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    sweep_directory(directory, std::process::id())
}

fn sweep_directory(directory: &Path, own_pid: u32) -> StaleTemporaries {
    let mut outcome = StaleTemporaries::default();
    if !cfg!(unix) {
        return outcome;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        // An unreadable directory also fails the export's own staging step.
        return outcome;
    };
    for entry in entries.take(MAX_SCANNED_ENTRIES) {
        let Ok(entry) = entry else {
            outcome.failed = outcome.failed.saturating_add(1);
            continue;
        };
        let file_name = entry.file_name();
        let Some(pid) = file_name.to_str().and_then(owner) else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        match remove_if_abandoned(&entry.path()) {
            Ok(true) => outcome.removed = outcome.removed.saturating_add(1),
            Ok(false) => {}
            Err(_) => outcome.failed = outcome.failed.saturating_add(1),
        }
    }
    outcome
}

fn remove_if_abandoned(path: &Path) -> io::Result<bool> {
    // Never follow a link or touch anything but a regular file.
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Ok(false);
    }
    let file = File::open(path)?;
    if !lock(&file)? {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

/// `Ok(true)` once this handle holds the exclusive lock, `Ok(false)` when a
/// live owner already holds it.
#[cfg(unix)]
fn lock(file: &File) -> io::Result<bool> {
    use rustix::fs::{FlockOperation, flock};
    match flock(file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(unix))]
fn lock(_: &File) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "temporary ownership locks are unavailable",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-export-sweep-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"staged").unwrap();
            path
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exact_names_parse_their_owner_and_everything_else_is_ignored() {
        assert_eq!(owner(&name(42, 7)), Some(42));
        for other in [
            ".hex-export-42-7.tmp.bak",
            ".hex-export-42.tmp",
            ".hex-export--7.tmp",
            ".hex-export-4a-7.tmp",
            ".hex-export-+42-7.tmp",
            ".hex-export-42-7-1.tmp",
            "hex-export-42-7.tmp",
            ".hex-export-99999999999-7.tmp",
            "song.wav",
        ] {
            assert_eq!(owner(other), None, "{other}");
        }
    }

    #[test]
    fn sweep_removes_abandoned_and_keeps_owned_own_and_unrelated_files() {
        let dir = Dir::new();
        let own = 4_000_000;
        let abandoned = dir.file(&name(own + 1, 1));
        let live = dir.file(&name(own + 2, 1));
        let held = hold(&File::open(&live).unwrap()).expect("flock on the test file system");
        let mine = dir.file(&name(own, 3));
        let unrelated = [
            dir.file("song.wav"),
            dir.file(".hex-export-12-3.tmp.bak"),
            dir.file(".hex-export-abc-1.tmp"),
        ];
        fs::create_dir(dir.0.join(name(own + 3, 1))).unwrap();
        let outcome = sweep_directory(&dir.0, own);
        assert_eq!(
            outcome,
            StaleTemporaries {
                removed: 1,
                failed: 0
            }
        );
        assert!(!abandoned.exists());
        assert!(live.exists(), "a live writer's lock keeps its temporary");
        assert!(mine.exists(), "this process's temporaries are never swept");
        assert!(unrelated.iter().all(|path| path.exists()));
        assert!(dir.0.join(name(own + 3, 1)).is_dir());
        drop(held);
        assert_eq!(
            sweep_directory(&dir.0, own),
            StaleTemporaries {
                removed: 1,
                failed: 0
            },
            "released by its writer, the same file is abandoned"
        );
        assert!(!live.exists());
    }

    #[test]
    fn missing_directory_sweeps_nothing() {
        let dir = Dir::new();
        assert_eq!(
            sweep_directory(&dir.0.join("absent"), 1),
            StaleTemporaries::default()
        );
    }
}
