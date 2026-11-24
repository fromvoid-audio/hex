//! Browser places persistence, reconciliation, source selection, and directory scan requests.
use super::*;

impl EditorView {
    pub(in crate::editor) fn browser_scan_initial(&mut self, cx: &mut Context<Self>) {
        self.start_browser_places_load(cx);
        self.request_browser_scan(self.browser.directory.clone(), cx);
        self.start_sample_catalog(cx);
        self.start_plugin_catalog(cx);
    }

    pub(in crate::editor) fn start_browser_places_load(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.browser.preferences_path.clone() else {
            self.browser.preferences_loading = false;
            self.browser.preferences_writable = false;
            self.browser.places_status =
                Some("browser preferences unavailable · home directory is unset".into());
            return;
        };
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::load(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser_places_loaded(result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn browser_places_loaded(
        &mut self,
        loaded: Result<BrowserPlaces, hex_preferences::PreferencesError>,
        cx: &mut Context<Self>,
    ) {
        self.browser.preferences_loading = false;
        match loaded {
            Err(error) => {
                self.browser.preferences_writable = false;
                self.browser.places_status =
                    Some(places_error("could not load browser places", &error));
            }
            Ok(loaded) => {
                self.browser
                    .layout
                    .set_samples_visible(!loaded.builtin_hidden(BrowserBuiltin::Samples));
                self.browser.source_preferences = loaded.clone();
                self.browser.place_icons = loaded
                    .icon_assignments()
                    .map(|(path, id)| (path.to_owned(), id.to_owned()))
                    .collect();
                if self
                    .browser
                    .place_icons
                    .values()
                    .any(|id| !place_icons::known(id))
                    || [BrowserBuiltin::Home, BrowserBuiltin::Music]
                        .into_iter()
                        .filter_map(|source| loaded.builtin_icon(source))
                        .any(|id| !place_icons::known(id))
                {
                    self.browser.places_status =
                        Some("some place icons unavailable · showing folder".into());
                }
                let mut combined = loaded.additional().to_vec();
                let local_places = self
                    .browser
                    .places
                    .iter()
                    .filter(|path| **path != self.browser.home && **path != self.browser.music)
                    .cloned();
                for path in local_places {
                    if !combined.contains(&path) {
                        combined.push(path);
                    }
                }
                if combined.len() > PLACE_CAPACITY.saturating_sub(DEFAULT_PLACE_COUNT) {
                    self.browser.preferences_writable = false;
                    self.browser.places_status = Some(format!(
                        "browser places exceed the {} folder limit",
                        PLACE_CAPACITY
                    ));
                } else {
                    let additions = combined
                        .into_iter()
                        .filter(|path| !self.browser.places.contains(path))
                        .collect::<Vec<_>>();
                    self.browser.places.extend(additions);
                    if self.browser.preferences_dirty {
                        self.browser.preferences_dirty = false;
                        self.queue_browser_places_save(cx);
                    }
                }
            }
        }
        self.reconcile_browser_source_visibility(cx);
        self.refresh_sample_catalog(cx);
    }

    pub(in crate::editor) fn queue_browser_places_save(&mut self, cx: &mut Context<Self>) {
        if self.browser.preferences_loading {
            self.browser.preferences_dirty = true;
            self.browser.places_status = Some("saving browser places…".into());
            return;
        }
        if !self.browser.preferences_writable {
            return;
        }
        let additional = self
            .browser
            .places
            .iter()
            .filter(|path| **path != self.browser.home && **path != self.browser.music)
            .cloned();
        let places = BrowserPlaces::new(additional).and_then(|mut places| {
            places.vst3_scan_sources = self.browser.source_preferences.vst3_scan_sources.clone();
            for source in [
                BrowserBuiltin::Music,
                BrowserBuiltin::Home,
                BrowserBuiltin::Samples,
            ] {
                places.set_builtin_icon(
                    source,
                    self.browser.source_preferences.builtin_icon(source),
                )?;
                places.set_builtin_hidden(
                    source,
                    self.browser.source_preferences.builtin_hidden(source),
                );
            }
            for path in places.additional().to_vec() {
                places
                    .set_place_hidden(&path, self.browser.source_preferences.place_hidden(&path))?;
            }
            for (path, id) in &self.browser.place_icons {
                if places.additional().contains(path) {
                    places.set_icon(path, Some(id))?;
                }
            }
            Ok(places)
        });
        match places {
            Ok(places) => {
                self.browser.source_preferences = places.clone();
                self.browser.pending_places_save = Some(places);
                self.browser.places_status = Some("saving browser places…".into());
                self.start_browser_places_save(cx);
            }
            Err(error) => {
                self.browser.places_status =
                    Some(places_error("could not save browser places", &error));
            }
        }
    }

    pub(super) fn start_browser_places_save(&mut self, cx: &mut Context<Self>) {
        if self.browser.places_save_in_flight || self.browser.preferences_loading {
            return;
        }
        let Some(places) = self.browser.pending_places_save.take() else {
            return;
        };
        let Some(path) = self.browser.preferences_path.clone() else {
            self.browser.preferences_writable = false;
            self.browser.places_status =
                Some("browser preferences unavailable · home directory is unset".into());
            return;
        };
        self.browser.places_save_in_flight = true;
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::save(&path, &places)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser_places_saved(result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn browser_places_saved(
        &mut self,
        result: Result<(), hex_preferences::PreferencesError>,
        cx: &mut Context<Self>,
    ) {
        self.browser.places_save_in_flight = false;
        let error = result
            .err()
            .map(|error| places_error("could not save browser places", &error));
        if self.browser.pending_places_save.is_some() {
            // A queued save supersedes the data, not the user's notice: show
            // the failure now and keep the latest one until it is reported.
            if let Some(error) = error {
                self.browser.places_status = Some(error.clone());
                self.browser.unreported_places_save_error = Some(error);
            }
            self.start_browser_places_save(cx);
            return;
        }
        let earlier = self.browser.unreported_places_save_error.take();
        self.browser.places_status = Some(match (error, earlier) {
            (Some(error), _) => error,
            (None, Some(earlier)) => format!("{earlier} · a later save succeeded"),
            (None, None) => "browser places saved".into(),
        });
    }

    pub(super) fn select_browser_source(&mut self, source: BrowserSection, cx: &mut Context<Self>) {
        self.checkpoint_browser_history(cx);
        self.apply_browser_source(source, cx);
        self.record_browser_location(cx);
    }

    pub(super) fn apply_browser_source(&mut self, source: BrowserSection, cx: &mut Context<Self>) {
        self.browser.scans.generation = self.browser.scans.generation.saturating_add(1);
        self.browser.scans.pending = None;
        self.browser.loading = false;
        self.browser.error = None;
        self.browser.tree.source_changed();
        self.browser.section = source;
        self.reset_browser_results();
        match &self.browser.section {
            BrowserSection::Place(path) => self.request_browser_scan(path.clone(), cx),
            BrowserSection::All | BrowserSection::Projects => {
                if self
                    .browser
                    .listing
                    .as_ref()
                    .is_none_or(|listing| listing.directory != self.browser.directory)
                {
                    self.request_browser_scan(self.browser.directory.clone(), cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn reconcile_browser_source(&mut self, cx: &mut Context<Self>) {
        if matches!(&self.browser.section, BrowserSection::Place(path) if !self.browser.places.contains(path))
        {
            let mut catalog = self.browser.samples.catalog.clone();
            catalog.retain_roots(&self.sample_roots());
            self.browser.samples.replace(catalog);
            self.select_browser_source(BrowserSection::Samples, cx);
            self.refresh_sample_catalog(cx);
            self.browser.places_status = Some("selected place removed · showing samples".into());
        }
    }

    pub(super) fn request_browser_scan(&mut self, directory: PathBuf, cx: &mut Context<Self>) {
        if directory != self.browser.directory {
            self.reset_browser_results();
        }
        if matches!(self.browser.section, BrowserSection::Place(_)) {
            // Retain identity privately while unavailable rows are hidden. Query/source
            // changes clear this snapshot through reset_browser_results.
            if !self.browser.loading {
                self.browser.pending_selection = self.browser_selected_item(cx);
            }
            if self.browser.playback_path.is_some() || self.browser.playback_pending.is_some() {
                self.cancel_browser_sample_playback();
                self.status = "sample preview stopped".into();
            }
        }
        let start = self.browser.scans.request(directory.clone());
        self.browser.directory = directory.clone();
        self.browser.error = None;
        self.browser.loading = true;
        if let Some(request) = start {
            self.start_browser_scan(request, cx);
        }
    }

    pub(super) fn reset_browser_results(&mut self) {
        self.browser.pending_selection = None;
        if self.browser.playback_output.is_some() {
            self.cancel_browser_sample_playback();
            self.status = "sample preview stopped".into();
        }
        self.browser.reset_results();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn preferences_path_rejects_missing_or_empty_home_and_stays_under_home() {
        assert_eq!(preferences_path_for_home(None), None);
        assert_eq!(preferences_path_for_home(Some(Path::new(""))), None);
        let path = preferences_path_for_home(Some(Path::new("/Users/tester"))).unwrap();
        assert!(path.starts_with("/Users/tester"));
    }
}
