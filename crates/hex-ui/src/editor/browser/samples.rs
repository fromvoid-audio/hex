//! Sample catalog worker coordination and compact source filters.
use super::*;
use hex_library::catalog::{SampleCatalog, scan_samples};

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
    in_flight: bool,
    pending: bool,
    loaded: bool,
    pub(super) status: Option<String>,
    visible: RefCell<Option<VisibleCache>>,
    folder_visible: RefCell<Option<FolderCache>>,
}

impl SampleBrowser {
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
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(30))
                        .await;
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
        if self.browser.samples.in_flight {
            self.browser.samples.pending = true;
            return;
        }
        self.browser.samples.in_flight = true;
        cx.notify();
        let load = !self.browser.samples.loaded;
        let roots = self.sample_roots();
        let database = self
            .browser
            .preferences_path
            .with_extension("samples-v1.db");
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            if load {
                let path = database.clone();
                let cached = cx
                    .background_executor()
                    .spawn(async move { SampleCatalog::load(&path) })
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
            let scan_roots = roots.clone();
            let (scan, saved) = cx
                .background_executor()
                .spawn(async move {
                    let scan = scan_samples(&scan_roots);
                    let saved = scan.catalog.save(&database);
                    (scan, saved)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser.samples.in_flight = false;
                // Changes to roots while a scan runs must not publish old results.
                if roots == view.sample_roots() {
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
                    view.apply_sample_catalog(scan.catalog, cx);
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

    pub(super) fn apply_sample_catalog(&mut self, catalog: SampleCatalog, cx: &mut Context<Self>) {
        let remap = matches!(
            self.browser.section,
            BrowserSection::All | BrowserSection::Samples | BrowserSection::Place(_)
        );
        let selection = remap.then(|| self.browser_selected_item(cx)).flatten();
        if self.browser.samples.replace(catalog) && remap {
            self.restore_browser_selection(selection, cx);
        }
    }

    pub(super) fn sample_filters(&self, cx: &mut Context<Self>) -> Div {
        let count = self.browser_visible_entries(cx).len();
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
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text(format!(
                            "{count} samples{}",
                            if self.browser.samples.in_flight {
                                " · scanning…"
                            } else {
                                ""
                            }
                        ))),
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
                )),
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
}
