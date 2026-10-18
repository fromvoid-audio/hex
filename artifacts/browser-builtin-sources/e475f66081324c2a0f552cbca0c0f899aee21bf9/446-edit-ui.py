from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser.rs');s=p.read_text().replace('use hex_preferences::BrowserPlaces;','use hex_preferences::{BrowserBuiltin, BrowserPlaces};')
s=s.replace('    place_icons: HashMap<PathBuf, String>,','    place_icons: HashMap<PathBuf, String>,\n    source_preferences: BrowserPlaces,',1).replace('            place_icons: HashMap::new(),','            place_icons: HashMap::new(),\n            source_preferences: BrowserPlaces::default(),',1)
s=s.replace('            Ok(loaded) => {\n                self.browser.place_icons', '''            Ok(loaded) => {
                self.browser.layout.set_samples_visible(!loaded.builtin_hidden(BrowserBuiltin::Samples));
                self.browser.source_preferences = loaded.clone();
                self.browser.place_icons''',1)
s=s.replace('        self.refresh_sample_catalog(cx);\n    }\n\n    pub(super) fn queue_browser_places_save', '        self.reconcile_browser_source_visibility(cx);\n        self.refresh_sample_catalog(cx);\n    }\n\n    pub(super) fn queue_browser_places_save',1)
s=s.replace('        let places = BrowserPlaces::new(additional).and_then(|mut places| {','''        let places = BrowserPlaces::new(additional).and_then(|mut places| {
            for source in [BrowserBuiltin::Music, BrowserBuiltin::Home, BrowserBuiltin::Samples] {
                places.set_builtin_icon(source, self.browser.source_preferences.builtin_icon(source))?;
                places.set_builtin_hidden(source, self.browser.source_preferences.builtin_hidden(source));
            }
            for path in places.additional().to_vec() {
                places.set_place_hidden(&path, self.browser.source_preferences.place_hidden(&path))?;
            }''',1)
p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser/place_icons.rs');s=p.read_text()
s=s.replace('''        if path == self.browser.home || path == self.browser.music {
            return ICONS[0].1;
        }
        self.browser
            .place_icons
            .get(path)
            .and_then(|id| ICONS.iter().find(|(candidate, _, _)| candidate == id))''','''        self.browser_place_icon_id(path)
            .and_then(|id| ICONS.iter().find(|(candidate, _, _)| *candidate == id))''')
s=s.replace('''        path != self.browser.home
            && path != self.browser.music
            && self.browser.places.iter().any(|place| place == path)''','''        self.browser.places.iter().any(|place| place == path)
            && self.browser_place_visible(path)''')
s=s.replace('''        let selected = self
            .browser
            .place_icons
            .get(&path)''','''        let selected = self.browser_place_icon_id(&path)''').replace('candidate == id))\n            .map_or(0', '*candidate == id))\n            .map_or(0')
s=s.replace('''            )
            .collect(),''','''            )
            .chain(std::iter::once(MenuItem {
                label: "remove", swatch: None, checked: false,
                action: EditorContextAction::RemoveBrowserSource(BrowserSection::Place(path)),
            }))
            .collect(),''',1)
s=s.replace('''        if let Some(id) = id {
            self.browser.place_icons.insert(path, id.into());''','''        if let Some(source) = self.browser_builtin_place(&path) {
            // Validation is shared with custom icons; unknown saved IDs remain untouched until chosen/reset.
            if let Err(error) = self.browser.source_preferences.set_builtin_icon(source, id) {
                self.browser.places_status = Some(places_error("could not set source icon", &error));
                cx.notify();
                return;
            }
        } else if let Some(id) = id {
            self.browser.place_icons.insert(path, id.into());''')
s += '''
impl EditorView {
    fn browser_builtin_place(&self, path: &std::path::Path) -> Option<BrowserBuiltin> {
        if path == self.browser.music { Some(BrowserBuiltin::Music) }
        else if path == self.browser.home { Some(BrowserBuiltin::Home) }
        else { None }
    }

    fn browser_place_icon_id(&self, path: &std::path::Path) -> Option<&str> {
        match self.browser_builtin_place(path) {
            Some(source) => self.browser.source_preferences.builtin_icon(source),
            None => self.browser.place_icons.get(path).map(String::as_str),
        }
    }

    pub(super) fn browser_place_visible(&self, path: &std::path::Path) -> bool {
        match self.browser_builtin_place(path) {
            Some(source) => !self.browser.source_preferences.builtin_hidden(source),
            None => !self.browser.source_preferences.place_hidden(path),
        }
    }

    pub(in crate::editor) fn browser_source_removable(&self, source: &BrowserSection) -> bool {
        !self.browser.preferences_loading && self.browser.preferences_writable && match source {
            BrowserSection::Place(path) => self.browser.places.contains(path) && self.browser_place_visible(path),
            BrowserSection::Samples => self.browser.layout.samples_visible(),
            _ => false,
        }
    }

    pub(super) fn set_browser_source_visible(&mut self, source: BrowserSection, visible: bool, cx: &mut Context<Self>) {
        if self.browser.preferences_loading || !self.browser.preferences_writable { return; }
        match source {
            BrowserSection::Place(ref path) if self.browser.places.contains(path) => {
                if let Some(builtin) = self.browser_builtin_place(path) {
                    self.browser.source_preferences.set_builtin_hidden(builtin, !visible);
                } else {
                    // Newly added paths are also retained, even if their first save is still queued.
                    if !self.browser.source_preferences.additional().contains(path) {
                        self.browser.source_preferences = self.browser_preferences_snapshot_for_visibility();
                    }
                    if let Err(error) = self.browser.source_preferences.set_place_hidden(path, !visible) {
                        self.browser.places_status = Some(places_error("could not hide source", &error));
                        cx.notify();
                        return;
                    }
                }
            }
            BrowserSection::Samples => {
                self.browser.layout.set_samples_visible(visible);
                self.browser.source_preferences.set_builtin_hidden(BrowserBuiltin::Samples, !visible);
            }
            _ => return,
        }
        self.reconcile_browser_source_visibility(cx);
        self.queue_browser_places_save(cx);
        cx.notify();
    }

    fn browser_preferences_snapshot_for_visibility(&self) -> BrowserPlaces {
        let mut snapshot = BrowserPlaces::new(self.browser.places.iter()
            .filter(|path| **path != self.browser.home && **path != self.browser.music).cloned())
            .expect("browser place capacity is bounded at insertion");
        for source in [BrowserBuiltin::Home, BrowserBuiltin::Music, BrowserBuiltin::Samples] {
            snapshot.set_builtin_hidden(source, self.browser.source_preferences.builtin_hidden(source));
            let _ = snapshot.set_builtin_icon(source, self.browser.source_preferences.builtin_icon(source));
        }
        for path in snapshot.additional().to_vec() {
            let _ = snapshot.set_place_hidden(&path, self.browser.source_preferences.place_hidden(&path));
        }
        snapshot
    }

    fn reconcile_browser_source_visibility(&mut self, cx: &mut Context<Self>) {
        let hidden = match &self.browser.section {
            BrowserSection::Place(path) => !self.browser_place_visible(path),
            BrowserSection::Samples => !self.browser.layout.samples_visible(),
            _ => false,
        };
        if hidden {
            self.cancel_browser_sample_playback();
            self.browser.layout.reveal_all();
            self.select_browser_source(BrowserSection::All, cx);
        }
    }

    pub(super) fn open_browser_samples_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.browser_source_removable(&BrowserSection::Samples) { return; }
        self.cancel_time_selection_gesture();
        let previous_focus = self.context_menu.take().and_then(|menu| menu.previous_focus).or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            position, icon_grid: None, selected: 0, focus, previous_focus,
            items: vec![MenuItem { label: "remove", swatch: None, checked: false,
                action: EditorContextAction::RemoveBrowserSource(BrowserSection::Samples) }],
        });
        cx.notify();
    }
}
'''
p.write_text(s)
p=Path('crates/hex-ui/src/editor/context_menu.rs');s=p.read_text()
s=s.replace('pub(super) enum EditorContextAction {','pub(super) enum EditorContextAction {\n    RemoveBrowserSource(super::browser::BrowserSection),',1)
s=s.replace('''        match action {
            EditorContextAction::PlaceIcon''','''        match action {
            EditorContextAction::RemoveBrowserSource(source) => {
                if self.browser_source_removable(&source) {
                    self.set_browser_source_visible(source, false, cx);
                    self.browser.results_focus.focus(window);
                    window.prevent_default();
                }
            }
            EditorContextAction::PlaceIcon''',1)
s=s.replace('''                EditorContextAction::InsertTrack(_) => false,''','''                EditorContextAction::RemoveBrowserSource(source) => !self.browser_source_removable(source),
                EditorContextAction::InsertTrack(_) => false,''',1)
p.write_text(s)
# Shared popup: existing glyph mapping already permits trailing full-width actions, fix its actual size.
p=Path('crates/hex-ui/src/context_menu.rs');s=p.read_text()
s=s.replace('// Item zero is a full-width action; remaining items are glyph cells.','// Item zero and any items after the glyph catalog are full-width actions.')
s=s.replace('''ROW_HEIGHT
                + menu.items.len().saturating_sub(1).div_ceil(ICON_COLUMNS) as f32 * ICON_CELL
                + 13.0''','''ROW_HEIGHT * menu.items.len().saturating_sub(grid.map_or(0, Vec::len)) as f32
                + grid.map_or(0, Vec::len).div_ceil(ICON_COLUMNS) as f32 * ICON_CELL
                + 13.0''')
p.write_text(s)
