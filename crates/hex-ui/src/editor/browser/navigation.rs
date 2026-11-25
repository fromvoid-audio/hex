//! Browser keyboard navigation, search focus, selection, visible rows, and catalog snapshots.
use super::*;

impl EditorView {
    pub(in crate::editor) fn browser_content_width(&self, window: &Window) -> f32 {
        let content = if !self.browser.open {
            COLLAPSED_WIDTH
        } else {
            let base = self
                .clamped_browser_width(self.browser.width, f32::from(window.bounds().size.width));
            base + self.browser.layout.editor_width(base) + 4.0
        };
        content + 2.0 * hex_widgets::PANEL_FRAME_INSET
    }

    pub(in crate::editor) fn browser_key_context(&self, window: &Window, cx: &App) -> &'static str {
        if self
            .browser
            .search
            .read(cx)
            .focus_handle()
            .is_focused(window)
        {
            "hex_browser_search"
        } else if self.browser.results_focus.is_focused(window) {
            "hex_browser"
        } else {
            "hex_editor"
        }
    }

    pub(in crate::editor) fn search_browser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.gain_numeric_editing(cx) {
            self.error = Some(GAIN_SEARCH_ERROR.into());
            cx.notify();
            return;
        }
        if self
            .error
            .as_ref()
            .is_some_and(|error| error.as_ref() == GAIN_SEARCH_ERROR)
        {
            self.error = None;
        }
        if !super::super::device_panel::finalize_controls(self, cx) {
            cx.notify();
            return;
        }
        if self.clip_name_edit.is_some() {
            self.finish_clip_name(true, window, cx);
        }
        if self.settings.theme.open {
            self.cancel_theme_settings(window, cx);
            if self.settings.theme.open {
                return;
            }
        }
        if self.context_menu.is_some() {
            self.dismiss_context_menu(window, cx);
        }
        if self.controller_input_is_open() {
            self.close_controller_input_menu(window, cx);
        }
        self.browser.open = true;
        self.select_browser_source(BrowserSection::All, cx);
        self.browser.layout.reveal_all();
        self.reset_browser_results();
        self.browser.search.update(cx, |input, cx| {
            input.focus_and_select_all(window, cx);
        });
        cx.notify();
    }

    pub(in crate::editor) fn toggle_browser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.browser.open = !self.browser.open;
        if !self.browser.open {
            self.browser.layout.sidebar_drag = None;
            self.browser.resizing = false;
            self.browser.results_focused = false;
            self.focus.focus(window);
        }
        cx.notify();
    }

    pub(in crate::editor) fn browser_previous(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_browser_selection(false, cx);
        if self.browser.results_focused {
            self.browser.results_focus.focus(window);
        }
    }

    pub(in crate::editor) fn browser_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.move_browser_selection(true, cx);
        if self.browser.results_focused {
            self.browser.results_focus.focus(window);
        }
    }

    pub(super) fn move_browser_selection(&mut self, next: bool, cx: &mut Context<Self>) {
        self.checkpoint_browser_history(cx);
        let previous = (self.browser.results_focused, self.browser.selected_index);
        let count = self.browser_result_count(cx);
        let project_samples = self.browser.section == BrowserSection::ProjectSamples;
        let current = if project_samples {
            self.browser
                .project_sample_selection
                .as_ref()
                .and_then(|key| self.browser_history_index(key, cx))
        } else {
            self.browser
                .results_focused
                .then_some(self.browser.selected_index)
        };
        self.browser.selected_index = current.filter(|_| count > 0).map_or(0, |index| {
            let index = if next {
                index.saturating_add(1)
            } else {
                index.saturating_sub(1)
            };
            index.min(count - 1)
        });
        self.browser.results_focused = count > 0;
        if project_samples {
            let selection = self.browser_item_at(self.browser.selected_index, cx);
            if self.browser.project_sample_selection != selection {
                self.select_project_sample(selection, cx);
            }
        } else if previous != (self.browser.results_focused, self.browser.selected_index) {
            let path = self
                .browser_selected_entry(cx)
                .filter(|entry| entry.kind == EntryKind::WaveAudio)
                .map(|entry| entry.path);
            self.browser_selection_audition(path, cx);
        }
        self.browser.scroll.scroll_to_item(
            self.browser_scroll_row(self.browser.selected_index, cx),
            gpui::ScrollStrategy::Top,
        );
        cx.notify();
    }

    pub(in crate::editor) fn browser_activate(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.checkpoint_browser_history(cx);
        if self.browser.section == BrowserSection::ProjectSamples {
            if let Some(BrowserItemKey::ProjectSample(generation, revision, asset)) =
                self.browser_selected_item(cx)
                && !self
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            {
                self.insert_browser_project_sample(generation, revision, asset, cx);
            }
            cx.notify();
            return;
        }
        if let Some(BrowserItemKey::Plugin(module, cid)) = self.browser_selected_item(cx) {
            if !self
                .browser
                .search
                .read(cx)
                .focus_handle()
                .is_focused(window)
            {
                self.activate_browser_plugin(&module, cid, cx);
            }
            cx.notify();
            return;
        }

        if self.browser.section == BrowserSection::Instruments {
            let Some(BrowserItemKey::Device(hex_engine::NativeDeviceClass::Instrument, type_id)) =
                self.browser_selected_item(cx)
            else {
                self.error =
                    Some("select an available browser instrument before pressing enter".into());
                cx.notify();
                return;
            };
            // Selection is metadata only; native defaults are constructed by Service.
            let registered = hex_engine::native_device_catalog_for_class(
                hex_engine::NativeDeviceClass::Instrument,
            )
            .is_ok_and(|entries| entries.iter().any(|entry| entry.type_id == type_id));
            let Some(state) = self.state.as_ref().filter(|_| registered) else {
                self.error = Some("selected browser instrument is unavailable".into());
                cx.notify();
                return;
            };
            let gain = match self.settings.track_defaults.captured_gain() {
                Ok(gain) => gain,
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                    return;
                }
            };
            if self.enqueue_catalog_device(Request::InsertBrowserInstrumentTrack {
                project_generation: self.project_generation,
                revision: state.revision,
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id,
                new_track_gain: Some(gain),
                control: None,
            }) {
                self.reveal_session_track_proposal(None);
            }
            cx.notify();
            return;
        }
        let Some(entry) = self.browser_selected_entry(cx) else {
            if matches!(
                self.browser_selected_item(cx),
                Some(BrowserItemKey::Device(..))
            ) {
                self.status = "drag a browser device onto a track or device rail".into();
                cx.notify();
            }
            return;
        };
        match entry.kind {
            EntryKind::Directory => self.toggle_browser_folder(entry.path, false, cx),
            EntryKind::WaveAudio => {
                // Text/IME Enter must not import a sample. Non-file device activation
                // keeps its existing informational action, independent of this guard.
                if !self
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
                {
                    self.insert_browser_audio(entry.path, cx);
                }
            }
            EntryKind::Project => self.open_browser_project(entry.path, cx),
        }
        cx.notify();
    }

    pub(in crate::editor) fn browser_escape(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_plugin_effect();
        self.cancel_first_sampler_load(None);
        self.checkpoint_browser_history(cx);
        // Audio/device owners handle Escape first. A remaining browser folder
        // drag has no model gesture; retire it before clearing the query.
        if cx.stop_active_drag(window) {
            cx.notify();
            return;
        }
        if self
            .browser
            .search
            .read(cx)
            .focus_handle()
            .is_focused(window)
            || self.browser.results_focus.is_focused(window)
        {
            self.reset_browser_results();
            self.browser.search.update(cx, |input, cx| {
                input.clear(cx);
            });
        } else {
            self.browser.results_focused = true;
        }
        cx.notify();
    }

    pub(super) fn browser_selected_item(&self, cx: &App) -> Option<BrowserItemKey> {
        if !self.browser.results_focused {
            return None;
        }
        if self.browser.section == BrowserSection::ProjectSamples {
            self.browser.project_sample_selection.clone()
        } else {
            self.browser_item_at(self.browser.selected_index, cx)
        }
    }

    pub(super) fn project_sample_current(&self, key: &BrowserItemKey) -> bool {
        matches!(key, BrowserItemKey::ProjectSample(generation, revision, _)
            if *generation == self.project_generation
                && self.state.as_ref().is_some_and(|state| *revision == state.revision))
            && self.browser.section == BrowserSection::ProjectSamples
            && self.browser.results_focused
            && self.browser.project_sample_selection.as_ref() == Some(key)
    }

    pub(super) fn project_sample_waveform(
        &self,
        key: &BrowserItemKey,
    ) -> Option<Arc<hex_session::CachedWaveform>> {
        let BrowserItemKey::ProjectSample(_, _, asset) = key else {
            return None;
        };
        if !self.project_sample_current(key) {
            return None;
        }
        self.state.as_ref()?.waveforms.get(asset).cloned()
    }

    pub(super) fn select_project_sample(
        &mut self,
        key: Option<BrowserItemKey>,
        cx: &mut Context<Self>,
    ) {
        self.browser.project_sample_selection = key.clone();
        if let Some(key) = key
            && self.browser.auto_audition
        {
            self.play_browser_project_sample_from(&key, 0.0, cx);
        } else {
            self.stop_browser_preview_for_drop();
        }
    }

    pub(super) fn browser_selected_sample(&self, cx: &App) -> Option<PathBuf> {
        if !self.browser.results_focused {
            return None;
        }
        self.browser_selected_entry(cx)
            .filter(|entry| entry.kind == EntryKind::WaveAudio)
            .map(|entry| entry.path)
    }

    /// Reveal only the same stable sample painted by the control. A replacement
    /// result or selection cannot turn an old pointer event into a new selection.
    /// This view-only action must not enter the selection/audition request path.
    pub(super) fn reveal_browser_sample(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser_selected_sample(cx).as_deref() != Some(path) {
            return;
        }
        let index = self.browser.selected_index;
        self.browser
            .scroll
            .scroll_to_item(index, gpui::ScrollStrategy::Top);
        self.browser.results_focus.focus(window);
        cx.notify();
    }

    pub(super) fn restore_browser_selection(
        &mut self,
        selection: Option<BrowserItemKey>,
        cx: &App,
    ) {
        let index = selection
            .as_ref()
            .and_then(|key| self.browser_history_index(key, cx));
        if let Some(index) = index {
            self.browser.selected_index = index;
            self.browser.results_focused = true;
            self.browser.project_sample_selection =
                selection.filter(|key| matches!(key, BrowserItemKey::ProjectSample(..)));
            self.browser.scroll.scroll_to_item(
                self.browser_scroll_row(index, cx),
                gpui::ScrollStrategy::Top,
            );
        } else {
            self.reset_browser_results();
        }
    }

    /// The current sample catalog for relink lookups, and whether a scan is
    /// still running (so results may be incomplete). Callers search a clone on a
    /// background worker.
    pub(in crate::editor) fn sample_catalog_snapshot(
        &self,
    ) -> (hex_library::catalog::SampleCatalog, bool) {
        (
            self.browser.samples.catalog.clone(),
            self.browser.samples.history_loading(),
        )
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn test_replace_sample_catalog(
        &mut self,
        catalog: hex_library::catalog::SampleCatalog,
    ) -> bool {
        let settled = !self.browser.samples.history_loading();
        self.browser.samples.replace(catalog);
        settled
    }

    pub(super) fn browser_place_unavailable(&self) -> bool {
        matches!(self.browser.section, BrowserSection::Place(_))
            && (self.browser.loading || self.browser.error.is_some())
    }

    pub(super) fn browser_devices_first(&self, cx: &App) -> bool {
        self.browser.section == BrowserSection::All
            && !self.browser.search.read(cx).text().is_empty()
    }

    pub(super) fn browser_rows(&self, cx: &App) -> BrowserRows {
        let file_count = if self.browser.section == BrowserSection::ProjectSamples {
            self.browser_project_samples(cx).len()
        } else {
            self.browser_visible_entries(cx).len()
        };
        BrowserRows::new(
            file_count,
            self.browser_visible_devices(cx).len(),
            self.browser_visible_plugins(cx).len(),
            self.browser_devices_first(cx),
        )
    }

    pub(super) fn browser_result_count(&self, cx: &App) -> usize {
        let rows = self.browser_rows(cx);
        rows.files.len() + rows.devices.len() + rows.plugins.len()
    }

    pub(super) fn browser_selected_entry(&self, cx: &App) -> Option<LibraryEntry> {
        let local = self
            .browser
            .selected_index
            .checked_sub(self.browser_rows(cx).files.start)?;
        self.browser_visible_entries(cx).get(local).cloned()
    }

    pub(super) fn browser_project_samples(&self, cx: &App) -> Arc<Vec<ProjectSample>> {
        let Some(state) = &self.state else {
            self.browser.project_samples.borrow_mut().take();
            return Arc::new(Vec::new());
        };
        let query = self.browser.search.read(cx).text().to_lowercase();
        let mut cache = self.browser.project_samples.borrow_mut();
        if let Some(cached) = cache.as_ref().filter(|cached| {
            cached.generation == self.project_generation
                && cached.revision == state.revision
                && cached.query == query
        }) {
            return Arc::clone(&cached.rows);
        }
        let mut rows: Vec<_> = state
            .project
            .assets()
            .filter_map(|asset| {
                state.project.existing_import_frames(asset.id()).ok()?;
                let name = asset.path().file_name()?.to_string_lossy().into_owned();
                name.to_lowercase()
                    .contains(&query)
                    .then_some(ProjectSample {
                        asset: asset.id(),
                        name,
                    })
            })
            .collect();
        rows.sort_by_cached_key(|row| (row.name.to_lowercase(), row.asset));
        let rows = Arc::new(rows);
        *cache = Some(ProjectSampleCache {
            generation: self.project_generation,
            revision: state.revision,
            query,
            rows: Arc::clone(&rows),
        });
        rows
    }

    pub(super) fn browser_visible_entries(&self, cx: &App) -> Arc<Vec<LibraryEntry>> {
        if self.browser.section == BrowserSection::ProjectSamples {
            return Arc::new(Vec::new());
        }
        if self.browser_place_unavailable() {
            return Arc::new(Vec::new());
        }
        let query = self.browser.search.read(cx).text().to_lowercase();
        let listing = self
            .browser
            .listing
            .as_ref()
            .filter(|listing| listing.directory == self.browser.directory);
        let base = match &self.browser.section {
            BrowserSection::Samples
                if samples::ResultsMode::for_query(&query) == samples::ResultsMode::Hierarchy =>
            {
                self.browser.tree.roots(self.sample_roots())
            }
            BrowserSection::Samples => self.browser.samples.visible(&query),
            BrowserSection::Place(root) if !query.is_empty() => self
                .browser
                .samples
                .visible_with_listing(&query, Some(root), listing),
            BrowserSection::Place(_) => self.browser.samples.folder(&query, listing, false),
            BrowserSection::Collection(_) => Arc::new(Vec::new()),
            BrowserSection::All if query.is_empty() => {
                let mut roots = self.sample_roots();
                roots.push(self.browser.directory.clone());
                self.browser.tree.roots(roots)
            }
            BrowserSection::All => self
                .browser
                .samples
                .visible_with_listing(&query, None, listing),
            BrowserSection::Projects => self.browser.samples.folder(&query, listing, true),
            BrowserSection::Instruments
            | BrowserSection::Plugins
            | BrowserSection::ProjectSamples => Arc::new(Vec::new()),
        };
        self.browser.tree.visible(
            base,
            &query,
            match self.browser.section {
                BrowserSection::Samples => tree::Files::Samples,
                BrowserSection::Projects => tree::Files::Projects,
                _ => tree::Files::All,
            },
        )
    }

    pub(super) fn browser_visible_devices(
        &self,
        cx: &App,
    ) -> Vec<(
        hex_engine::NativeDeviceClass,
        hex_engine::NativeDeviceCatalogEntry,
    )> {
        if !matches!(
            self.browser.section,
            BrowserSection::All | BrowserSection::Instruments
        ) {
            return Vec::new();
        }
        let query = self.browser.search.read(cx).text().to_lowercase();
        self.browser
            .device_catalog
            .as_ref()
            .and_then(|catalog| catalog.as_ref().ok())
            .into_iter()
            .flatten()
            .filter(|(_, entry)| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_rows_orders_devices_first_only_when_requested() {
        let file_first = BrowserRows::new(3, 2, 4, false);
        assert_eq!(file_first.files, 0..3);
        assert_eq!(file_first.devices, 3..5);
        assert_eq!(file_first.plugins, 5..9);

        let devices_first = BrowserRows::new(3, 2, 4, true);
        assert_eq!(devices_first.devices, 0..2);
        assert_eq!(devices_first.plugins, 2..6);
        assert_eq!(devices_first.files, 6..9);
    }
}
