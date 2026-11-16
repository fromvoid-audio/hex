//! Naming and live-writer custody of hidden export staging files.
//!
//! A filename, process ID and available advisory lock do not prove ownership:
//! automatic stale-file cleanup is suspended, and unclaimed files remain for
//! deliberate inspection. Older running hex versions can still sweep matching
//! files. When the writer's advisory lock is available, keep it while rendering
//! and publishing so those versions cannot remove the active staging file.
//! Lock acquisition can fail; it is not a guarantee against older versions.
use std::{fs::File, io};

const PREFIX: &str = ".hex-export-";
const SUFFIX: &str = ".tmp";

/// Historical pre-export cleanup counts, retained for the export report API.
///
/// Automatic stale-file cleanup is suspended, so both counts are zero. Files
/// from interrupted exports or publication conflicts remain for inspection.
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

/// Keep the writer's lock for as long as the returned handle lives.
pub(crate) fn hold(file: &File) -> Option<File> {
    let held = file.try_clone().ok()?;
    lock(&held).ok().filter(|locked| *locked).map(|_| held)
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
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn live_writer_lock_blocks_older_cooperating_sweeper() {
        let path = std::env::temp_dir().join(format!(
            "hex-export-lock-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let held = hold(&file).expect("flock on the test file system");
        let other = File::open(&path).unwrap();
        assert!(!lock(&other).unwrap(), "live writer must remain protected");
        drop(held);
        drop(file);
        assert!(
            lock(&other).unwrap(),
            "released writer must not retain the lock"
        );
        drop(other);
        fs::remove_file(path).unwrap();
    }
}
