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
