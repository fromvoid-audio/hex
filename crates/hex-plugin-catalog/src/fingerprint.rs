//! In-memory `stat`-fingerprint fast path for verifying unchanged plugin bundles
//! without re-hashing multi-MB Mach-O binaries from disk on every refresh or
//! incremental scan.
//!
//! Mirrors `hex-plugin-host/src/native/source.rs`: verifies non-symlink canonical
//! bundle path, non-symlink intermediate components, and exact `(dev, ino, len,
//! mtime, ctime)` metadata before trusting a prior SHA-256 verification. Any
//! metadata change, symlink introduction, or cache miss falls back to full
//! SHA-256 revalidation so tampering or updates are always detected.

use crate::{ClapBundleIdentity, MAX_MODULES};
use hex_plugin_scan::macos::identity::{Bundle, Executable};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static VST3_FULL_HASH_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static CLAP_FULL_HASH_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStatFingerprint {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl FileStatFingerprint {
    #[cfg(unix)]
    fn capture_regular_no_symlink(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(path).ok()?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return None;
        }
        Some(Self {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_nsec: meta.ctime_nsec(),
        })
    }

    #[cfg(not(unix))]
    fn capture_regular_no_symlink(_path: &Path) -> Option<Self> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CachedSource {
    Vst3 {
        canonical_path: PathBuf,
        info_sha256: String,
        executable_relative_path: String,
        executable_sha256: String,
        info: FileStatFingerprint,
        executable: FileStatFingerprint,
    },
    Clap {
        canonical_path: PathBuf,
        info_plist_sha256: Option<String>,
        executable_relative_path: String,
        executable_sha256: String,
        info: Option<FileStatFingerprint>,
        executable: FileStatFingerprint,
    },
}

const MAX_CACHE_ENTRIES: usize = MAX_MODULES * 2;

fn cache() -> &'static Mutex<BTreeMap<PathBuf, CachedSource>> {
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, CachedSource>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn canonical_bundle_dir_no_symlink(
    supplied_path: &Path,
    expected_canonical: &Path,
) -> Option<PathBuf> {
    let meta = fs::symlink_metadata(supplied_path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return None;
    }
    let current = fs::canonicalize(supplied_path).ok()?;
    (current == expected_canonical).then_some(current)
}

fn stat_relative_member(root: &Path, relative: &str) -> Option<FileStatFingerprint> {
    if relative.is_empty() || relative.contains('\0') {
        return None;
    }
    let mut path = root.to_path_buf();
    let components: Vec<_> = Path::new(relative).components().collect();
    if components.is_empty() || components.len() > 32 {
        return None;
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(segment) = component else {
            return None;
        };
        path.push(segment);
        let is_last = index + 1 == components.len();
        if is_last {
            return FileStatFingerprint::capture_regular_no_symlink(&path);
        }
        let meta = fs::symlink_metadata(&path).ok()?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return None;
        }
    }
    None
}

fn insert_cache(path: PathBuf, entry: CachedSource) {
    if let Ok(mut guard) = cache().lock() {
        if guard.len() >= MAX_CACHE_ENTRIES && !guard.contains_key(&path) {
            let _ = guard.pop_first();
        }
        guard.insert(path, entry);
    }
}

fn remove_cache(path: &Path) {
    if let Ok(mut guard) = cache().lock() {
        guard.remove(path);
    }
}

/// Record a freshly verified VST3 bundle/executable stat fingerprint in the
/// in-memory cache (called after full SHA-256 verification succeeds).
pub(crate) fn record_vst3_verified(bundle: &Bundle, executable: &Executable) {
    let Some(canonical) =
        canonical_bundle_dir_no_symlink(&bundle.supplied_path, &bundle.canonical_path)
    else {
        return;
    };
    let Some(info) = stat_relative_member(&canonical, "Contents/Info.plist") else {
        return;
    };
    let Some(exec) = stat_relative_member(&canonical, &executable.relative_path) else {
        return;
    };
    insert_cache(
        bundle.supplied_path.clone(),
        CachedSource::Vst3 {
            canonical_path: canonical,
            info_sha256: bundle.info_sha256.clone(),
            executable_relative_path: executable.relative_path.clone(),
            executable_sha256: executable.sha256.clone(),
            info,
            executable: exec,
        },
    );
}

/// Verify a cataloged VST3 bundle/executable using the O(1) `stat`-fingerprint
/// fast path first, falling back to full SHA-256 `executable_unchanged` on any
/// cache miss or metadata change.
pub(crate) fn verify_vst3_unchanged(
    bundle: &Bundle,
    executable: &Executable,
) -> Result<(), String> {
    if let Ok(guard) = cache().lock()
        && let Some(CachedSource::Vst3 {
            canonical_path,
            info_sha256,
            executable_relative_path,
            executable_sha256,
            info,
            executable: cached_exec,
        }) = guard.get(&bundle.supplied_path)
        && canonical_path == &bundle.canonical_path
        && info_sha256 == &bundle.info_sha256
        && executable_relative_path == &executable.relative_path
        && executable_sha256 == &executable.sha256
    {
        let expected_info = *info;
        let expected_exec = *cached_exec;
        drop(guard);
        if let Some(current) =
            canonical_bundle_dir_no_symlink(&bundle.supplied_path, &bundle.canonical_path)
            && stat_relative_member(&current, "Contents/Info.plist") == Some(expected_info)
            && stat_relative_member(&current, &executable.relative_path) == Some(expected_exec)
        {
            return Ok(());
        }
    }

    #[cfg(test)]
    VST3_FULL_HASH_COUNT.fetch_add(1, Ordering::Relaxed);

    match bundle.executable_unchanged(executable) {
        Ok(()) => {
            record_vst3_verified(bundle, executable);
            Ok(())
        }
        Err(error) => {
            remove_cache(&bundle.supplied_path);
            Err(error.to_string())
        }
    }
}

/// Record a freshly verified CLAP bundle stat fingerprint in the in-memory cache.
pub(crate) fn record_clap_verified(identity: &ClapBundleIdentity) {
    let Some(canonical) =
        canonical_bundle_dir_no_symlink(&identity.supplied_path, &identity.canonical_path)
    else {
        return;
    };
    let info = if identity.info_plist_sha256.is_some() {
        let Some(fp) = stat_relative_member(&canonical, "Contents/Info.plist") else {
            return;
        };
        Some(fp)
    } else {
        None
    };
    let Some(exec) = stat_relative_member(&canonical, &identity.executable_relative_path) else {
        return;
    };
    insert_cache(
        identity.supplied_path.clone(),
        CachedSource::Clap {
            canonical_path: canonical,
            info_plist_sha256: identity.info_plist_sha256.clone(),
            executable_relative_path: identity.executable_relative_path.clone(),
            executable_sha256: identity.executable_sha256.clone(),
            info,
            executable: exec,
        },
    );
}

/// Verify a cataloged CLAP bundle using the O(1) `stat`-fingerprint fast path
/// first, falling back to full SHA-256 `identity.unchanged()` on any cache miss
/// or metadata change.
pub(crate) fn verify_clap_unchanged(identity: &ClapBundleIdentity) -> Result<(), String> {
    if let Ok(guard) = cache().lock()
        && let Some(CachedSource::Clap {
            canonical_path,
            info_plist_sha256,
            executable_relative_path,
            executable_sha256,
            info,
            executable: cached_exec,
        }) = guard.get(&identity.supplied_path)
        && canonical_path == &identity.canonical_path
        && info_plist_sha256 == &identity.info_plist_sha256
        && executable_relative_path == &identity.executable_relative_path
        && executable_sha256 == &identity.executable_sha256
    {
        let expected_info = *info;
        let expected_exec = *cached_exec;
        drop(guard);
        if let Some(current) =
            canonical_bundle_dir_no_symlink(&identity.supplied_path, &identity.canonical_path)
        {
            let info_ok = match expected_info {
                Some(fp) => stat_relative_member(&current, "Contents/Info.plist") == Some(fp),
                None => !current.join("Contents/Info.plist").exists(),
            };
            if info_ok
                && stat_relative_member(&current, &identity.executable_relative_path)
                    == Some(expected_exec)
            {
                return Ok(());
            }
        }
    }

    #[cfg(test)]
    CLAP_FULL_HASH_COUNT.fetch_add(1, Ordering::Relaxed);

    match identity.unchanged() {
        Ok(()) => {
            record_clap_verified(identity);
            Ok(())
        }
        Err(error) => {
            remove_cache(&identity.supplied_path);
            Err(error)
        }
    }
}

#[cfg(test)]
pub(crate) fn vst3_full_hash_count() -> usize {
    VST3_FULL_HASH_COUNT.load(Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn clear_entry_for_test(path: &Path) {
    remove_cache(path);
}
