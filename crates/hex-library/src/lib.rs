//! # Purpose
//! Own bounded folder listings and a persistent derived sample-discovery catalog while preserving
//! original paths.
//!
//! # Dependencies
//! Uses only the standard library; filesystem/catalog work runs on control workers.
//!
//! # Does not own
//! Does not decode audio, mutate projects, navigate UI or perform callback work. Catalog records
//! do not establish media readiness or source custody.

pub mod catalog;

use std::{
    cmp::Ordering,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

/// Maximum entries returned after stable sorting.
pub const MAX_RETAINED_ENTRIES: usize = 2_048;
/// Maximum directory entries inspected in a single scan.
pub const MAX_ENUMERATED_ENTRIES: usize = 8_192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    WaveAudio,
    Project,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryEntry {
    /// Original filesystem display name. Presentation code decides its casing.
    pub name: String,
    /// Original path, kept intact for later import/open operations.
    pub path: PathBuf,
    pub kind: EntryKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryListing {
    pub directory: PathBuf,
    pub entries: Vec<LibraryEntry>,
    pub enumerated_entries: usize,
    pub truncated: bool,
}

#[derive(Debug)]
pub enum ScanError {
    ReadDirectory { path: PathBuf, source: io::Error },
    ReadEntry { path: PathBuf, source: io::Error },
    ReadFileType { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadDirectory { path, source } => {
                write!(f, "could not read {}: {source}", path.display())
            }
            Self::ReadEntry { path, source } => {
                write!(
                    f,
                    "could not inspect an entry in {}: {source}",
                    path.display()
                )
            }
            Self::ReadFileType { path, source } => {
                write!(f, "could not inspect {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadDirectory { source, .. }
            | Self::ReadEntry { source, .. }
            | Self::ReadFileType { source, .. } => Some(source),
        }
    }
}

impl ScanError {
    /// Short display text that never rewrites or exposes the original path.
    pub fn user_message(&self) -> &'static str {
        let source = match self {
            Self::ReadDirectory { source, .. }
            | Self::ReadEntry { source, .. }
            | Self::ReadFileType { source, .. } => source,
        };
        match source.kind() {
            io::ErrorKind::NotFound => "folder no longer exists",
            io::ErrorKind::PermissionDenied => "folder access denied",
            _ => match self {
                Self::ReadDirectory { .. } => "could not read folder",
                Self::ReadEntry { .. } => "could not finish folder listing",
                Self::ReadFileType { .. } => "could not inspect folder item",
            },
        }
    }
}

/// Enumerates one folder without recursing and returns directories, WAVs and Hex projects.
///
/// Hidden entries are excluded. Enumeration stops after [`MAX_ENUMERATED_ENTRIES`];
/// retained entries are sorted case-insensitively with directories first, then
/// capped at [`MAX_RETAINED_ENTRIES`].
pub fn scan_directory(directory: impl AsRef<Path>) -> Result<DirectoryListing, ScanError> {
    let directory = directory.as_ref().to_path_buf();
    let read_dir = fs::read_dir(&directory).map_err(|source| ScanError::ReadDirectory {
        path: directory.clone(),
        source,
    })?;
    let mut entries = Vec::new();
    let mut enumerated_entries = 0;
    let mut iterator = read_dir;

    while enumerated_entries < MAX_ENUMERATED_ENTRIES {
        let Some(item) = iterator.next() else {
            break;
        };
        let item = item.map_err(|source| ScanError::ReadEntry {
            path: directory.clone(),
            source,
        })?;
        enumerated_entries += 1;
        let path = item.path();
        let name_os = item.file_name();
        if is_hidden(&name_os) {
            continue;
        }
        let name = name_os.to_string_lossy().into_owned();
        let file_type = item.file_type().map_err(|source| ScanError::ReadFileType {
            path: path.clone(),
            source,
        })?;
        let kind = if file_type.is_dir() {
            Some(EntryKind::Directory)
        } else if file_type.is_file() && is_wav(path.extension()) {
            Some(EntryKind::WaveAudio)
        } else if file_type.is_file() && is_project(path.extension()) {
            Some(EntryKind::Project)
        } else {
            None
        };
        if let Some(kind) = kind {
            entries.push(LibraryEntry { name, path, kind });
        }
    }

    sort_entries(&mut entries, false);
    let enumeration_truncated = if enumerated_entries == MAX_ENUMERATED_ENTRIES {
        iterator
            .next()
            .transpose()
            .map_err(|source| ScanError::ReadEntry {
                path: directory.clone(),
                source,
            })?
            .is_some()
    } else {
        false
    };
    let truncated = entries.len() > MAX_RETAINED_ENTRIES || enumeration_truncated;
    entries.truncate(MAX_RETAINED_ENTRIES);
    Ok(DirectoryListing {
        directory,
        entries,
        enumerated_entries,
        truncated,
    })
}

fn is_hidden(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn is_wav(extension: Option<&OsStr>) -> bool {
    extension.is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("wav"))
}

fn is_project(extension: Option<&OsStr>) -> bool {
    extension.is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("hex"))
}

/// Fold display names once, leaving original names and native paths untouched.
/// `with_path` adds the catalog's native-path tie break; folder ties retain input order.
fn sort_entries(entries: &mut [LibraryEntry], with_path: bool) {
    let rank = |kind| match kind {
        EntryKind::Directory => 0_u8,
        EntryKind::WaveAudio | EntryKind::Project => 1,
    };
    let mut keys: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (rank(entry.kind), entry.name.to_lowercase(), index))
        .collect();
    keys.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| entries[a.2].name.cmp(&entries[b.2].name))
            .then_with(|| {
                if with_path {
                    entries[a.2].path.cmp(&entries[b.2].path)
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| a.2.cmp(&b.2))
    });
    // Keys contain an enumeration permutation. Swaps displace earlier originals;
    // their processed indices redirect to the slot now containing that original.
    // Following those redirects always reaches an unprocessed slot in bounds.
    for i in 0..keys.len() {
        let mut index = keys[i].2;
        while index < i {
            index = keys[index].2;
        }
        keys[i].2 = index;
        entries.swap(i, index);
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryKind, MAX_ENUMERATED_ENTRIES, MAX_RETAINED_ENTRIES, scan_directory};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("hex-library-test-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("create fixture");
            Self(path)
        }
        fn file(&self, name: &str) {
            fs::write(self.0.join(name), []).expect("write fixture file");
        }
        fn directory(&self, name: &str) {
            fs::create_dir(self.0.join(name)).expect("create fixture folder");
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn lists_case_preserved_wav_names_and_folders_first_in_stable_order() {
        let fixture = Fixture::new();
        fixture.directory("zFolder");
        fixture.directory("Alpha");
        fixture.file("mixB.WAV");
        fixture.file("mixA.wav");
        fixture.file("notes.txt");
        fixture.file("My Set.HEX");
        fixture.file(".hidden.wav");

        let listing = scan_directory(&fixture.0).expect("scan");
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "zFolder", "mixA.wav", "mixB.WAV", "My Set.HEX"]
        );
        assert_eq!(listing.entries[0].kind, EntryKind::Directory);
        assert_eq!(listing.entries[2].kind, EntryKind::WaveAudio);
        assert_eq!(listing.entries[3].path, fixture.0.join("mixB.WAV"));
    }

    #[test]
    fn reports_io_errors_and_limits_retained_entries() {
        let missing =
            std::env::temp_dir().join(format!("hex-library-missing-{}", std::process::id()));
        let error = scan_directory(&missing).expect_err("missing folder must fail");
        assert!(error.to_string().contains("could not read"));
        assert_eq!(error.user_message(), "folder no longer exists");

        let fixture = Fixture::new();
        for index in 0..(MAX_RETAINED_ENTRIES + 1) {
            fixture.file(&format!("{index:04}.wav"));
        }
        let listing = scan_directory(&fixture.0).expect("scan");
        assert_eq!(listing.entries.len(), MAX_RETAINED_ENTRIES);
        assert!(listing.truncated);
        assert_eq!(listing.enumerated_entries, MAX_RETAINED_ENTRIES + 1);
    }

    #[test]
    fn caps_enumeration_and_keeps_listing_bounds() {
        let fixture = Fixture::new();
        for index in 0..=MAX_ENUMERATED_ENTRIES {
            fixture.file(&format!("{index:05}.wav"));
        }
        let listing = scan_directory(&fixture.0).expect("bounded scan");
        assert_eq!(listing.enumerated_entries, MAX_ENUMERATED_ENTRIES);
        assert!(listing.truncated);
        assert!(listing.entries.len() <= MAX_RETAINED_ENTRIES);
    }
}
