//! Recursive, bounded sample discovery. All calls belong on control workers.
use crate::{
    DirectoryListing, EntryKind, LibraryEntry, MAX_RETAINED_ENTRIES, compare_entries, is_hidden,
    is_wav,
};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

mod database;
#[cfg(test)]
mod tests;

pub const MAX_SAMPLES: usize = 50_000;
pub const MAX_SCAN_ENTRIES: usize = 500_000;
pub const MAX_SCAN_DEPTH: usize = 64;
pub const MAX_DIRECTORIES: usize = 16_384;
pub const MAX_ROOTS: usize = 16;
pub(super) const MAX_PATH_BYTES: usize = 16_384;
pub(super) const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;

/// A derived sample database. Original paths remain the import/preview identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SampleCatalog {
    entries: Vec<LibraryEntry>,
}

#[derive(Debug, Default)]
pub struct CatalogScan {
    pub catalog: SampleCatalog,
    /// Bounded per-folder errors; a failed folder does not hide other roots.
    pub errors: Vec<String>,
    pub truncated: bool,
}

impl SampleCatalog {
    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
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
                            matches_path_terms(&entry.path, &terms)
                        } else {
                            entry.name.to_lowercase().contains(&query)
                        }
                    })
                    .filter(|entry| seen.insert(entry.path.as_path()))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            entries.extend(extra);
        }
        entries.sort_by(|left, right| {
            compare_entries(left, right).then_with(|| left.path.cmp(&right.path))
        });
        entries
    }

    /// Discard cached records outside the current configured roots.
    pub fn retain_roots(&mut self, roots: &[PathBuf]) {
        self.entries
            .retain(|entry| roots.iter().any(|root| entry.path.starts_with(root)));
    }

    /// Match all whitespace-separated terms against the original path/name.
    pub fn filtered(&self, query: &str, source: Option<&Path>) -> Vec<LibraryEntry> {
        let query = query.to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        self.entries
            .iter()
            .filter(|entry| {
                if !source.is_none_or(|source| entry.path.starts_with(source)) {
                    return false;
                }
                matches_path_terms(&entry.path, &terms)
            })
            .cloned()
            .collect()
    }
}

fn matches_path_terms(path: &Path, terms: &[&str]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let path = path.to_string_lossy().to_lowercase();
    terms.iter().all(|term| path.contains(term))
}

/// Scan Music and explicitly added places, skipping hidden entries and directory
/// symlinks. Canonical directory identities prevent overlapping roots from repeating
/// work. The supplied paths, including non-UTF8 names, are preserved in results.
pub fn scan_samples(roots: &[PathBuf]) -> CatalogScan {
    let mut result = CatalogScan::default();
    let mut visited = BTreeSet::new();
    let mut inspected = 0;
    let mut path_bytes = 0;
    result.truncated = roots.len() > MAX_ROOTS;
    for root in roots.iter().take(MAX_ROOTS) {
        if !root.is_absolute() {
            report(
                &mut result,
                root,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "sample folders must be absolute paths",
                ),
            );
            continue;
        }
        visit(
            root,
            0,
            &mut visited,
            &mut inspected,
            &mut path_bytes,
            &mut result,
        );
    }
    result.catalog.entries.sort_by(|left, right| {
        compare_entries(left, right).then_with(|| left.path.cmp(&right.path))
    });
    result
}

fn report(result: &mut CatalogScan, path: &Path, error: std::io::Error) {
    if result.errors.len() < MAX_ROOTS {
        result
            .errors
            .push(format!("could not scan {}: {error}", path.display()));
    }
}

fn visit(
    directory: &Path,
    depth: usize,
    visited: &mut BTreeSet<PathBuf>,
    inspected: &mut usize,
    path_bytes: &mut usize,
    result: &mut CatalogScan,
) {
    if *inspected >= MAX_SCAN_ENTRIES || result.catalog.entries.len() >= MAX_SAMPLES {
        result.truncated = true;
        return;
    }
    let identity = match fs::canonicalize(directory) {
        Ok(path) => path,
        Err(error) => {
            report(result, directory, error);
            return;
        }
    };
    if visited.len() >= MAX_DIRECTORIES
        || identity.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES
    {
        result.truncated = true;
        return;
    }
    let identity_bytes = identity.as_os_str().as_encoded_bytes().len();
    if *path_bytes + identity_bytes > MAX_CATALOG_BYTES / 2 {
        result.truncated = true;
        return;
    }
    if !visited.insert(identity) {
        return;
    }
    *path_bytes += identity_bytes;
    let items = match fs::read_dir(directory) {
        Ok(items) => items,
        Err(error) => {
            report(result, directory, error);
            return;
        }
    };
    for item in items {
        if *inspected >= MAX_SCAN_ENTRIES || result.catalog.entries.len() >= MAX_SAMPLES {
            result.truncated = true;
            break;
        }
        *inspected += 1;
        let item = match item {
            Ok(item) => item,
            Err(error) => {
                report(result, directory, error);
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
                report(result, &path, error);
                continue;
            }
        };
        if kind.is_dir() {
            if depth < MAX_SCAN_DEPTH {
                visit(&path, depth + 1, visited, inspected, path_bytes, result);
            } else {
                result.truncated = true;
            }
        } else if kind.is_file() && is_wav(path.extension()) {
            let size = path.as_os_str().as_encoded_bytes().len();
            if size > MAX_PATH_BYTES || *path_bytes + size > MAX_CATALOG_BYTES / 2 {
                result.truncated = true;
                continue;
            }
            *path_bytes += size;
            result.catalog.entries.push(LibraryEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                path,
                kind: EntryKind::WaveAudio,
            });
        }
    }
}
