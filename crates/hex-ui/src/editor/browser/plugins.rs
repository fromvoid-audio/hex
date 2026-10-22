//! Explicit trusted VST3 discovery. Blocking process/catalog work stays off UI.
//!
//! One browser owner selects a trusted bundle or explicitly scans standard user/system
//! folders through the bounded catalog worker. Cancellation discards pending folder
//! results; skipped links and per-module failures remain visible. Startup restores
//! metadata without executing plugin code. Source bytes remain unchanged; display
//! passes through `theme::text`. Rows retain foreign UID/module identities and support
//! search/history in vst3/all, but expose no device insertion or playable hosting.
//! Custom roots and native interaction acceptance remain separate.
use super::*;
use hex_plugin_catalog::{Catalog, Row};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
enum PluginScan {
    Selected(PathBuf),
    Standard,
    #[cfg(all(test, feature = "ui-test-support"))]
    TestFolders(Vec<PathBuf>),
}

#[derive(Default)]
pub(super) struct Plugins {
    catalog: Catalog,
    projection: RefCell<Option<(String, Arc<Vec<Row>>)>>,
    pub(super) status: String,
    pub(super) loading: bool,
    pub(super) busy: bool,
    picking: bool,
    writable: bool,
    app: Option<PathBuf>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    cancellation: Option<hex_plugin_catalog::worker::Cancellation>,
    #[cfg(all(
        test,
        feature = "ui-test-support",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    test_roots: Option<Vec<PathBuf>>,
}
impl Plugins {
    fn replace(&mut self, catalog: Catalog) {
        self.catalog = catalog;
        self.projection.borrow_mut().take();
    }
    fn rows(&self, query: &str) -> Arc<Vec<Row>> {
        let mut cached = self.projection.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|(previous, _)| previous != query)
        {
            *cached = Some((query.into(), Arc::new(self.catalog.rows(query))));
        }
        cached
            .as_ref()
            .map_or_else(|| Arc::new(Vec::new()), |(_, rows)| Arc::clone(rows))
    }
}

impl EditorView {
    fn plugin_catalog_path(&self) -> PathBuf {
        self.browser
            .preferences_path
            .with_file_name("plugin-catalog.json")
    }
    pub(super) fn apply_plugin_catalog(&mut self, catalog: Catalog, cx: &App) {
        let selected = if matches!(
            self.browser.section,
            BrowserSection::All | BrowserSection::Plugins
        ) {
            self.browser_selected_item(cx)
        } else {
            None
        };
        self.browser.plugins.replace(catalog);
        if let Some(key) = selected {
            self.restore_browser_selection(Some(key), cx);
        }
    }
    pub(super) fn start_plugin_catalog(&mut self, cx: &mut Context<Self>) {
        if self.browser.plugins.loading || self.browser.plugins.busy {
            return;
        }
        self.browser.plugins.loading = true;
        let path = self.plugin_catalog_path();
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let (loaded, app) = cx
                .background_executor()
                .spawn(async move {
                    let catalog = Catalog::load(&path);
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let app = hex_plugin_catalog::worker::available_app();
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    let app: Result<PathBuf, String> =
                        Err("vst3 discovery requires native macos arm64".into());
                    (catalog, app)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser.plugins.loading = false;
                match loaded {
                    Ok(catalog) => {
                        view.apply_plugin_catalog(catalog, cx);
                        view.browser.plugins.writable = true;
                        view.browser.plugins.status =
                            "plugin catalog restored · hosting unavailable".into();
                    }
                    Err(error) => {
                        view.browser.plugins.writable = false;
                        view.browser.plugins.status =
                            format!("plugin catalog unavailable · {error}");
                    }
                }
                match app {
                    Ok(path) => view.browser.plugins.app = Some(path),
                    Err(error) => {
                        view.browser.plugins.app = None;
                        view.browser.plugins.status =
                            format!("{} · {error}", view.browser.plugins.status);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    #[cfg(all(
        test,
        feature = "ui-test-support",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    pub(super) fn set_plugin_test_app(&mut self, app: PathBuf, preferences: PathBuf) {
        self.browser.preferences_path = preferences;
        self.browser.plugins.app = Some(app);
        self.browser.plugins.writable = true;
    }
    #[cfg(all(
        test,
        feature = "ui-test-support",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    pub(super) fn set_plugin_test_roots(&mut self, roots: Vec<PathBuf>) {
        self.browser.plugins.test_roots = Some(roots);
    }
    pub(super) fn browser_visible_plugins(&self, cx: &App) -> Arc<Vec<Row>> {
        if !matches!(
            self.browser.section,
            BrowserSection::Plugins | BrowserSection::All
        ) {
            return Arc::new(Vec::new());
        }
        self.browser
            .plugins
            .rows(&self.browser.search.read(cx).content)
    }
    pub(super) fn plugin_controls(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.browser.plugins;
        let can_scan = state.app.is_some()
            && state.writable
            && !state.loading
            && !state.busy
            && !state.picking;
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .px_1()
            .py_1()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("scan-trusted-vst3")
                            .text_xs()
                            .cursor_pointer()
                            .text_color(rgb(theme::color(if can_scan {
                                theme::PaletteRole::Text
                            } else {
                                theme::PaletteRole::Muted
                            })))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _, cx| view.pick_trusted_plugin(cx)),
                            )
                            .child(theme::text("scan trusted vst3…")),
                    )
                    .when(state.busy, |row| {
                        row.child(
                            div()
                                .id("cancel-vst3-scan")
                                .text_xs()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|view, _, _, cx| {
                                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                                        if let Some(cancel) = &view.browser.plugins.cancellation {
                                            view.browser.plugins.status = if cancel.cancel() {
                                                "cancel requested · waiting for scanner cleanup"
                                                    .into()
                                            } else {
                                                "finishing catalog update".into()
                                            };
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(theme::text("cancel scan")),
                        )
                    })
                    .when(!state.busy && !state.loading && !state.picking, |row| {
                        row.child(
                            div()
                                .id("reload-plugin-catalog")
                                .text_xs()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|view, _, _, cx| view.start_plugin_catalog(cx)),
                                )
                                .child(theme::text("reload catalog")),
                        )
                    }),
            )
            .child(
                div().id("scan-standard-vst3")
                    .debug_selector(|| "browser-vst3-scan-standard".into())
                    .text_xs().cursor_pointer()
                    .text_color(rgb(theme::color(if can_scan { theme::PaletteRole::Text } else { theme::PaletteRole::Muted })))
                    .on_mouse_down(MouseButton::Left, cx.listener(|view, _, _, cx| view.scan_standard_plugins(cx)))
                    .child(theme::text("scan trusted standard folders")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(if state.loading {
                        "loading plugin catalog…"
                    } else {
                        &state.status
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(
                        "plugin code in selected bundles/folders runs as your user · discovery only",
                    )),
            )
    }
    fn pick_trusted_plugin(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.browser.plugins;
        if state.app.is_none() || !state.writable || state.loading || state.busy || state.picking {
            return;
        }
        state.picking = true;
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("scan trusted vst3 bundle · executes plugin code".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                view.browser.plugins.picking = false;
                match picked {
                    Ok(Ok(Some(paths))) if paths.len() == 1 => {
                        if let Some(path) = paths.into_iter().next() {
                            view.scan_trusted_plugin(path, cx);
                        }
                    }
                    Ok(Ok(_)) => {}
                    _ => view.browser.plugins.status = "plugin selection failed".into(),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn scan_trusted_plugin(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.start_plugin_scan(PluginScan::Selected(path), cx);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let _ = (path, cx);
    }
    fn scan_standard_plugins(&mut self, cx: &mut Context<Self>) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            #[cfg(all(test, feature = "ui-test-support"))]
            if let Some(roots) = self.browser.plugins.test_roots.clone() {
                self.start_plugin_scan(PluginScan::TestFolders(roots), cx);
                return;
            }
            self.start_plugin_scan(PluginScan::Standard, cx);
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let _ = cx;
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn start_plugin_scan(&mut self, request: PluginScan, cx: &mut Context<Self>) {
        {
            use hex_plugin_catalog::worker::{self, Cancellation};
            if self.browser.plugins.busy
                || self.browser.plugins.loading
                || !self.browser.plugins.writable
                || self.browser.plugins.picking
            {
                return;
            }
            let Some(app) = self.browser.plugins.app.clone() else {
                return;
            };
            let cancellation = Cancellation::default();
            let token = cancellation.token();
            self.browser.plugins.cancellation = Some(cancellation);
            self.browser.plugins.busy = true;
            self.browser.plugins.status = match &request {
                PluginScan::Selected(_) => "scanning selected trusted plugin…",
                _ => "scanning trusted standard folders…",
            }
            .into();
            let catalog = self.plugin_catalog_path();
            let this = cx.entity().downgrade();
            cx.spawn(async move |_, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        match request {
                            PluginScan::Selected(path) => worker::scan(&catalog, &app, path, token),
                            PluginScan::Standard => worker::scan_standard(&catalog, &app, token),
                            #[cfg(all(test, feature = "ui-test-support"))]
                            PluginScan::TestFolders(roots) => {
                                worker::scan_folders(&catalog, &app, &roots, token)
                            }
                        }
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.browser.plugins.busy = false;
                    view.browser.plugins.cancellation = None;
                    if let Some(catalog) = result.catalog {
                        view.apply_plugin_catalog(catalog, cx);
                    }
                    view.browser.plugins.status = result.status;
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
        }
    }
    pub(super) fn browser_plugin_row(
        &mut self,
        index: usize,
        row: Row,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let key = BrowserItemKey::Plugin(row.module.clone(), row.cid);
        div()
            .id(("browser-vst3", index))
            .debug_selector(move || format!("browser-vst3-row-{index}"))
            .h(px(ROW_HEIGHT))
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .bg(rgb(theme::color(
                if self.browser.results_focused && self.browser.selected_index == index {
                    theme::PaletteRole::Hover
                } else {
                    theme::PaletteRole::Surface
                },
            )))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    let Some(current) = view.browser_history_index(&key, cx) else {
                        return;
                    };
                    view.checkpoint_browser_history(cx);
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    cx.notify();
                }),
            )
            .child(div().text_xs().child(theme::text("vst3")))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(theme::text(row.name)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(row.status)),
            )
    }
}
