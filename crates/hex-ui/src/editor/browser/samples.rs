//! Sample catalog worker coordination and compact source filters.
use super::*;
use hex_library::catalog::{SampleCatalog, scan_samples};

type VisibleCache = Option<(String, Option<PathBuf>, Arc<Vec<LibraryEntry>>)>;

#[derive(Default)]
pub(super) struct SampleBrowser {
    pub(super) catalog: SampleCatalog,
    pub(super) source: Option<PathBuf>,
    in_flight: bool,
    pending: bool,
    loaded: bool,
    pub(super) status: Option<String>,
    visible: RefCell<VisibleCache>,
}

impl SampleBrowser {
    pub(super) fn visible(&self, query: &str) -> Arc<Vec<LibraryEntry>> {
        let mut cache = self.visible.borrow_mut();
        if let Some((previous, source, entries)) = cache.as_ref()
            && previous == query
            && *source == self.source
        {
            return entries.clone();
        }
        let entries = Arc::new(self.catalog.filtered(query, self.source.as_deref()));
        *cache = Some((query.to_owned(), self.source.clone(), entries.clone()));
        entries
    }

    pub(super) fn replace(&mut self, catalog: SampleCatalog) {
        if self.catalog != catalog {
            self.catalog = catalog;
            *self.visible.borrow_mut() = None;
        }
    }
}

impl EditorView {
    fn sample_roots(&self) -> Vec<PathBuf> {
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
                        view.browser.samples.loaded = true;
                        match cached {
                            Ok(mut catalog) if roots == view.sample_roots() => {
                                catalog.retain_roots(&roots);
                                view.browser.samples.replace(catalog);
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
                    let selected = (view.browser.section == BrowserSection::Samples)
                        .then(|| {
                            view.browser_visible_entries(cx)
                                .get(view.browser.selected_index)
                                .map(|entry| entry.path.clone())
                        })
                        .flatten();
                    view.browser.samples.replace(scan.catalog);
                    if view.browser.section == BrowserSection::Samples {
                        view.browser.selected_index = selected
                            .and_then(|path| {
                                view.browser_visible_entries(cx)
                                    .iter()
                                    .position(|entry| entry.path == path)
                            })
                            .unwrap_or(0);
                    }
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
                    |view, _, cx| view.refresh_sample_catalog(cx),
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
            let active = self.browser.samples.source == source;
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
                            view.browser.samples.source = source.clone();
                            view.browser.selected_index = 0;
                            view.browser
                                .scroll
                                .scroll_to_item(0, gpui::ScrollStrategy::Top);
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
