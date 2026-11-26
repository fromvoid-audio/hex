//! Sample catalog worker coordination and compact source filters.
use super::*;
use hex_library::catalog::{SampleCatalog, ScanControl, scan_samples_with};
use std::sync::atomic::{AtomicBool, Ordering};

mod state;
pub(super) use state::{ResultsMode, SampleBrowser};

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
            .as_ref()
            .map(|path| path.with_extension("samples-v1.db"));
        #[cfg(test)]
        let test_environment = Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            if load {
                let cached = if let Some(path) = database.clone() {
                    #[cfg(test)]
                    let load_environment = Arc::clone(&test_environment);
                    Some(
                        cx.background_executor()
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
                            .await,
                    )
                } else {
                    None
                };
                if this
                    .update(cx, |view, cx| {
                        if view.browser.samples.loaded {
                            return;
                        }
                        view.browser.samples.loaded = true;
                        match cached {
                            Some(Ok(mut catalog)) if roots == view.sample_roots() => {
                                catalog.retain_roots(&roots);
                                view.apply_sample_catalog(catalog, cx);
                            }
                            Some(Err(error)) => {
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
                    } else if let Some(path) = database.as_deref() {
                        scan.catalog.save(path)
                    } else {
                        Ok(false)
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

    pub(super) fn persist_sample_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(database) = self
            .browser
            .preferences_path
            .as_ref()
            .map(|path| path.with_extension("samples-v1.db"))
        else {
            return;
        };
        let snapshot = self.browser.samples.catalog.clone();
        #[cfg(test)]
        let test_environment = Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    snapshot.save(&database)
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |view, cx| {
                    view.browser.samples.status = Some(format!(
                        "could not save tag changes · {}",
                        hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
                    ));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn add_sample_tag(
        &mut self,
        path: &std::path::Path,
        tag: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, &'static str> {
        let changed = self.browser.samples.catalog.add_tag(path, tag)?;
        if changed {
            self.browser.samples.invalidate_visible();
            self.browser.samples.status = None;
            self.persist_sample_catalog(cx);
        }
        Ok(changed)
    }

    pub(super) fn remove_sample_tag(
        &mut self,
        path: &std::path::Path,
        tag: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.browser.samples.catalog.remove_tag(path, tag);
        if changed {
            self.browser.samples.invalidate_visible();
            self.browser.samples.status = None;
            self.persist_sample_catalog(cx);
        }
        changed
    }

    pub(super) fn toggle_sample_tag(
        &mut self,
        path: &std::path::Path,
        tag: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, &'static str> {
        let assigned = self
            .browser
            .samples
            .catalog
            .tags_for(path)
            .is_some_and(|set| set.contains(tag));
        if assigned {
            Ok(self.remove_sample_tag(path, tag, cx))
        } else {
            self.add_sample_tag(path, tag, cx)
        }
    }

    /// Atomic bulk tag update across multiple sample paths. If every item already
    /// carries `tag`, removes it from all; otherwise assigns it to every item.
    /// Validation failure leaves the entire catalog unchanged.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn bulk_toggle_sample_tag(
        &mut self,
        paths: &[PathBuf],
        tag: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, &'static str> {
        if paths.is_empty() {
            return Ok(false);
        }
        let all_assigned = paths.iter().all(|path| {
            self.browser
                .samples
                .catalog
                .tags_for(path)
                .is_some_and(|set| set.contains(tag))
        });
        let snapshot = self.browser.samples.catalog.clone();
        let mut any_changed = false;
        for path in paths {
            if all_assigned {
                any_changed |= self.browser.samples.catalog.remove_tag(path, tag);
            } else {
                match self.browser.samples.catalog.add_tag(path, tag) {
                    Ok(changed) => any_changed |= changed,
                    Err(err) => {
                        self.browser.samples.catalog = snapshot;
                        return Err(err);
                    }
                }
            }
        }
        if any_changed {
            self.browser.samples.invalidate_visible();
            self.browser.samples.status = None;
            self.persist_sample_catalog(cx);
        }
        Ok(any_changed)
    }

    pub(super) fn toggle_sample_collection(
        &mut self,
        path: &std::path::Path,
        collection: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, &'static str> {
        let assigned = self
            .browser
            .samples
            .catalog
            .collection_members(collection)
            .is_some_and(|members| members.contains(path));
        let changed = if assigned {
            self.browser
                .samples
                .catalog
                .remove_from_collection(collection, path)
        } else {
            self.browser
                .samples
                .catalog
                .add_to_collection(collection, path)?
        };
        if changed {
            self.browser.samples.invalidate_visible();
            self.browser.samples.status = None;
            self.persist_sample_catalog(cx);
        }
        Ok(changed)
    }

    pub(super) fn create_browser_collection(
        &mut self,
        name: &str,
        color: hex_project::ElementColor,
        cx: &mut Context<Self>,
    ) -> Result<usize, &'static str> {
        let normalized = name.trim().to_lowercase();
        if normalized.is_empty()
            || normalized.len() > hex_preferences::MAX_COLLECTION_NAME_BYTES
            || normalized.chars().any(char::is_control)
        {
            return Err("invalid collection name");
        }
        if self
            .browser
            .layout
            .collections
            .iter()
            .any(|item| item.name == normalized)
        {
            return Err("collection already exists");
        }
        if self.browser.layout.collections.len() >= hex_preferences::MAX_BROWSER_COLLECTIONS {
            return Err("collection limit reached");
        }
        self.browser
            .samples
            .catalog
            .create_collection(&normalized)?;
        self.browser
            .layout
            .collections
            .push(layout::CollectionItem {
                name: normalized,
                color,
            });
        let index = self.browser.layout.collections.len() - 1;
        self.browser.samples.invalidate_visible();
        self.persist_sample_catalog(cx);
        self.sync_sidebar_preferences_and_save(cx);
        Ok(index)
    }

    pub(super) fn rename_browser_collection(
        &mut self,
        index: usize,
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        let Some(current) = self.browser.layout.collections.get(index) else {
            return Err("collection not found");
        };
        let old_name = current.name.clone();
        let normalized = new_name.trim().to_lowercase();
        if normalized.is_empty()
            || normalized.len() > hex_preferences::MAX_COLLECTION_NAME_BYTES
            || normalized.chars().any(char::is_control)
        {
            return Err("invalid collection name");
        }
        if normalized == old_name {
            return Ok(());
        }
        if self
            .browser
            .layout
            .collections
            .iter()
            .enumerate()
            .any(|(i, item)| i != index && item.name == normalized)
        {
            return Err("collection already exists");
        }
        if self
            .browser
            .samples
            .catalog
            .collection_members(&old_name)
            .is_some()
        {
            self.browser
                .samples
                .catalog
                .rename_collection(&old_name, &normalized)?;
        }
        if let Some(item) = self.browser.layout.collections.get_mut(index) {
            item.name = normalized;
        }
        self.browser.samples.invalidate_visible();
        self.persist_sample_catalog(cx);
        self.sync_sidebar_preferences_and_save(cx);
        Ok(())
    }

    pub(super) fn recolor_browser_collection(
        &mut self,
        index: usize,
        color: hex_project::ElementColor,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(item) = self.browser.layout.collections.get_mut(index) else {
            return false;
        };
        if item.color == color {
            return false;
        }
        item.color = color;
        self.sync_sidebar_preferences_and_save(cx);
        true
    }

    pub(super) fn reorder_browser_collection(
        &mut self,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let len = self.browser.layout.collections.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        let item = self.browser.layout.collections.remove(from);
        self.browser.layout.collections.insert(to, item);
        if let BrowserSection::Collection(active) = self.browser.section {
            let next_active = if active == from {
                to
            } else if from < active && active <= to {
                active - 1
            } else if to <= active && active < from {
                active + 1
            } else {
                active
            };
            self.browser.section = BrowserSection::Collection(next_active);
        }
        self.sync_sidebar_preferences_and_save(cx);
        true
    }

    pub(super) fn delete_browser_collection(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if index >= self.browser.layout.collections.len() {
            return false;
        }
        let removed = self.browser.layout.collections.remove(index);
        self.browser
            .samples
            .catalog
            .delete_collection(&removed.name);
        if let BrowserSection::Collection(active) = self.browser.section {
            if active == index {
                self.cancel_browser_sample_playback();
                let fallback = self.browser.layout.first_visible_section();
                self.select_browser_source(fallback, cx);
            } else if active > index {
                self.browser.section = BrowserSection::Collection(active - 1);
            }
        }
        self.browser.samples.invalidate_visible();
        self.persist_sample_catalog(cx);
        self.sync_sidebar_preferences_and_save(cx);
        true
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
