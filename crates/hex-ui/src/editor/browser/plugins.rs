//! Explicit trusted VST3 discovery. Blocking process/catalog work stays off UI.
//!
//! One browser owner selects a trusted bundle or explicitly scans standard user/system
//! folders through the bounded catalog worker. Cancellation discards pending folder
//! results; skipped links and per-module failures remain visible. Startup restores
//! metadata without executing plugin code. Source bytes remain unchanged; display
//! passes through `theme::text`. Rows retain foreign UID/module identities and support
//! search/history in vst3/all. Explicit effect activation copies exact metadata
//! into Session's insertion request; discovery itself never admits a source.
//! Custom folders use the same explicit, bounded batch worker as standard folders;
//! their results persist in the catalog, without automatic startup execution.
//! Native interaction acceptance remains separate.
use super::*;
use gpui::AnimationExt;
use hex_plugin_catalog::{Catalog, Row};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
enum PluginScan {
    #[cfg(all(test, feature = "ui-test-support"))]
    Selected(PathBuf),
    Configured(hex_preferences::Vst3ScanSources),
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
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) diagnostics: Option<hex_plugin_catalog::worker::ScanDiagnostics>,
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
    #[cfg(all(
        test,
        feature = "ui-test-support",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    pub(super) fn scan_evidence(&self) -> impl std::fmt::Debug + '_ {
        (&self.status, &self.diagnostics, &self.catalog)
    }
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
    pub(super) fn activate_browser_plugin(
        &mut self,
        module: &std::path::Path,
        cid: Option<[u8; 16]>,
        cx: &App,
    ) {
        let selected = cid
            .ok_or_else(|| "select an available vst3 class".to_string())
            .and_then(|cid| self.browser.plugins.catalog.select_plugin(module, cid));
        match selected {
            Ok(selection) => self.insert_browser_plugin_effect(selection, cx),
            Err(error) => self.error = Some(error.into()),
        }
    }

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
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.browser.plugins.diagnostics = None;
        }
        let path = self.plugin_catalog_path();
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let (loaded, app) = cx
                .background_executor()
                .spawn(async move {
                    let catalog = Catalog::load(&path).and_then(|mut catalog| {
                        catalog.refresh_sources()?;
                        Ok(catalog)
                    });
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
                        view.browser.plugins.status = "plugin catalog restored".into();
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
            .rows(self.browser.search.read(cx).text())
    }
    pub(super) fn plugin_controls(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.browser.plugins;
        let sources = &self.browser.source_preferences.vst3_scan_sources;
        let has_source = sources.system
            || sources.user
            || (sources.custom_enabled && sources.custom_folder.is_some());
        let can_scan = state.app.is_some()
            && state.writable
            && !state.loading
            && !state.busy
            && !state.picking
            && !self.browser.preferences_loading
            && self.browser.preferences_writable
            && has_source;
        let scan_button = div()
            .id("scan-vst3")
            .debug_selector(|| "browser-vst3-scan".into())
            .text_xs()
            .cursor_pointer()
            .tooltip(|_, cx| {
                layout::browser_label_tooltip(
                    "scan enabled folders · executes trusted plugin code",
                    cx,
                )
            })
            .text_color(rgb(theme::color(if can_scan || state.busy {
                theme::PaletteRole::Text
            } else {
                theme::PaletteRole::Muted
            })))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _, cx| {
                    if view.browser.plugins.busy {
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        if let Some(cancel) = &view.browser.plugins.cancellation {
                            view.browser.plugins.status = if cancel.cancel() {
                                "cancel requested · waiting for scanner cleanup".into()
                            } else {
                                "finishing catalog update".into()
                            };
                            cx.notify();
                        }
                    } else if can_scan {
                        view.scan_configured_plugins(cx);
                    }
                }),
            )
            .child(theme::text(if state.busy {
                "cancel scan"
            } else {
                "scan vst3"
            }));
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
                    .items_center()
                    .gap_2()
                    .children([scan_button])
                    .when(state.busy, |row| {
                        row.children([div()
                            .debug_selector(|| "browser-vst3-scan-activity".into())
                            .text_xs()
                            .child(theme::text("●"))
                            .with_animation(
                                "vst3-scan-pulse",
                                gpui::Animation::new(std::time::Duration::from_millis(900))
                                    .repeat(),
                                |dot, progress| {
                                    dot.opacity(0.35 + 0.65 * (1.0 - (2.0 * progress - 1.0).abs()))
                                },
                            )])
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(if state.loading {
                        "loading plugin catalog…"
                    } else if !has_source && !state.busy {
                        "enable a vst3 scan source in browser settings"
                    } else {
                        &state.status
                    })),
            )
    }

    pub(super) fn plugin_scan_settings(&self, cx: &mut Context<Self>) -> Div {
        let scan = &self.browser.source_preferences.vst3_scan_sources;
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_1()
            .text_xs()
            .child(theme::text("vst3 scan sources"));
        for (name, active) in [
            ("system", scan.system),
            ("user", scan.user),
            ("custom", scan.custom_enabled),
        ] {
            panel = panel.child(
                div()
                    .id(gpui::ElementId::Name(
                        format!("vst3-scan-source-{name}").into(),
                    ))
                    .debug_selector(move || format!("browser-vst3-source-{name}"))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            if view.browser.preferences_loading
                                || !view.browser.preferences_writable
                            {
                                return;
                            }
                            let scan = &mut view.browser.source_preferences.vst3_scan_sources;
                            let flag = match name {
                                "system" => &mut scan.system,
                                "user" => &mut scan.user,
                                _ => &mut scan.custom_enabled,
                            };
                            *flag = !*flag;
                            view.queue_browser_places_save(cx);
                            cx.notify();
                        }),
                    )
                    .child(theme::text(format!(
                        "{} {name}",
                        if active { "☑" } else { "□" }
                    ))),
            );
        }
        let folder = scan
            .custom_folder
            .as_deref()
            .and_then(std::path::Path::file_name);
        let folder_label = folder.map_or_else(
            || "choose vst3 folder…".into(),
            |name| hex_session::service::text::verbatim(name.to_string_lossy()),
        );
        panel
            .child(
                div()
                    .id("choose-vst3-folder")
                    .debug_selector(|| "browser-vst3-choose-folder".into())
                    .cursor_pointer()
                    .truncate()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _, cx| view.pick_plugin_scan_folder(cx)),
                    )
                    .child(theme::text(folder_label)),
            )
            .child(
                div()
                    .id("reload-plugin-catalog")
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _, cx| view.start_plugin_catalog(cx)),
                    )
                    .child(theme::text("reload catalog")),
            )
            .child(div().child(theme::text("scan runs trusted plugin code")))
    }

    fn pick_plugin_scan_folder(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.browser.plugins;
        if state.picking || self.browser.preferences_loading || !self.browser.preferences_writable {
            return;
        }
        state.picking = true;
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("choose vst3 scan folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                view.browser.plugins.picking = false;
                match picked {
                    Ok(Ok(Some(paths))) if paths.len() == 1 => {
                        let scan = &mut view.browser.source_preferences.vst3_scan_sources;
                        scan.custom_folder = paths.into_iter().next();
                        scan.custom_enabled = true;
                        view.queue_browser_places_save(cx);
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

    #[cfg(all(
        test,
        feature = "ui-test-support",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    pub(super) fn scan_trusted_plugin(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.start_plugin_scan(PluginScan::Selected(path), cx);
    }

    fn scan_configured_plugins(&mut self, cx: &mut Context<Self>) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            #[cfg(all(test, feature = "ui-test-support"))]
            if let Some(roots) = self.browser.plugins.test_roots.clone() {
                self.start_plugin_scan(PluginScan::TestFolders(roots), cx);
                return;
            }
            let scan = self.browser.source_preferences.vst3_scan_sources.clone();
            self.start_plugin_scan(PluginScan::Configured(scan), cx);
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
            self.browser.plugins.diagnostics = None;
            self.browser.plugins.status = "scanning vst3…".into();
            let catalog = self.plugin_catalog_path();
            let this = cx.entity().downgrade();
            cx.spawn(async move |_, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let mut update = match request {
                            #[cfg(all(test, feature = "ui-test-support"))]
                            PluginScan::Selected(path) => worker::scan(&catalog, &app, path, token),
                            PluginScan::Configured(scan) => {
                                let folder = scan.custom_folder.filter(|_| scan.custom_enabled);
                                match hex_plugin_catalog::roots::macos_scan_paths(
                                    scan.system,
                                    scan.user,
                                    folder,
                                ) {
                                    Ok(roots) => {
                                        worker::scan_folders(&catalog, &app, &roots, token)
                                    }
                                    Err(status) => worker::Update {
                                        catalog: None,
                                        diagnostics: None,
                                        status,
                                    },
                                }
                            }
                            #[cfg(all(test, feature = "ui-test-support"))]
                            PluginScan::TestFolders(roots) => {
                                worker::scan_folders(&catalog, &app, &roots, token)
                            }
                        };
                        if let Some(catalog) = &mut update.catalog
                            && let Err(error) = catalog.refresh_sources()
                        {
                            update.catalog = None;
                            update.status = format!("plugin source inspection failed · {error}");
                        }
                        update
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.browser.plugins.busy = false;
                    view.browser.plugins.cancellation = None;
                    view.browser.plugins.diagnostics = result.diagnostics;
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
        let payload = self
            .state
            .as_ref()
            .zip(row.selection)
            .map(|(state, selection)| drag::NativeDeviceDrag {
                project_generation: self.project_generation,
                revision: state.revision,
                source: drag::NativeDeviceDragSource::PluginEffect(selection),
                destination: Rc::new(Cell::new(None)),
                return_destination: Rc::new(Cell::new(None)),
            });
        let tooltip = format!(
            "{}\n{}",
            hex_session::service::text::verbatim(row.module.display()),
            row.status
        );
        let row = div()
            .id(("browser-vst3", index))
            .debug_selector(move || format!("browser-vst3-row-{index}"))
            .h(px(ROW_HEIGHT))
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .tooltip(move |_, cx| layout::browser_label_tooltip(&tooltip, cx))
            .bg(rgb(theme::color(
                if self.browser.results_focused && self.browser.selected_index == index {
                    theme::PaletteRole::Hover
                } else {
                    theme::PaletteRole::Surface
                },
            )))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    let Some(current) = view.browser_history_index(&key, cx) else {
                        return;
                    };
                    view.checkpoint_browser_history(cx);
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    if event.click_count >= 2 {
                        view.browser_activate(window, cx);
                    }
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
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(row.status)),
            );
        Self::with_browser_device_drag(row, payload, cx)
    }
}
