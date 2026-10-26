//! Session-local browser locations, independent of project Undo and audio history.
use super::*;

const CAPACITY: usize = 64;
// The catalog already bounds native paths to 16KiB. Bound query retention too,
// without changing the live input or truncating a case-sensitive path/query.
const FIELD_BYTES: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Location {
    section: BrowserSection,
    directory: PathBuf,
    query: String,
}

#[derive(Clone, Default)]
struct Position {
    selected: Option<BrowserItemKey>,
    top: Option<BrowserItemKey>,
}

#[derive(Clone)]
struct Entry {
    location: Location,
    position: Position,
}

impl Entry {
    fn bounded(&self) -> bool {
        let path_fits = |path: &PathBuf| path.as_os_str().as_encoded_bytes().len() <= FIELD_BYTES;
        let key_fits = |key: &Option<BrowserItemKey>| match key {
            Some(BrowserItemKey::File(path)) | Some(BrowserItemKey::Plugin(path, _)) => {
                path_fits(path)
            }
            _ => true,
        };
        self.location.query.len() <= FIELD_BYTES
            && path_fits(&self.location.directory)
            && match &self.location.section {
                BrowserSection::Place(path) => path_fits(path),
                _ => true,
            }
            && key_fits(&self.position.selected)
            && key_fits(&self.position.top)
    }
}

#[derive(Default)]
pub(super) struct History {
    entries: Vec<Entry>,
    cursor: usize,
    editing_query: bool,
    pending: Option<Entry>,
}

impl History {
    pub(super) fn can_back(&self) -> bool {
        self.cursor > 0
    }
    pub(super) fn can_forward(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    fn save(&mut self, entry: Entry) -> bool {
        if !entry.bounded() {
            *self = Self::default();
            return false;
        }
        if self.entries.is_empty() {
            self.entries.push(entry);
        } else if self.pending.is_none() {
            self.entries[self.cursor] = entry;
        }
        true
    }

    fn push(&mut self, entry: Entry) -> bool {
        if !entry.bounded() {
            *self = Self::default();
            return false;
        }
        if self
            .entries
            .get(self.cursor)
            .is_some_and(|old| old.location == entry.location)
        {
            return true;
        }
        self.entries.truncate(self.cursor + 1);
        if self.entries.len() == CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push(entry);
        self.cursor = self.entries.len() - 1;
        true
    }

    fn step(&mut self, forward: bool) -> Option<Entry> {
        if forward {
            if !self.can_forward() {
                return None;
            }
            self.cursor += 1;
        } else {
            if !self.can_back() {
                return None;
            }
            self.cursor -= 1;
        }
        self.editing_query = false;
        let entry = self.entries[self.cursor].clone();
        self.pending = Some(entry.clone());
        Some(entry)
    }
}

impl EditorView {
    fn browser_history_entry(&self, cx: &App) -> Entry {
        let count = self.browser_visible_entries(cx).len()
            + self.browser_visible_devices(cx).len()
            + self.browser_visible_plugins(cx).len();
        let top = self.browser_history_top_index(count);
        Entry {
            location: Location {
                section: self.browser.section.clone(),
                directory: self.browser.directory.clone(),
                query: self.browser.search.read(cx).text().to_owned(),
            },
            position: Position {
                selected: self.browser_selected_item(cx),
                top: self.browser_item_at(top, cx),
            },
        }
    }

    fn browser_history_top_index(&self, count: usize) -> usize {
        let scroll = self.browser.scroll.0.borrow();
        // Both browser UniformLists have zero vertical padding and fixed
        // ROW_HEIGHT rows. Generic ScrollHandle child indexes do not describe
        // virtualized global rows; use the actual painted offset instead.
        let index = scroll.deferred_scroll_to_item.as_ref().map_or_else(
            || {
                let offset = -scroll.base_handle.offset().y / px(1.0);
                if offset.is_finite() && ROW_HEIGHT.is_finite() && ROW_HEIGHT > 0.0 {
                    (offset.max(0.0) / ROW_HEIGHT).floor() as usize
                } else {
                    0
                }
            },
            |pending| pending.item_index,
        );
        index.min(count.saturating_sub(1))
    }

    // Synchronize before navigation as well as from the observer: a queued observer
    // must not turn a restored query into another edit or reset restored selection.
    pub(super) fn sync_browser_query(&mut self, cx: &mut Context<Self>) {
        let query = self.browser.search.read(cx).text();
        if query == self.browser.observed_query {
            return;
        }
        let location = Location {
            section: self.browser.section.clone(),
            directory: self.browser.directory.clone(),
            query: query.to_owned(),
        };
        let editing = self.browser.history.editing_query;
        if self.browser.history.entries.is_empty() {
            let mut old = location.clone();
            old.query.clone_from(&self.browser.observed_query);
            self.browser.history.save(Entry {
                location: old,
                position: Position::default(),
            });
        }
        // Explicit query entry snapshots the old highlight/scroll BEFORE the
        // query changes. Never remap its old row index through the new filter.
        self.browser.history.pending = None;
        let next = Entry {
            location,
            position: Position::default(),
        };
        let retained = if editing {
            self.browser.history.save(next)
        } else {
            self.browser.history.push(next)
        };
        let history = &mut self.browser.history;
        let collapsed = editing
            && retained
            && history.cursor > 0
            && history.entries[history.cursor - 1].location
                == history.entries[history.cursor].location;
        let restored = if collapsed {
            history.entries.remove(history.cursor);
            history.cursor -= 1;
            Some(history.entries[history.cursor].clone())
        } else {
            None
        };
        history.editing_query = retained && !collapsed;
        self.browser.observed_query = self.browser.search.read(cx).text().to_owned();
        if !retained {
            self.browser.places_status = Some("browser history field limit reached".into());
        }
        self.reset_browser_results();
        // A type/delete round trip retains the predecessor's saved identity and
        // ends this text transaction; subsequent typing starts a new branch.
        self.browser.history.pending = restored;
    }

    /// A source choice or deliberate result navigation commits the current query
    /// transaction. No timer, per-character entries, or focus/paint bookkeeping.
    pub(super) fn checkpoint_browser_history(&mut self, cx: &mut Context<Self>) {
        self.sync_browser_query(cx);
        let entry = self.browser_history_entry(cx);
        if !self.browser.history.save(entry) {
            self.browser.places_status = Some("browser history field limit reached".into());
        }
        self.browser.history.editing_query = false;
        self.browser.history.pending = None;
    }

    pub(super) fn record_browser_location(&mut self, cx: &App) {
        let entry = self.browser_history_entry(cx);
        if !self.browser.history.push(entry) {
            self.browser.places_status = Some("browser history field limit reached".into());
        }
    }

    pub(super) fn move_browser_history(&mut self, forward: bool, cx: &mut Context<Self>) {
        let available = |history: &History| {
            if forward {
                history.can_forward()
            } else {
                history.can_back()
            }
        };
        if !available(&self.browser.history) {
            return;
        }
        self.sync_browser_query(cx);
        // A synchronous query change may have removed the painted Forward
        // branch or collapsed Back. Never checkpoint a now-disabled direction.
        if !available(&self.browser.history) {
            return;
        }
        self.checkpoint_browser_history(cx);
        let Some(mut entry) = self.browser.history.step(forward) else {
            return;
        };
        self.accept_browser_sample_phase(Phase::Idle);
        self.cancel_browser_sample_playback();
        let removed = matches!(&entry.location.section, BrowserSection::Place(path)
            if !self.browser.places.contains(path) || !self.browser_place_visible(path));
        if removed
            || !self
                .browser
                .layout
                .history_source_available(&entry.location.section)
        {
            // A configuration change must not resurrect an unconfigured source,
            // nor turn restoration into another pushed history entry.
            entry.location.section = BrowserSection::All;
            entry.location.directory.clone_from(&self.browser.directory);
            entry.position = Position::default();
            self.browser.layout.reveal_all();
            self.browser.places_status = Some("history source unavailable · showing all".into());
        }
        // Update the input's own caret/mark state without taking focus. A later
        // observer acknowledges this exact value and does not create another step.
        self.browser
            .observed_query
            .clone_from(&entry.location.query);
        self.browser.search.update(cx, |input, cx| {
            input.set_text(&entry.location.query, cx);
        });
        self.apply_browser_source(entry.location.section.clone(), cx);
        if self.browser.directory != entry.location.directory {
            self.request_browser_scan(entry.location.directory.clone(), cx);
        }
        // reset_results/scan setup may clear ordinary pending item selection;
        // this restoration is owned by the latest history intent instead.
        self.browser.history.pending = Some(entry);
        cx.notify();
    }

    pub(super) fn browser_item_at(&self, index: usize, cx: &App) -> Option<BrowserItemKey> {
        let files = self.browser_visible_entries(cx);
        let devices = self.browser_visible_devices(cx);
        let plugins = self.browser_visible_plugins(cx);
        let rows = BrowserRows::new(
            files.len(),
            devices.len(),
            plugins.len(),
            self.browser_devices_first(cx),
        );
        if rows.files.contains(&index) {
            return files
                .get(index - rows.files.start)
                .map(|entry| BrowserItemKey::File(entry.path.clone()));
        }
        if rows.devices.contains(&index) {
            return devices
                .get(index - rows.devices.start)
                .map(|(class, entry)| BrowserItemKey::Device(*class, entry.type_id));
        }
        plugins
            .get(index.checked_sub(rows.plugins.start)?)
            .map(|row| BrowserItemKey::Plugin(row.module.clone(), row.cid))
    }

    pub(super) fn browser_history_index(&self, key: &BrowserItemKey, cx: &App) -> Option<usize> {
        let files = self.browser_visible_entries(cx);
        let devices = self.browser_visible_devices(cx);
        let plugins = self.browser_visible_plugins(cx);
        let rows = BrowserRows::new(
            files.len(),
            devices.len(),
            plugins.len(),
            self.browser_devices_first(cx),
        );
        match key {
            BrowserItemKey::File(path) => files
                .iter()
                .position(|entry| &entry.path == path)
                .map(|i| rows.files.start + i),
            BrowserItemKey::Device(class, id) => devices
                .iter()
                .position(|(candidate, entry)| candidate == class && entry.type_id == *id)
                .map(|i| rows.devices.start + i),
            BrowserItemKey::Plugin(path, id) => plugins
                .iter()
                .position(|row| &row.module == path && row.cid == *id)
                .map(|i| rows.plugins.start + i),
        }
    }

    pub(super) fn restore_browser_history_position(&mut self, cx: &App) {
        let query = self.browser.search.read(cx).text();
        let indexed = samples::ResultsMode::for_query(query) == samples::ResultsMode::Indexed;
        let (listing, tree, catalog) = match self.browser.section {
            BrowserSection::Samples => (false, true, indexed),
            BrowserSection::All => (indexed, true, indexed),
            BrowserSection::Place(_) => (true, true, indexed),
            BrowserSection::Projects => (true, true, false),
            BrowserSection::Instruments
            | BrowserSection::Plugins
            | BrowserSection::Collection(_) => (false, false, false),
        };
        if (listing && self.browser.loading)
            || (tree
                && self
                    .browser
                    .tree
                    .history_loading_for(&self.browser_visible_entries(cx)))
            || (catalog && self.browser.samples.history_loading())
        {
            return;
        }
        let Some(entry) = self.browser.history.pending.take() else {
            return;
        };
        // A user edit/source choice cancels pending before reaching here. Workers
        // have their own scan/tree/catalog generations and cannot change location.
        if entry.location.section != self.browser.section
            || entry.location.directory != self.browser.directory
            || entry.location.query != self.browser.search.read(cx).text()
        {
            return;
        }
        let selected = entry
            .position
            .selected
            .as_ref()
            .and_then(|key| self.browser_history_index(key, cx));
        self.browser.results_focused = selected.is_some();
        self.browser.selected_index = selected.unwrap_or(0);
        let top = entry
            .position
            .top
            .as_ref()
            .and_then(|key| self.browser_history_index(key, cx))
            .unwrap_or(0);
        self.browser
            .scroll
            .scroll_to_item_strict(top, gpui::ScrollStrategy::Top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(index: usize) -> Entry {
        Entry {
            location: Location {
                section: BrowserSection::Collection(index),
                directory: "/samples".into(),
                query: String::new(),
            },
            position: Position::default(),
        }
    }
    #[test]
    fn bounded_locations_boundaries_branch_duplicates_and_oversized_fields() {
        let mut history = History::default();
        assert!(history.step(false).is_none());
        assert!(history.step(true).is_none());
        for index in 0..CAPACITY * 2 {
            assert!(history.push(entry(index)));
        }
        assert_eq!(history.entries.len(), CAPACITY);
        assert_eq!(history.cursor, CAPACITY - 1);
        assert!(history.step(true).is_none());
        for _ in 1..CAPACITY {
            assert!(history.step(false).is_some());
        }
        assert!(history.step(false).is_none());
        assert!(history.push(entry(CAPACITY)));
        assert_eq!(
            history.entries.len(),
            CAPACITY,
            "same location is not another step"
        );
        assert!(history.push(entry(999)));
        assert_eq!(history.entries.len(), 2);
        assert!(!history.can_forward());
        let mut oversized = entry(1000);
        oversized.location.query = "é".repeat(FIELD_BYTES);
        assert!(!history.push(oversized));
        assert!(
            history.entries.is_empty(),
            "refuse retention; never truncate the live query/path"
        );
        assert!(!history.can_back());
        assert!(!history.can_forward());
        assert!(history.save(entry(1001)));
        assert!(!history.can_back());
    }
}
