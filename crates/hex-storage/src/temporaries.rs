//! Best-effort removal of save temporaries leaked by an interrupted process.
//!
//! Publication writes complete copies under exactly two hidden name patterns in
//! the project's directory, `.hex-pending-<pid>-<n>` (new projects, see
//! `files.rs`) and `.hex-r-<pid>-<n>` (replacement project/recovery copies,
//! journal checkpoints and automation drafts, via `replacement::prepare_temp`). A crash between writing one and
//! removing it leaves a full project copy behind. After a successful save, this
//! sweep removes such files when all of these hold:
//! - the name matches one pattern exactly, with decimal `<pid>` and `<n>`;
//! - `<pid>` is not this process, whose temporaries may still be in use;
//! - the entry is a regular file, never a symbolic link or directory;
//! - it was last modified at least [`STALE_AFTER`] ago, far longer than any
//!   save takes, so another running hex process cannot be mid-publication.
//!
//! Recovery sidecars (`.<name>.hex-recovery`), journals, locks and every other
//! file never match. Removing a leaked temporary only unlinks that name: if it
//! was already hard-linked into place, the published file keeps its data.
use std::{
    fs, io,
    path::Path,
    time::{Duration, SystemTime},
};

/// Minimum age before a foreign temporary counts as leaked.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(60 * 60);
/// Bound the directory scan so a huge folder cannot stall a save.
const MAX_SCANNED_ENTRIES: usize = 4096;
const PREFIXES: [&str; 2] = [".hex-pending-", ".hex-r-"];

/// Remove leaked temporaries in `directory`; returns how many were removed.
/// Failures to read or remove an individual entry are skipped, never reported
/// as a save failure.
pub(crate) fn sweep_stale_temporaries(directory: &Path) -> io::Result<usize> {
    sweep_at(directory, SystemTime::now(), std::process::id())
}

fn sweep_at(directory: &Path, now: SystemTime, own_pid: u32) -> io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(directory)?.take(MAX_SCANNED_ENTRIES) {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(temporary_pid) else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_AFTER);
        if stale && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// The writer pid of an exact temporary name, or `None` for any other name.
fn temporary_pid(name: &str) -> Option<u32> {
    let rest = PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))?;
    let (pid, counter) = rest.split_once('-')?;
    let decimal = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !decimal(pid) || !decimal(counter) {
        return None;
    }
    pid.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Dir(std::path::PathBuf);
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
        fn file(&self, name: &str) -> std::path::PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"copy").expect("fixture file");
            path
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_stale_foreign_regular_temporaries_are_removed() {
        let dir = Dir::new();
        let own = 4242;
        let stale_pending = dir.file(".hex-pending-17-3");
        let stale_replacement = dir.file(".hex-r-17-4");
        let kept = [
            dir.file(".hex-pending-4242-1"),
            dir.file(".song.hex.hex-recovery"),
            dir.file(".song.hex.hex-journal"),
            dir.file(".song.hex.hex-journal.lock"),
            dir.file(".song.hex.hex-recovery.moved-1"),
            dir.file(".hex-pending-17"),
            dir.file(".hex-pending-x-1"),
            dir.file(".hex-r-17-4-5"),
            dir.file(".hex-preset-pending-17-1"),
            dir.file("song.hex"),
        ];
        let directory = dir.0.join(".hex-r-18-1");
        fs::create_dir(&directory).expect("directory fixture");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&kept[9], dir.0.join(".hex-pending-19-1")).expect("symlink");

        let fresh = SystemTime::now();
        assert_eq!(sweep_at(&dir.0, fresh, own).expect("fresh sweep"), 0);
        assert!(stale_pending.exists() && stale_replacement.exists());

        let later = fresh + STALE_AFTER + Duration::from_secs(1);
        assert_eq!(sweep_at(&dir.0, later, own).expect("stale sweep"), 2);
        assert!(!stale_pending.exists() && !stale_replacement.exists());
        for path in &kept {
            assert!(path.exists(), "{path:?} must be preserved");
        }
        assert!(directory.is_dir());
        #[cfg(unix)]
        assert!(fs::symlink_metadata(dir.0.join(".hex-pending-19-1")).is_ok());
    }

    #[test]
    fn save_and_replace_sweep_a_stale_leaked_temporary() {
        let dir = Dir::new();
        let target = dir.0.join("song.hex");
        let project = hex_project::Project::new(48_000).expect("project");
        let mut leaked = Vec::new();
        for name in [".hex-pending-1-1", ".hex-r-1-2"] {
            let path = dir.file(name);
            fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(SystemTime::now() - STALE_AFTER * 2))
                .expect("age fixture");
            leaked.push(path);
        }
        crate::save_new(&target, &project).expect("save new");
        assert!(leaked.iter().all(|path| !path.exists()));

        let again = dir.file(".hex-r-1-3");
        fs::File::options()
            .write(true)
            .open(&again)
            .and_then(|file| file.set_modified(SystemTime::now() - STALE_AFTER * 2))
            .expect("age fixture");
        let (_, token) = crate::load_with_token(&target).expect("token");
        crate::replace_existing(&target, &token, &project).expect("replace");
        assert!(!again.exists());
        assert!(crate::recovery_path(&target).expect("sidecar").exists());
    }
}
