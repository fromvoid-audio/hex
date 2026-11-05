//! Sample catalog worker coordination and compact source filters.
use super::*;
use hex_library::catalog::{SampleCatalog, ScanControl, scan_samples_with};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

/// The Samples listing model shared by row construction and its count label.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResultsMode {
    Hierarchy,
    Indexed,
}

impl ResultsMode {
    pub(super) fn for_query(query: &str) -> Self {
        if query.is_empty() {
            Self::Hierarchy
        } else {
            Self::Indexed
        }
    }

    fn count_noun(self) -> &'static str {
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
pub(super) struct SampleBrowser {
    pub(super) catalog: SampleCatalog,
    /// Configured roots the last completed scan found missing.
    pub(super) missing_roots: BTreeSet<PathBuf>,
    /// Roots and cancel flag of the scan in flight; a root change cancels it.
    scanning: Option<(Vec<PathBuf>, Arc<AtomicBool>)>,
    /// Entries inspected by the scan in flight.
    progress: Arc<AtomicUsize>,
    in_flight: bool,
    pending: bool,
    loaded: bool,
    /// Periodic rescan cadence; scans run on control workers, never the UI.
    rescan: RescanBackoff,
    pub(super) status: Option<String>,
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
struct RescanBackoff {
    interval: Duration,
}

impl Default for RescanBackoff {
    fn default() -> Self {
        Self {
            interval: RESCAN_MIN,
        }
    }
}

impl RescanBackoff {
    fn completed(&mut self, changed: bool) {
        self.interval = if changed {
            RESCAN_MIN
        } else {
            self.interval.saturating_mul(2).min(RESCAN_MAX)
        };
    }
}

impl SampleBrowser {
    pub(super) fn history_loading(&self) -> bool {
        self.in_flight || self.pending
    }

    pub(super) fn visible(&self, query: &str) -> Arc<Vec<LibraryEntry>> {
        self.visible_with_listing(query, None, None)
    }

    /// Global search passes no source filter and the current bounded listing.
    /// Samples passes its source filter and no folder listing.
    pub(super) fn visible_with_listing(
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

    pub(super) fn folder(
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
    pub(super) fn invalidate_listing(&self) {
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

    pub(super) fn replace(&mut self, catalog: SampleCatalog) -> bool {
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

impl EditorView {
    pub(super) fn sample_roots(&self) -> Vec<PathBuf> {
        // Home remains a navigation shortcut; never recursively scan a user's
        // entire home merely because that shortcut exists by default.
        self.browser
            .places
            .iter()
            .filter(|path| **path != self.browser.home)
            .cloned()
            .collect()
    }

    pub(super) fn start_sample_catalog(&mut self, cx: &mut Context<Self>) {
        self.refresh_sample_catalog(cx);
        #[cfg(not(test))]
        {
            let this = cx.entity().downgrade();
            cx.spawn(async move |_, cx| {
                loop {
                    // Read before sleeping; a scan finishing meanwhile adjusts
                    // the following interval.
                    let Ok(interval) =
                        this.update(cx, |view, _| view.browser.samples.rescan.interval)
                    else {
                        break;
                    };
                    cx.background_executor().timer(interval).await;
                    if this
                        .update(cx, |view, cx| view.refresh_sample_catalog(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    pub(super) fn refresh_sample_catalog(&mut self, cx: &mut Context<Self>) {
        if self.browser.preferences_loading {
            return;
        }
        let roots = self.sample_roots();
        if self.browser.samples.in_flight {
            if let Some((scanning, cancel)) = &self.browser.samples.scanning
                && *scanning != roots
            {
                // Its result would be discarded anyway; stop reading files now.
                cancel.store(true, Ordering::Relaxed);
            }
            self.browser.samples.pending = true;
            return;
        }
        self.browser.samples.in_flight = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.browser.samples.scanning = Some((roots.clone(), Arc::clone(&cancel)));
        self.browser.samples.progress.store(0, Ordering::Relaxed);
        let progress = Arc::clone(&self.browser.samples.progress);
        #[cfg(not(test))]
        self.repaint_sample_scan_progress(cx);
        cx.notify();
        let load = !self.browser.samples.loaded;
        let database = self
            .browser
            .preferences_path
            .with_extension("samples-v1.db");
        #[cfg(test)]
        let test_environment = Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            if load {
                let path = database.clone();
                #[cfg(test)]
                let load_environment = Arc::clone(&test_environment);
                let cached = cx
                    .background_executor()
                    .spawn(async move {
                        #[cfg(test)]
                        let _test_environment = load_environment;
                        // A refused cache is kept aside, then rebuilt by this scan.
                        SampleCatalog::load(&path).inspect_err(|error| {
                            if error.kind() == std::io::ErrorKind::InvalidData {
                                let _ = SampleCatalog::preserve_corrupt(&path);
                            }
                        })
                    })
                    .await;
                if this
                    .update(cx, |view, cx| {
                        if view.browser.samples.loaded {
                            return;
                        }
                        view.browser.samples.loaded = true;
                        match cached {
                            Ok(mut catalog) if roots == view.sample_roots() => {
                                catalog.retain_roots(&roots);
                                view.apply_sample_catalog(catalog, cx);
                            }
                            Err(error) => {
                                view.browser.samples.status =
                                    Some(format!("rebuilding sample database · {error}"))
                            }
                            _ => {}
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let Ok(previous) = this.update(cx, |view, _| view.browser.samples.catalog.clone())
            else {
                return;
            };
            let scan_roots = roots.clone();
            let (scan, saved) = cx
                .background_executor()
                .spawn(async move {
                    let scan = scan_samples_with(
                        &scan_roots,
                        ScanControl {
                            previous: Some(&previous),
                            cancel: Some(&cancel),
                            progress: Some(&progress),
                        },
                    );
                    #[cfg(test)]
                    let gate = test_environment.before_catalog_save().await;
                    // A cancelled scan is partial; keep the last complete cache.
                    // An unchanged catalog is not rewritten or synced.
                    let saved = if scan.cancelled {
                        Ok(false)
                    } else {
                        scan.catalog.save(&database)
                    };
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.did_save(saved.is_ok(), test_environment.root().is_dir());
                    }
                    (scan, saved)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser.samples.in_flight = false;
                view.browser.samples.scanning = None;
                // Changes to roots while a scan runs must not publish old results.
                if roots == view.sample_roots() && !scan.cancelled {
                    view.browser.samples.missing_roots =
                        scan.missing_roots.iter().cloned().collect();
                    let mut status = Vec::new();
                    if scan.truncated {
                        status.push("sample scan limit reached".to_owned());
                    }
                    if let Some(error) = scan.errors.first() {
                        status.push(error.clone());
                    }
                    if let Err(error) = saved {
                        status.push(format!("could not save sample database · {error}"));
                    }
                    view.browser.samples.status = if status.is_empty() {
                        None
                    } else {
                        Some(status.join(" · "))
                    };
                    let changed = view.apply_sample_catalog(scan.catalog, cx);
                    view.browser.samples.rescan.completed(changed);
                } else {
                    view.browser.samples.pending = true;
                }
                if std::mem::take(&mut view.browser.samples.pending) {
                    view.refresh_sample_catalog(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Redraw the progress count a few times a second while a scan runs. The
    /// count is an atomic read; the UI never walks folders itself.
    #[cfg(not(test))]
    fn repaint_sample_scan_progress(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let scanning = this
                    .update(cx, |view, cx| {
                        cx.notify();
                        view.browser.samples.in_flight
                    })
                    .unwrap_or(false);
                if !scanning {
                    break;
                }
            }
        })
        .detach();
    }

    /// Publish `catalog`; returns whether it differed from the shown one.
    pub(super) fn apply_sample_catalog(
        &mut self,
        catalog: SampleCatalog,
        cx: &mut Context<Self>,
    ) -> bool {
        let remap = !self.browser_place_unavailable()
            && matches!(
                self.browser.section,
                BrowserSection::All | BrowserSection::Samples | BrowserSection::Place(_)
            );
        let selection = remap.then(|| self.browser_selected_item(cx)).flatten();
        let changed = self.browser.samples.replace(catalog);
        if changed && remap {
            self.restore_browser_selection(selection, cx);
        }
        changed
    }

    pub(super) fn sample_results_count_text(&self, cx: &App) -> String {
        let count = self.browser_visible_entries(cx).len();
        let noun = ResultsMode::for_query(self.browser.search.read(cx).text()).count_noun();
        let scanned = self.browser.samples.progress.load(Ordering::Relaxed);
        if !self.browser.samples.in_flight {
            format!("{count} {noun}")
        } else if scanned == 0 {
            format!("{count} {noun} · scanning…")
        } else {
            format!("{count} {noun} · scanning {scanned} files…")
        }
    }

    pub(super) fn sample_filters(&self, cx: &mut Context<Self>) -> Div {
        let control = self.browser_random_button(cx);
        let mut content = div().flex().flex_col().flex_shrink_0().child(
            div()
                .h(px(23.0))
                .px_1()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text(self.sample_results_count_text(cx))),
                )
                .child(self.browser_icon(
                    "↻",
                    "rescan sample folders",
                    |view, _, cx| view.refresh_browser_tree(cx),
                    cx,
                ))
                .child(self.browser_icon(
                    "+",
                    "add sample folder",
                    |view, _, cx| view.open_browser_folder(cx),
                    cx,
                ))
                .child(control),
        );
        let mut filters = div().flex().flex_wrap().px_1().gap_1();
        for (index, source) in std::iter::once(None)
            .chain(self.sample_roots().into_iter().map(Some))
            .enumerate()
        {
            let selected = source.as_ref().map_or(BrowserSection::Samples, |path| {
                BrowserSection::Place(path.clone())
            });
            let active = self.browser.section == selected;
            let label = source.as_ref().map_or_else(
                || "all folders".to_owned(),
                |path| {
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned()
                },
            );
            filters = filters.child(
                div()
                    .id(("sample-source", index))
                    .max_w(px(120.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .line_clamp(1)
                    .h(px(22.0))
                    .px_1()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(theme::color(if active {
                        theme::PaletteRole::Secondary
                    } else {
                        theme::PaletteRole::Muted
                    })))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            view.select_browser_source(selected.clone(), cx);
                            cx.notify();
                        }),
                    )
                    .child(theme::text(label)),
            );
        }
        content = content.child(filters);
        if let Some(status) = self.browser.samples.status.as_ref() {
            content = content.child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(status)),
            );
        }
        content
    }
}

#[cfg(test)]
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
