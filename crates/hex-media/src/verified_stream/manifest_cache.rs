//! Bounded process-wide memo of completed verified-WAV page manifests.
//!
//! A manifest is recorded only after an open hashed every page under stable
//! metadata. A later open of the same path whose length, modification time and,
//! on Unix, device/inode still match reuses the whole-file fingerprint and page
//! digests instead of re-reading and re-hashing the file. The reader still
//! verifies every page it serves against these digests, so changed content is
//! never served: an edit that preserves the identity fails as `SourceChanged`
//! when its page is read, rather than at open.
//!
//! The memo holds at most [`MAX_ENTRIES`] manifests and
//! [`MAX_CACHED_DIGEST_BYTES`] of digests, evicting the oldest recorded entry.
//! Its lock is never held during file I/O or hashing. A poisoned lock behaves
//! as an empty memo. It is in-memory only and never saved.
use super::DIGEST_BYTES;
use std::{
    collections::HashMap,
    fs::Metadata,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::SystemTime,
};

/// Largest number of remembered manifests.
pub(super) const MAX_ENTRIES: usize = 1024;
/// Largest total digest payload remembered: 1,024 sources of 64 MiB each.
pub(super) const MAX_CACHED_DIGEST_BYTES: usize = 32 * 1024 * 1024;

type Digest = [u8; DIGEST_BYTES];

/// Identity of the opened file; any difference forces a full re-hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileKey {
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileKey {
    pub(super) fn of(metadata: &Metadata, modified: SystemTime) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }
}

/// A remembered complete manifest.
pub(super) struct Manifest {
    pub(super) fingerprint: Digest,
    pub(super) pages: Arc<[Digest]>,
}

struct Entry {
    key: FileKey,
    fingerprint: Digest,
    pages: Arc<[Digest]>,
    stamp: u64,
}

#[derive(Default)]
struct Memo {
    entries: HashMap<PathBuf, Entry>,
    digest_bytes: usize,
    next_stamp: u64,
}

impl Memo {
    fn remove(&mut self, path: &Path) {
        if let Some(entry) = self.entries.remove(path) {
            self.digest_bytes = self
                .digest_bytes
                .saturating_sub(entry.pages.len() * DIGEST_BYTES);
        }
    }

    fn evict_oldest(&mut self) -> bool {
        let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.stamp)
            .map(|(path, _)| path.clone())
        else {
            return false;
        };
        self.remove(&oldest);
        true
    }

    fn lookup(&mut self, path: &Path, key: FileKey, page_count: usize) -> Option<Manifest> {
        let entry = self.entries.get(path)?;
        if entry.key == key && entry.pages.len() == page_count {
            return Some(Manifest {
                fingerprint: entry.fingerprint,
                pages: Arc::clone(&entry.pages),
            });
        }
        self.remove(path);
        None
    }

    fn record(&mut self, path: &Path, key: FileKey, fingerprint: Digest, pages: Arc<[Digest]>) {
        let bytes = pages.len().saturating_mul(DIGEST_BYTES);
        if bytes > MAX_CACHED_DIGEST_BYTES {
            return;
        }
        self.remove(path);
        while self.entries.len() >= MAX_ENTRIES
            || self.digest_bytes.saturating_add(bytes) > MAX_CACHED_DIGEST_BYTES
        {
            if !self.evict_oldest() {
                break;
            }
        }
        let stamp = self.next_stamp;
        self.next_stamp = stamp.wrapping_add(1);
        self.digest_bytes = self.digest_bytes.saturating_add(bytes);
        self.entries.insert(
            path.to_path_buf(),
            Entry {
                key,
                fingerprint,
                pages,
                stamp,
            },
        );
    }
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Mutex::default);

/// The remembered manifest for `path`, only if its identity and page count
/// still match. A stale entry for the path is dropped.
pub(super) fn lookup(path: &Path, key: FileKey, page_count: usize) -> Option<Manifest> {
    MEMO.lock().ok()?.lookup(path, key, page_count)
}

/// Remember a manifest that an open completely hashed under `key`.
pub(super) fn record(path: &Path, key: FileKey, fingerprint: Digest, pages: &[Digest]) {
    if pages.len().saturating_mul(DIGEST_BYTES) > MAX_CACHED_DIGEST_BYTES {
        return;
    }
    // Copy outside the lock; only the map update runs under it.
    let pages: Arc<[Digest]> = Arc::from(pages);
    if let Ok(mut memo) = MEMO.lock() {
        memo.record(path, key, fingerprint, pages);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(len: u64) -> FileKey {
        FileKey {
            len,
            modified: SystemTime::UNIX_EPOCH,
            #[cfg(unix)]
            device: 1,
            #[cfg(unix)]
            inode: 1,
        }
    }

    #[test]
    fn identity_mismatch_drops_the_entry_and_bounds_evict_oldest_first() {
        let mut memo = Memo::default();
        let path = Path::new("/a.wav");
        memo.record(path, key(1), [1; 32], Arc::from([[2; 32]].as_slice()));
        assert!(
            memo.lookup(path, key(1), 2).is_none(),
            "page count must match"
        );
        memo.record(path, key(1), [1; 32], Arc::from([[2; 32]].as_slice()));
        assert_eq!(
            memo.lookup(path, key(1), 1).map(|m| m.fingerprint),
            Some([1; 32])
        );
        assert!(memo.lookup(path, key(2), 1).is_none());
        assert!(memo.entries.is_empty() && memo.digest_bytes == 0);

        for index in 0..=MAX_ENTRIES {
            let path = PathBuf::from(format!("/{index}.wav"));
            memo.record(&path, key(1), [0; 32], Arc::from([[0; 32]].as_slice()));
        }
        assert_eq!(memo.entries.len(), MAX_ENTRIES);
        assert!(!memo.entries.contains_key(Path::new("/0.wav")));
        assert!(memo.entries.contains_key(Path::new("/1.wav")));
        assert_eq!(memo.digest_bytes, MAX_ENTRIES * DIGEST_BYTES);
    }
}
