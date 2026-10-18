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
            Some(BrowserItemKey::File(path)) => path_fits(path),
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
        Entry {
            location: Location {
                section: self.browser.section.clone(),
                directory: self.browser.directory.clone(),
                query: self.browser.search.read(cx).content.clone(),
            },
            position: Position {
                selected: self.browser_selected_item(cx),
                top: self.browser_item_at(self.browser.scroll.logical_scroll_top_index(), cx),
            },
        }
    }

    // Synchronize before navigation as well as from the observer: a queued observer
    // must not turn a restored query into another edit or reset restored selection.
    pub(super) fn sync_browser_query(&mut self, cx: &mut Context<Self>) {
        let query = &self.browser.search.read(cx).content;
        if *query == self.browser.observed_query {
            return;
        }
        let location = Location {
            section: self.browser.section.clone(),
            directory: self.browser.directory.clone(),
            query: query.clone(),
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
        self.browser.history.editing_query = retained;
        self.browser.observed_query = self.browser.search.read(cx).content.clone();
        if !retained {
            self.browser.places_status = Some("browser history field limit reached".into());
        }
        self.reset_browser_results();
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
        self.checkpoint_browser_history(cx);
        let Some(entry) = self.browser.history.step(forward) else {
            return;
        };
        self.cancel_browser_sample_playback();
        // Update the input's own caret/mark state without taking focus. A later
        // observer acknowledges this exact value and does not create another step.
        self.browser
            .observed_query
            .clone_from(&entry.location.query);
        self.browser.search.update(cx, |input, cx| {
            input.restore_query(&entry.location.query, cx);
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

    fn browser_item_at(&self, index: usize, cx: &App) -> Option<BrowserItemKey> {
        let files = self.browser_visible_entries(cx);
        if let Some(entry) = files.get(index) {
            Some(BrowserItemKey::File(entry.path.clone()))
        } else {
            self.browser_visible_devices(cx)
                .get(index.checked_sub(files.len())?)
                .map(|(class, entry)| BrowserItemKey::Device(*class, entry.type_id))
        }
    }

    fn browser_history_index(&self, key: &BrowserItemKey, cx: &App) -> Option<usize> {
        let files = self.browser_visible_entries(cx);
        match key {
            BrowserItemKey::File(path) => files.iter().position(|entry| &entry.path == path),
            BrowserItemKey::Device(class, id) => self
                .browser_visible_devices(cx)
                .iter()
                .position(|(candidate, entry)| candidate == class && entry.type_id == *id)
                .map(|index| files.len() + index),
        }
    }

    pub(super) fn restore_browser_history_position(&mut self, cx: &App) {
        let files = !matches!(
            self.browser.section,
            BrowserSection::Instruments | BrowserSection::Collection(_)
        );
        let catalog = matches!(
            self.browser.section,
            BrowserSection::Samples | BrowserSection::All | BrowserSection::Place(_)
        );
        if self.browser.history.pending.is_none()
            || (files && (self.browser.loading || self.browser.tree.history_loading()))
            || (catalog && self.browser.samples.history_loading())
        {
            return;
        }
        let entry = self
            .browser
            .history
            .pending
            .take()
            .expect("checked pending history");
        // A user edit/source choice cancels pending before reaching here. Workers
        // have their own scan/tree/catalog generations and cannot change location.
        if entry.location.section != self.browser.section
            || entry.location.directory != self.browser.directory
            || entry.location.query != self.browser.search.read(cx).content
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
            .scroll_to_item(top, gpui::ScrollStrategy::Top);
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
