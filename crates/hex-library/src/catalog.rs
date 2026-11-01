//! Recursive, bounded sample discovery. All calls belong on control workers.
use crate::{
    DirectoryListing, EntryKind, LibraryEntry, MAX_RETAINED_ENTRIES, is_hidden, is_wav,
    sort_entries,
};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering},
    time::UNIX_EPOCH,
};

mod database;
mod fingerprint;
mod lookup;
#[cfg(test)]
mod tests;

pub use fingerprint::{FINGERPRINT_BLOCK, Fingerprint};
pub use lookup::{CatalogMatch, MAX_CANDIDATES, MatchKind, MissingSample};

pub const MAX_SAMPLES: usize = 50_000;
pub const MAX_SCAN_ENTRIES: usize = 500_000;
pub const MAX_SCAN_DEPTH: usize = 64;
pub const MAX_DIRECTORIES: usize = 16_384;
pub const MAX_ROOTS: usize = 16;
pub(super) const MAX_PATH_BYTES: usize = 16_384;
pub(super) const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;

/// A derived sample database. Original paths remain the import/preview identity.
///
/// This is application data under the app's support directory, never project data,
/// and it is replaceable: a lost or corrupt catalog is rebuilt by a fresh scan.
/// It is not a library identity authority; per-item tags and stable identities
/// belong to the planned neutral library schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SampleCatalog {
    entries: Vec<LibraryEntry>,
    /// File facts keyed by original path. Entries loaded from a version 1 cache
    /// have no record until the next scan inspects them.
    records: HashMap<PathBuf, SampleRecord>,
}

/// Facts recorded for one sample file when it was last scanned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleRecord {
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch; 0 when unavailable.
    pub modified_ns: u64,
    /// `None` when the file could not be read; such entries stay browseable.
    pub fingerprint: Option<Fingerprint>,
}

#[derive(Debug, Default)]
pub struct CatalogScan {
    pub catalog: SampleCatalog,
    /// Bounded per-folder errors; a failed folder does not hide other roots.
    pub errors: Vec<String>,
    pub truncated: bool,
    /// The scan stopped early on request; its catalog is partial and must not
    /// replace a complete one.
    pub cancelled: bool,
    /// Configured roots that no longer exist, in input order.
    pub missing_roots: Vec<PathBuf>,
    /// Files fingerprinted by reading their contents during this scan.
    pub fingerprinted: usize,
    /// Files whose previous record was reused because size and mtime matched.
    pub reused: usize,
    /// Sample files that could not be inspected or read.
    pub unreadable: usize,
}

/// Optional inputs for an incremental, cancellable scan.
#[derive(Clone, Copy, Default)]
pub struct ScanControl<'a> {
    /// The last catalog; unchanged files (same size and mtime) reuse its records.
    pub previous: Option<&'a SampleCatalog>,
    /// Set from any thread to stop the scan at the next entry.
    pub cancel: Option<&'a AtomicBool>,
    /// Incremented once per inspected directory entry, for progress display.
    pub progress: Option<&'a AtomicUsize>,
}

impl SampleCatalog {
    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    pub fn record(&self, path: &Path) -> Option<&SampleRecord> {
        self.records.get(path)
    }

    /// Re-key records under `old` to the same relative paths under `new` after a
    /// place was relocated, so a later incremental scan can reuse fingerprints of
    /// moved files whose size and mtime are unchanged. Paths outside `old` stay.
    pub fn relocate_root(&mut self, old: &Path, new: &Path) {
        if old == new {
            return;
        }
        let mut moved = false;
        for entry in &mut self.entries {
            let Ok(relative) = entry.path.strip_prefix(old) else {
                continue;
            };
            let path = new.join(relative);
            if let Some(record) = self.records.remove(&entry.path) {
                self.records.insert(path.clone(), record);
            }
            entry.path = path;
            moved = true;
        }
        // A relocated entry may now alias one already cataloged under `new`.
        let mut seen = BTreeSet::new();
        self.entries.retain(|entry| seen.insert(entry.path.clone()));
        if moved {
            sort_entries(&mut self.entries, true);
        }
    }

    /// Combine global sample matches with a bounded current-folder snapshot.
    /// Original-path identity deduplicates WAVs present in both sources while
    /// retaining current-folder WAVs outside configured sample roots. This does
    /// not enumerate files or recursively search projects.
    pub fn search_with_listing(
        &self,
        query: &str,
        listing: Option<&DirectoryListing>,
    ) -> Vec<LibraryEntry> {
        self.search_with_listing_in(query, None, listing)
    }

    /// The same indexed/current-folder composition scoped to one active Place.
    pub fn search_with_listing_in(
        &self,
        query: &str,
        source: Option<&Path>,
        listing: Option<&DirectoryListing>,
    ) -> Vec<LibraryEntry> {
        let mut entries = self.filtered(query, source);
        let mut folded = String::new();
        if let Some(listing) = listing {
            let query = query.to_lowercase();
            let terms: Vec<_> = query.split_whitespace().collect();
            let extra = {
                let mut seen = entries
                    .iter()
                    .map(|entry| entry.path.as_path())
                    .collect::<BTreeSet<_>>();
                listing
                    .entries
                    .iter()
                    .take(MAX_RETAINED_ENTRIES)
                    .filter(|entry| source.is_none_or(|root| entry.path.starts_with(root)))
                    .filter(|entry| {
                        if source.is_some() {
                            matches_path_terms(&entry.path, &terms, &mut folded)
                        } else {
                            lowercase_into(&entry.name, &mut folded).contains(&query)
                        }
                    })
                    .filter(|entry| seen.insert(entry.path.as_path()))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            entries.extend(extra);
        }
        sort_entries(&mut entries, true);
        entries
    }

    /// Discard cached records outside the current configured roots.
    pub fn retain_roots(&mut self, roots: &[PathBuf]) {
        self.entries
            .retain(|entry| roots.iter().any(|root| entry.path.starts_with(root)));
        let entries = &self.entries;
        if self.records.len() > entries.len() {
            let kept: BTreeSet<_> = entries.iter().map(|entry| entry.path.as_path()).collect();
            self.records.retain(|path, _| kept.contains(path.as_path()));
        }
    }

    /// Match all whitespace-separated terms against the original path/name.
    pub fn filtered(&self, query: &str, source: Option<&Path>) -> Vec<LibraryEntry> {
        let query = query.to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        let mut folded = String::new();
        self.entries
            .iter()
            .filter(|entry| {
                if !source.is_none_or(|source| entry.path.starts_with(source)) {
                    return false;
                }
                matches_path_terms(&entry.path, &terms, &mut folded)
            })
            .cloned()
            .collect()
    }
}

fn matches_path_terms(path: &Path, terms: &[&str], folded: &mut String) -> bool {
    if terms.is_empty() {
        return true;
    }
    let path = path.to_string_lossy();
    let path = lowercase_into(&path, folded);
    terms.iter().all(|term| path.contains(term))
}

/// Reuse query-local ASCII capacity. Full Unicode lowering stays with `str` so
/// contextual mappings (such as final Greek sigma) retain existing search semantics.
fn lowercase_into<'a>(value: &str, folded: &'a mut String) -> &'a str {
    if value.is_ascii() {
        folded.clear();
        folded.push_str(value);
        folded.make_ascii_lowercase();
    } else {
        *folded = value.to_lowercase();
    }
    folded.as_str()
}

/// Scan Music and explicitly added places, skipping hidden entries and directory
/// symlinks. Canonical directory identities prevent overlapping roots from repeating
/// work. The supplied paths, including non-UTF8 names, are preserved in results.
pub fn scan_samples(roots: &[PathBuf]) -> CatalogScan {
    scan_samples_with(roots, ScanControl::default())
}

/// [`scan_samples`] with incremental reuse, cancellation and progress. Sample
/// files are opened read-only; nothing under a root is created, moved or changed.
pub fn scan_samples_with(roots: &[PathBuf], control: ScanControl<'_>) -> CatalogScan {
    let mut walk = Walk {
        control,
        visited: BTreeSet::new(),
        inspected: 0,
        path_bytes: 0,
        result: CatalogScan {
            truncated: roots.len() > MAX_ROOTS,
            ..CatalogScan::default()
        },
    };
    for root in roots.iter().take(MAX_ROOTS) {
        if walk.cancelled() {
            break;
        }
        if !root.is_absolute() {
            walk.report(
                root,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "sample folders must be absolute paths",
                ),
            );
            continue;
        }
        if matches!(fs::symlink_metadata(root), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            walk.result.missing_roots.push(root.clone());
        }
        walk.visit(root, 0);
    }
    let mut result = walk.result;
    sort_entries(&mut result.catalog.entries, true);
    result
}

struct Walk<'a> {
    control: ScanControl<'a>,
    visited: BTreeSet<PathBuf>,
    inspected: usize,
    path_bytes: usize,
    result: CatalogScan,
}

impl Walk<'_> {
    fn cancelled(&mut self) -> bool {
        if !self.result.cancelled
            && self
                .control
                .cancel
                .is_some_and(|cancel| cancel.load(AtomicOrdering::Relaxed))
        {
            self.result.cancelled = true;
        }
        self.result.cancelled
    }

    fn full(&mut self) -> bool {
        if self.inspected >= MAX_SCAN_ENTRIES || self.result.catalog.entries.len() >= MAX_SAMPLES {
            self.result.truncated = true;
            return true;
        }
        false
    }

    fn report(&mut self, path: &Path, error: std::io::Error) {
        if self.result.errors.len() < MAX_ROOTS {
            self.result
                .errors
                .push(format!("could not scan {}: {error}", path.display()));
        }
    }

    fn visit(&mut self, directory: &Path, depth: usize) {
        if self.full() {
            return;
        }
        let identity = match fs::canonicalize(directory) {
            Ok(path) => path,
            Err(error) => {
                self.report(directory, error);
                return;
            }
        };
        let identity_bytes = identity.as_os_str().as_encoded_bytes().len();
        if self.visited.len() >= MAX_DIRECTORIES
            || identity_bytes > MAX_PATH_BYTES
            || self.path_bytes + identity_bytes > MAX_CATALOG_BYTES / 2
        {
            self.result.truncated = true;
            return;
        }
        if !self.visited.insert(identity) {
            return;
        }
        self.path_bytes += identity_bytes;
        let items = match fs::read_dir(directory) {
            Ok(items) => items,
            Err(error) => {
                self.report(directory, error);
                return;
            }
        };
        for item in items {
            if self.cancelled() || self.full() {
                break;
            }
            self.inspected += 1;
            if let Some(progress) = self.control.progress {
                progress.fetch_add(1, AtomicOrdering::Relaxed);
            }
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    self.report(directory, error);
                    continue;
                }
            };
            if is_hidden(&item.file_name()) {
                continue;
            }
            let path = item.path();
            let kind = match item.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    self.report(&path, error);
                    continue;
                }
            };
            if kind.is_dir() {
                if depth < MAX_SCAN_DEPTH {
                    self.visit(&path, depth + 1);
                } else {
                    self.result.truncated = true;
                }
            } else if kind.is_file() && is_wav(path.extension()) {
                let size = path.as_os_str().as_encoded_bytes().len();
                if size > MAX_PATH_BYTES || self.path_bytes + size > MAX_CATALOG_BYTES / 2 {
                    self.result.truncated = true;
                    continue;
                }
                self.path_bytes += size;
                if let Some(record) = self.record(&item, &path) {
                    self.result.catalog.records.insert(path.clone(), record);
                }
                self.result.catalog.entries.push(LibraryEntry {
                    name: item.file_name().to_string_lossy().into_owned(),
                    path,
                    kind: EntryKind::WaveAudio,
                });
            }
        }
    }

    /// Reuse the previous record when size and mtime are unchanged; otherwise
    /// fingerprint the file. Unreadable files stay listed without a fingerprint.
    fn record(&mut self, item: &fs::DirEntry, path: &Path) -> Option<SampleRecord> {
        let Ok(metadata) = item.metadata() else {
            self.result.unreadable += 1;
            return None;
        };
        let size = metadata.len();
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| {
                u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
            });
        if let Some(previous) = self
            .control
            .previous
            .and_then(|catalog| catalog.records.get(path))
            && previous.size == size
            && previous.modified_ns == modified_ns
            && modified_ns != 0
            && previous.fingerprint.is_some()
        {
            self.result.reused += 1;
            return Some(*previous);
        }
        let fingerprint = match Fingerprint::of_file(path) {
            Ok(fingerprint) => {
                self.result.fingerprinted += 1;
                Some(fingerprint)
            }
            Err(_) => {
                self.result.unreadable += 1;
                None
            }
        };
        Some(SampleRecord {
            size,
            modified_ns,
            fingerprint,
        })
    }
}
#[cfg(test)]
mod profile_test;
