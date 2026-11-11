//! In-memory sample browsing and catalog publication state.
use hex_library::{DirectoryListing, EntryKind, LibraryEntry, catalog::SampleCatalog};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
    time::Duration,
};

/// The Samples listing model shared by row construction and its count label.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::editor::browser) enum ResultsMode {
    Hierarchy,
    Indexed,
}

impl ResultsMode {
    pub(in crate::editor::browser) fn for_query(query: &str) -> Self {
        if query.is_empty() {
            Self::Hierarchy
        } else {
            Self::Indexed
        }
    }

    pub(super) fn count_noun(self) -> &'static str {
        match self {
            Self::Hierarchy => "items",
            Self::Indexed => "samples",
        }
    }
}

struct VisibleCache {
    query: String,
    source: Option<PathBuf>,
    directory: Option<PathBuf>,
    entries: Arc<Vec<LibraryEntry>>,
}

struct FolderCache {
    query: String,
    directory: Option<PathBuf>,
    projects_only: bool,
    entries: Arc<Vec<LibraryEntry>>,
}

#[derive(Default)]
pub(in crate::editor::browser) struct SampleBrowser {
    pub(in crate::editor::browser) catalog: SampleCatalog,
    /// Configured roots the last completed scan found missing.
    pub(in crate::editor::browser) missing_roots: BTreeSet<PathBuf>,
    /// Roots and cancel flag of the scan in flight; a root change cancels it.
    pub(super) scanning: Option<(Vec<PathBuf>, Arc<AtomicBool>)>,
    /// Entries inspected by the scan in flight.
    pub(super) progress: Arc<AtomicUsize>,
    pub(super) in_flight: bool,
    pub(super) pending: bool,
    pub(super) loaded: bool,
    /// Periodic rescan cadence; scans run on control workers, never the UI.
    pub(super) rescan: RescanBackoff,
    pub(in crate::editor::browser) status: Option<String>,
    visible: RefCell<Option<VisibleCache>>,
    folder_visible: RefCell<Option<FolderCache>>,
}

/// Shortest periodic rescan interval, used after a scan found a change.
const RESCAN_MIN: Duration = Duration::from_secs(30);
/// Longest interval reached while consecutive scans find nothing new.
const RESCAN_MAX: Duration = Duration::from_secs(300);

/// Polling cadence for library changes made outside hex. Each completed scan
/// that found no change doubles the wait up to [`RESCAN_MAX`]; a change resets
/// it to [`RESCAN_MIN`]. Place edits still rescan immediately. This bounds the
/// full-tree walk of an idle library to about twelve per hour instead of 120;
/// filesystem notifications would need a separate native-boundary decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RescanBackoff {
    pub(super) interval: Duration,
}

impl Default for RescanBackoff {
    fn default() -> Self {
        Self {
            interval: RESCAN_MIN,
        }
    }
}

impl RescanBackoff {
    pub(super) fn completed(&mut self, changed: bool) {
        self.interval = if changed {
            RESCAN_MIN
        } else {
            self.interval.saturating_mul(2).min(RESCAN_MAX)
        };
    }
}

impl SampleBrowser {
    pub(in crate::editor::browser) fn history_loading(&self) -> bool {
        self.in_flight || self.pending
    }

    pub(in crate::editor::browser) fn visible(&self, query: &str) -> Arc<Vec<LibraryEntry>> {
        self.visible_with_listing(query, None, None)
    }

    /// Global search passes no source filter and the current bounded listing.
    /// Samples passes its source filter and no folder listing.
    pub(in crate::editor::browser) fn visible_with_listing(
        &self,
        query: &str,
        source: Option<&std::path::Path>,
        listing: Option<&DirectoryListing>,
    ) -> Arc<Vec<LibraryEntry>> {
        let directory = listing.map(|listing| listing.directory.as_path());
        let mut cache = self.visible.borrow_mut();
        if let Some(previous) = cache.as_ref()
            && previous.query == query
            && previous.source.as_deref() == source
            && previous.directory.as_deref() == directory
        {
            return previous.entries.clone();
        }
        let entries = Arc::new(if listing.is_some() {
            self.catalog.search_with_listing_in(query, source, listing)
        } else {
            self.catalog.filtered(query, source)
        });
        *cache = Some(VisibleCache {
            query: query.to_owned(),
            source: source.map(PathBuf::from),
            directory: directory.map(PathBuf::from),
            entries: entries.clone(),
        });
        entries
    }

    pub(in crate::editor::browser) fn folder(
        &self,
        query: &str,
        listing: Option<&DirectoryListing>,
        projects_only: bool,
    ) -> Arc<Vec<LibraryEntry>> {
        let directory = listing.map(|listing| &listing.directory);
        let mut cache = self.folder_visible.borrow_mut();
        if let Some(previous) = cache.as_ref()
            && previous.query == query
            && previous.directory.as_ref() == directory
            && previous.projects_only == projects_only
        {
            return previous.entries.clone();
        }
        let entries = Arc::new(
            listing
                .into_iter()
                .flat_map(|listing| listing.entries.iter())
                .filter(|entry| !projects_only || entry.kind != EntryKind::WaveAudio)
                .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(query))
                .cloned()
                .collect::<Vec<_>>(),
        );
        *cache = Some(FolderCache {
            query: query.into(),
            directory: directory.cloned(),
            projects_only,
            entries: entries.clone(),
        });
        entries
    }

    /// Folder contents can change without changing the path or query.
    pub(in crate::editor::browser) fn invalidate_listing(&self) {
        *self.folder_visible.borrow_mut() = None;
        let has_listing = self
            .visible
            .borrow()
            .as_ref()
            .is_some_and(|cache| cache.directory.is_some());
        if has_listing {
            *self.visible.borrow_mut() = None;
        }
    }

    pub(in crate::editor::browser) fn replace(&mut self, catalog: SampleCatalog) -> bool {
        // A live publication supersedes a pending disk-cache bootstrap.
        self.loaded = true;
        if self.catalog == catalog {
            return false;
        }
        self.catalog = catalog;
        *self.visible.borrow_mut() = None;
        true
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod cache_tests {
    use super::*;

    #[test]
    fn combined_results_reuse_cache_and_accept_republished_folder_contents() {
        let directory = PathBuf::from("/sample-cache-fixture");
        let mut listing = DirectoryListing {
            directory: directory.clone(),
            entries: vec![LibraryEntry {
                name: "utility.wav".into(),
                path: directory.join("utility.wav"),
                kind: EntryKind::WaveAudio,
            }],
            enumerated_entries: 1,
            truncated: false,
        };
        let browser = SampleBrowser::default();
        let first = browser.visible_with_listing("utility", None, Some(&listing));
        assert_eq!(first.len(), 1);
        assert!(Arc::ptr_eq(
            &first,
            &browser.visible_with_listing("utility", None, Some(&listing))
        ));
        listing.entries.push(LibraryEntry {
            name: "utility-new.wav".into(),
            path: directory.join("utility-new.wav"),
            kind: EntryKind::WaveAudio,
        });
        listing.enumerated_entries += 1;
        browser.invalidate_listing();
        let updated = browser.visible_with_listing("utility", None, Some(&listing));
        assert_eq!(updated.len(), 2);
        assert!(!Arc::ptr_eq(&first, &updated));
        assert!(browser.visible("utility").is_empty());
        assert_eq!(
            browser
                .visible_with_listing("utility", None, Some(&listing))
                .len(),
            2
        );
    }

    #[test]
    fn idle_library_rescans_back_off_and_a_change_restores_the_short_interval() {
        let mut rescan = RescanBackoff::default();
        assert_eq!(rescan.interval, RESCAN_MIN);
        let mut waits = Vec::new();
        for _ in 0..6 {
            rescan.completed(false);
            waits.push(rescan.interval.as_secs());
        }
        assert_eq!(waits, [60, 120, 240, 300, 300, 300]);
        rescan.completed(true);
        assert_eq!(rescan.interval, RESCAN_MIN);
    }
}
