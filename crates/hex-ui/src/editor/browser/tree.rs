//! Bounded inline folder snapshots. Only the worker calls the filesystem.
use super::*;
use std::collections::{HashSet, VecDeque};
use std::path::Path;

const MAX_NODES: usize = 64;
const MAX_ENTRIES: usize = 8_192;
const MAX_PATH_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Files {
    All,
    Samples,
    Projects,
}

/// The visible folder state; loading and failure take priority over expansion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Disclosure {
    Loading,
    Error,
    Expanded,
    Collapsed,
}

#[derive(Clone)]
struct Request {
    epoch: u64,
    serial: u64,
    path: PathBuf,
}

#[derive(Default)]
struct Node {
    listing: Option<DirectoryListing>,
    error: Option<String>,
    queued: bool,
    serial: u64,
}

struct Snapshot {
    base: Arc<Vec<LibraryEntry>>,
    entries: Arc<Vec<LibraryEntry>>,
    depths: HashMap<PathBuf, usize>,
    query: String,
    files: Files,
}

struct RootCache {
    roots: Vec<PathBuf>,
    entries: Arc<Vec<LibraryEntry>>,
}

struct Projection<'a> {
    query: &'a str,
    files: Files,
    entries: Vec<LibraryEntry>,
    depths: HashMap<PathBuf, usize>,
    seen: HashSet<&'a Path>,
}

#[derive(Default)]
pub(super) struct Tree {
    nodes: HashMap<PathBuf, Node>,
    expanded: HashSet<PathBuf>,
    queue: VecDeque<PathBuf>,
    in_flight: Option<Request>,
    #[cfg(test)]
    test_scan_gate: Option<Arc<crate::editor::test_environment::CatalogSaveGate>>,
    epoch: u64,
    snapshot: RefCell<Option<Snapshot>>,
    root_cache: RefCell<Option<RootCache>>,
    next_serial: u64,
    dirty: Cell<bool>,
    pub(super) status: Option<String>,
    #[cfg(test)]
    pub(super) requests_started: usize,
}

impl Tree {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_gate_next_scan(
        &mut self,
    ) -> Arc<crate::editor::test_environment::CatalogSaveGate> {
        assert!(self.test_scan_gate.is_none());
        let gate = Arc::new(crate::editor::test_environment::CatalogSaveGate::default());
        self.test_scan_gate = Some(gate.clone());
        gate
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_worker_busy(&self) -> bool {
        self.in_flight.is_some()
    }

    pub(super) fn history_loading(&self) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|request| self.request_current(request))
            || !self.queue.is_empty()
            || self.dirty.get()
    }

    fn request_current(&self, request: &Request) -> bool {
        request.epoch == self.epoch
            && self.expanded.contains(&request.path)
            && self
                .nodes
                .get(&request.path)
                .is_some_and(|node| node.queued && node.serial == request.serial)
    }

    pub(super) fn history_loading_for(&self, entries: &[LibraryEntry]) -> bool {
        if !self.history_loading() {
            return false;
        }
        // Cached rows may augment indexed results too. Only expanded directories
        // actually present in this projection can change its saved row identities.
        let relevant = |path: &Path| {
            self.expanded.contains(path)
                && entries
                    .iter()
                    .any(|entry| entry.kind == EntryKind::Directory && entry.path == path)
        };
        self.dirty.get()
            || self.queue.iter().any(|path| relevant(path))
            || self
                .in_flight
                .as_ref()
                .is_some_and(|request| self.request_current(request) && relevant(&request.path))
    }

    pub(super) fn source_changed(&mut self) {
        self.dirty.set(true);
        self.status = None;
        self.epoch = self.epoch.saturating_add(1);
        self.queue.clear();
        for node in self.nodes.values_mut() {
            node.queued = false;
        }
        self.invalidate();
    }

    pub(super) fn roots(&self, mut roots: Vec<PathBuf>) -> Arc<Vec<LibraryEntry>> {
        roots.sort_by_key(|path| path.components().count());
        let mut distinct: Vec<PathBuf> = Vec::new();
        for root in roots {
            if !distinct.iter().any(|parent| root.starts_with(parent)) {
                distinct.push(root);
            }
        }
        let mut cache = self.root_cache.borrow_mut();
        if cache
            .as_ref()
            .is_none_or(|previous| previous.roots != distinct)
        {
            let rows = Arc::new(
                distinct
                    .iter()
                    .map(|path| LibraryEntry {
                        name: path
                            .file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy()
                            .into(),
                        path: path.clone(),
                        kind: EntryKind::Directory,
                    })
                    .collect(),
            );
            *cache = Some(RootCache {
                roots: distinct,
                entries: rows,
            });
            self.dirty.set(true);
            self.invalidate();
        }
        cache
            .as_ref()
            .map(|cached| cached.entries.clone())
            .unwrap_or_default()
    }

    fn invalidate(&self) {
        *self.snapshot.borrow_mut() = None;
    }

    fn queue(&mut self, path: PathBuf) {
        if self.queue.len() >= MAX_NODES
            || self.nodes.len() >= MAX_NODES && !self.nodes.contains_key(&path)
        {
            self.status = Some("folder cache limit reached · collapse a folder or refresh".into());
            return;
        }
        if self.nodes.get(&path).is_some_and(|node| node.queued) {
            return;
        }
        let Some(serial) = self.next_serial.checked_add(1) else {
            self.status = Some("folder request counter exhausted".into());
            return;
        };
        self.next_serial = serial;
        let node = self.nodes.entry(path.clone()).or_default();
        node.serial = serial;
        node.queued = true;
        self.queue.push_back(path);
    }

    fn next(&mut self) -> Option<Request> {
        if self.in_flight.is_some() {
            return None;
        }
        while let Some(path) = self.queue.pop_front() {
            if self.expanded.contains(&path) {
                #[cfg(test)]
                {
                    self.requests_started += 1;
                }
                let serial = self.nodes.get(&path).map_or(0, |node| node.serial);
                let request = Request {
                    epoch: self.epoch,
                    serial,
                    path,
                };
                self.in_flight = Some(request.clone());
                return Some(request);
            }
            if let Some(node) = self.nodes.get_mut(&path) {
                node.queued = false;
            }
        }
        None
    }

    /// Closing a branch releases its descendants; unrelated branches retain their state.
    fn close(&mut self, path: &Path) {
        self.expanded.retain(|key| !key.starts_with(path));
        self.nodes.retain(|key, _| !key.starts_with(path));
        self.queue.retain(|key| !key.starts_with(path));
        self.status = None;
        self.invalidate();
    }

    fn open(&mut self, path: PathBuf, keep_others: bool) {
        if !keep_others {
            // Live12 documented default: close other branches, retain ancestors.
            // Explicit Cmd-disclosure retains siblings.
            let closing: Vec<_> = self
                .expanded
                .iter()
                .filter(|other| !path.starts_with(other) && !other.starts_with(&path))
                .cloned()
                .collect();
            for other in closing {
                self.close(&other);
            }
        }
        if self.expanded.len() >= MAX_NODES && !self.expanded.contains(&path) {
            self.status = Some("folder expansion limit reached · collapse a folder".into());
            return;
        }
        self.expanded.insert(path.clone());
        if self
            .nodes
            .get(&path)
            .is_none_or(|node| node.listing.is_none() && node.error.is_none())
        {
            self.queue(path);
        }
        self.invalidate();
    }

    fn complete(&mut self, request: Request, result: Result<DirectoryListing, ScanError>) {
        self.in_flight = None;
        self.dirty.set(true);
        if request.epoch != self.epoch
            || !self.expanded.contains(&request.path)
            || self
                .nodes
                .get(&request.path)
                .is_none_or(|node| !node.queued || node.serial != request.serial)
        {
            return;
        }
        let mut node = self.nodes.remove(&request.path).unwrap_or_default();
        node.queued = false;
        match result {
            Ok(mut listing) => {
                let directories: Vec<_> = listing
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.kind == EntryKind::Directory
                            && entry.path.parent() == Some(request.path.as_path())
                    })
                    .map(|entry| entry.path.clone())
                    .collect();
                self.nodes.retain(|path, _| {
                    !path.starts_with(&request.path)
                        || directories.iter().any(|child| path.starts_with(child))
                });
                self.expanded.retain(|path| {
                    *path == request.path
                        || !path.starts_with(&request.path)
                        || directories.iter().any(|child| path.starts_with(child))
                });
                let used = self
                    .nodes
                    .values()
                    .filter_map(|node| node.listing.as_ref())
                    .map(|listing| listing.entries.len())
                    .sum::<usize>();
                let used_bytes = self
                    .nodes
                    .values()
                    .filter_map(|node| node.listing.as_ref())
                    .flat_map(|listing| &listing.entries)
                    .map(|entry| entry.path.as_os_str().as_encoded_bytes().len())
                    .sum::<usize>();
                let mut bytes = used_bytes;
                let before = listing.entries.len();
                listing.entries.retain(|entry| {
                    let length = entry.path.as_os_str().as_encoded_bytes().len();
                    bytes = bytes.saturating_add(length);
                    entry.path.parent() == Some(request.path.as_path())
                        && length <= 16_384
                        && bytes <= MAX_PATH_BYTES
                });
                listing.entries.truncate(MAX_ENTRIES.saturating_sub(used));
                listing.truncated |= listing.entries.len() != before;
                if listing.truncated {
                    self.status =
                        Some("folder tree capped · use a smaller folder or indexed search".into());
                }
                node.error = None;
                let reload: Vec<_> = listing
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.kind == EntryKind::Directory && self.expanded.contains(&entry.path)
                    })
                    .map(|entry| entry.path.clone())
                    .collect();
                node.listing = Some(listing);
                self.nodes.insert(request.path, node);
                for child in reload {
                    if !self.nodes.contains_key(&child) {
                        self.queue(child);
                    }
                }
            }
            Err(error) => {
                self.nodes
                    .retain(|path, _| !path.starts_with(&request.path));
                self.expanded
                    .retain(|path| *path == request.path || !path.starts_with(&request.path));
                node.listing = None;
                node.error = Some(error.user_message().into());
                self.nodes.insert(request.path, node);
            }
        }
        self.invalidate();
    }

    pub(super) fn visible(
        &self,
        base: Arc<Vec<LibraryEntry>>,
        query: &str,
        files: Files,
    ) -> Arc<Vec<LibraryEntry>> {
        let mut cache = self.snapshot.borrow_mut();
        if let Some(previous) = cache.as_ref()
            && Arc::ptr_eq(&previous.base, &base)
            && previous.query == query
            && previous.files == files
        {
            return previous.entries.clone();
        }
        if !base
            .iter()
            .any(|entry| entry.kind == EntryKind::Directory && self.expanded.contains(&entry.path))
        {
            let entries = base.clone();
            *cache = Some(Snapshot {
                base,
                entries: entries.clone(),
                depths: HashMap::new(),
                query: query.into(),
                files,
            });
            return entries;
        }
        let mut projection = Projection {
            query,
            files,
            entries: Vec::new(),
            depths: HashMap::new(),
            seen: HashSet::new(),
        };
        for entry in base.iter() {
            self.append(entry, 0, &mut projection);
        }
        let entries = Arc::new(projection.entries);
        *cache = Some(Snapshot {
            base: base.clone(),
            entries: entries.clone(),
            depths: projection.depths,
            query: query.into(),
            files,
        });
        entries
    }

    fn append<'a>(&'a self, entry: &'a LibraryEntry, depth: usize, output: &mut Projection<'a>) {
        if depth > MAX_DEPTH || !output.seen.insert(entry.path.as_path()) {
            return;
        }
        if depth > 0 {
            output.depths.insert(entry.path.clone(), depth);
        }
        output.entries.push(entry.clone());
        if entry.kind == EntryKind::Directory
            && self.expanded.contains(&entry.path)
            && let Some(listing) = self
                .nodes
                .get(&entry.path)
                .and_then(|node| node.listing.as_ref())
        {
            for child in &listing.entries {
                if (output.files == Files::Projects && child.kind == EntryKind::WaveAudio)
                    || (output.files == Files::Samples && child.kind == EntryKind::Project)
                {
                    continue;
                }
                let path = child.path.to_string_lossy().to_lowercase();
                if output
                    .query
                    .split_whitespace()
                    .all(|term| path.contains(term))
                {
                    self.append(child, depth + 1, output);
                }
            }
        }
    }

    pub(super) fn depth(&self, path: &Path) -> usize {
        self.snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.depths.get(path))
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn disclosure(&self, path: &Path) -> Disclosure {
        match self.nodes.get(path) {
            Some(node) if node.queued => Disclosure::Loading,
            Some(node) if node.error.is_some() => Disclosure::Error,
            _ if self.expanded.contains(path) => Disclosure::Expanded,
            _ => Disclosure::Collapsed,
        }
    }

    pub(super) fn error(&self, path: &Path) -> Option<&str> {
        self.nodes.get(path).and_then(|node| node.error.as_deref())
    }

    fn refresh(&mut self) {
        // The rendered snapshot may be filtered or contain only indexed files.
        // Refresh the already-bounded expansion set, retaining cached rows until
        // each replacement arrives. Parents run first so removed branches retire
        // their queued descendants before those descendants can start.
        let mut expanded: Vec<_> = self.expanded.iter().cloned().collect();
        expanded.sort_by(|left, right| {
            left.components()
                .count()
                .cmp(&right.components().count())
                .then_with(|| left.cmp(right))
        });
        self.source_changed();
        self.status = None;
        for path in expanded {
            self.queue(path);
        }
    }
}

impl EditorView {
    fn start_browser_tree_worker(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.browser.tree.next() else {
            return;
        };
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        #[cfg(test)]
        let test_gate = self.browser.tree.test_scan_gate.take();
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let path = request.path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    #[cfg(test)]
                    if let Some(gate) = test_gate {
                        gate.wait().await;
                    }
                    scan_directory(path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let selection = view.browser_selected_item(cx);
                view.browser.tree.complete(request, result);
                view.restore_browser_tree_selection(selection, cx);
                view.start_browser_tree_worker(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn restore_browser_tree_selection(
        &mut self,
        selection: Option<BrowserItemKey>,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = selection else {
            return;
        };
        let entries = self.browser_visible_entries(cx);
        let index = match selection {
            BrowserItemKey::File(path) => {
                if let Some(index) = entries.iter().position(|entry| entry.path == path) {
                    if entries[index].kind != EntryKind::WaveAudio {
                        self.browser_selection_audition(None, cx);
                    }
                    Some(self.browser_rows(cx).files.start + index)
                } else {
                    self.browser_selection_audition(None, cx);
                    entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| {
                            entry.kind == EntryKind::Directory && path.starts_with(&entry.path)
                        })
                        .max_by_key(|(_, entry)| entry.path.components().count())
                        .map(|(index, _)| self.browser_rows(cx).files.start + index)
                }
            }
            BrowserItemKey::Device(class, id) => {
                self.browser_history_index(&BrowserItemKey::Device(class, id), cx)
            }
            BrowserItemKey::Plugin(path, id) => {
                self.browser_history_index(&BrowserItemKey::Plugin(path, id), cx)
            }
        };
        if let Some(index) = index {
            self.browser.selected_index = index;
        } else {
            self.reset_browser_results();
        }
    }

    pub(super) fn toggle_browser_folder(
        &mut self,
        path: PathBuf,
        keep_others: bool,
        cx: &mut Context<Self>,
    ) {
        self.checkpoint_browser_history(cx);
        let selection = self.browser_selected_item(cx);
        if self.browser.tree.expanded.contains(&path) {
            self.browser.tree.close(&path);
        } else if self.browser.tree.depth(&path) >= MAX_DEPTH {
            self.browser.tree.status = Some("folder tree depth limit reached".into());
        } else {
            self.browser.tree.open(path.clone(), keep_others);
        }
        self.restore_browser_tree_selection(selection, cx);
        // A collapsed selected descendant falls back to its visible ancestor.
        if let Some(index) = self
            .browser_visible_entries(cx)
            .iter()
            .position(|entry| entry.path == path)
            && !self.browser.results_focused
        {
            self.browser.selected_index = self.browser_rows(cx).files.start + index;
            self.browser.results_focused = true;
        }
        self.start_browser_tree_worker(cx);
        cx.notify();
    }

    pub(in crate::editor) fn browser_collapse_folder(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.browser.results_focused {
            return;
        }
        self.checkpoint_browser_history(cx);
        let entries = self.browser_visible_entries(cx);
        let Some(entry) = self.browser_selected_entry(cx) else {
            return;
        };
        if entry.kind == EntryKind::Directory && self.browser.tree.expanded.contains(&entry.path) {
            self.toggle_browser_folder(entry.path.clone(), true, cx);
        } else if let Some(index) = entries.iter().position(|candidate| {
            candidate.kind == EntryKind::Directory
                && entry.path.parent() == Some(candidate.path.as_path())
        }) {
            let index = self.browser_rows(cx).files.start + index;
            self.browser.selected_index = index;
            self.browser.results_focused = true;
            self.browser_selection_audition(None, cx);
            self.browser
                .scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
            cx.notify();
        }
    }

    pub(super) fn expand_browser_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.browser.tree.expanded.contains(&path) {
            self.toggle_browser_folder(path, false, cx);
        } else {
            let entries = self.browser_visible_entries(cx);
            if let Some(index) = entries
                .iter()
                .position(|entry| entry.path.parent() == Some(path.as_path()))
            {
                let entry = &entries[index];
                let index = self.browser_rows(cx).files.start + index;
                self.browser.selected_index = index;
                self.browser.results_focused = true;
                self.browser_selection_audition(
                    (entry.kind == EntryKind::WaveAudio).then(|| entry.path.clone()),
                    cx,
                );
                self.browser
                    .scroll
                    .scroll_to_item(index, gpui::ScrollStrategy::Top);
                cx.notify();
            }
        }
    }

    pub(super) fn refresh_browser_tree(&mut self, cx: &mut Context<Self>) {
        self.browser.tree.refresh();
        if matches!(
            self.browser.section,
            BrowserSection::Place(_) | BrowserSection::Projects | BrowserSection::All
        ) {
            self.request_browser_scan(self.browser.directory.clone(), cx);
        }
        self.refresh_sample_catalog(cx);
        self.start_browser_tree_worker(cx);
        cx.notify();
    }

    pub(super) fn reconcile_browser_tree_requests(&mut self, cx: &mut Context<Self>) {
        if !self.browser.tree.dirty.replace(false) {
            return;
        }
        let entries = self.browser_visible_entries(cx);
        if self.browser.tree.dirty.get() {
            // GPUI suppresses invalidation during render. Notify after this
            // projection finishes, even when no folder worker starts; stable
            // roots stay clean, so no further callback is scheduled.
            let entity = cx.entity().downgrade();
            cx.defer(move |cx| {
                if let Some(entity) = entity.upgrade() {
                    cx.notify(entity.entity_id());
                }
            });
        }
        let needed: Vec<_> = entries
            .iter()
            .filter(|entry| {
                entry.kind == EntryKind::Directory
                    && self.browser.tree.expanded.contains(&entry.path)
                    && self.browser.tree.nodes.get(&entry.path).is_none_or(|node| {
                        node.listing.is_none() && node.error.is_none() && !node.queued
                    })
            })
            .map(|entry| entry.path.clone())
            .collect();
        for path in needed {
            self.browser.tree.queue(path);
        }
        self.start_browser_tree_worker(cx);
    }
}

/// Collision-free element identity from native path bytes; display labels remain names only.
pub(super) fn row_id(path: &Path) -> String {
    use std::fmt::Write;
    let mut id = String::from("browser-path-");
    for byte in path.as_os_str().as_encoded_bytes() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(path: impl Into<PathBuf>, kind: EntryKind) -> LibraryEntry {
        let path = path.into();
        LibraryEntry {
            name: path.file_name().unwrap().to_string_lossy().into(),
            path,
            kind,
        }
    }
    fn listing(path: &Path, entries: Vec<LibraryEntry>) -> DirectoryListing {
        DirectoryListing {
            directory: path.into(),
            enumerated_entries: entries.len(),
            entries,
            truncated: false,
        }
    }

    #[test]
    fn late_collapse_reopen_and_source_results_cannot_replace_new_request() {
        let root = PathBuf::from("/tree");
        let mut tree = Tree::default();
        let base = tree.roots(vec![root.clone()]);
        assert_eq!(
            tree.requests_started, 0,
            "collapsed roots require no enumeration"
        );
        tree.open(root.clone(), false);
        let old = tree.next().unwrap();
        tree.close(&root);
        tree.open(root.clone(), false);
        assert!(
            tree.next().is_none(),
            "one worker even across collapse/reopen"
        );
        tree.complete(
            old,
            Ok(listing(
                &root,
                vec![entry(root.join("old.wav"), EntryKind::WaveAudio)],
            )),
        );
        assert_eq!(tree.visible(base.clone(), "", Files::All).len(), 1);
        let current = tree.next().unwrap();
        tree.complete(
            current,
            Ok(listing(
                &root,
                vec![entry(root.join("new.wav"), EntryKind::WaveAudio)],
            )),
        );
        assert_eq!(
            tree.visible(base.clone(), "", Files::All)[1].name,
            "new.wav"
        );
        tree.refresh();
        let old_source = tree.next().unwrap();
        tree.source_changed();
        tree.complete(
            old_source,
            Ok(listing(
                &root,
                vec![entry(root.join("stale.wav"), EntryKind::WaveAudio)],
            )),
        );
        assert_eq!(tree.visible(base, "", Files::All)[1].name, "new.wav");
    }

    #[test]
    fn siblings_filter_refresh_error_and_same_names_keep_native_identity() {
        let root = PathBuf::from("/tree");
        let left = root.join("left");
        let right = root.join("right");
        let mut tree = Tree::default();
        let base = Arc::new(vec![
            entry(left.clone(), EntryKind::Directory),
            entry(right.clone(), EntryKind::Directory),
        ]);
        tree.open(left.clone(), false);
        let request = tree.next().unwrap();
        tree.complete(
            request,
            Ok(listing(
                &left,
                vec![entry(left.join("kick.wav"), EntryKind::WaveAudio)],
            )),
        );
        tree.open(right.clone(), true);
        let request = tree.next().unwrap();
        tree.complete(
            request,
            Ok(listing(
                &right,
                vec![entry(right.join("kick.wav"), EntryKind::WaveAudio)],
            )),
        );
        let rows = tree.visible(base.clone(), "", Files::All);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].name, rows[3].name);
        assert_ne!(row_id(&rows[1].path), row_id(&rows[3].path));
        assert!(Arc::ptr_eq(
            &rows,
            &tree.visible(base.clone(), "", Files::All)
        ));
        assert_eq!(tree.visible(base.clone(), "left kick", Files::All).len(), 3);
        assert_eq!(
            tree.visible(base.clone(), "", Files::Projects).len(),
            2,
            "Projects excludes WAV children"
        );
        tree.visible(base.clone(), "", Files::All);
        tree.refresh();
        assert_eq!(
            tree.visible(base.clone(), "", Files::All).len(),
            4,
            "refresh retains existing rows while work is real"
        );
        let request = tree.next().unwrap();
        let failed = request.path.clone();
        tree.complete(
            request,
            Err(ScanError::ReadDirectory {
                path: failed.clone(),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            }),
        );
        let rows = tree.visible(base, "", Files::All);
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| row.kind == EntryKind::Directory || !row.path.starts_with(&failed))
        );
        assert_eq!(tree.error(&failed), Some("folder access denied"));
        assert_eq!(tree.disclosure(&failed), Disclosure::Error);
    }

    #[test]
    fn child_cache_budget_and_nonchild_refusal_are_explicit() {
        let mut tree = Tree::default();
        let mut roots = Vec::new();
        for i in 0..5 {
            let root = PathBuf::from(format!("/root{i}"));
            roots.push(root.clone());
            tree.open(root.clone(), true);
            let request = tree.next().unwrap();
            let mut entries: Vec<_> = (0..2048)
                .map(|j| entry(root.join(format!("{j}.wav")), EntryKind::WaveAudio))
                .collect();
            entries[0] = entry("/escape.wav", EntryKind::WaveAudio);
            tree.complete(request, Ok(listing(&root, entries)));
        }
        assert!(
            tree.nodes
                .values()
                .filter_map(|node| node.listing.as_ref())
                .map(|listing| listing.entries.len())
                .sum::<usize>()
                <= MAX_ENTRIES
        );
        let base = tree.roots(roots);
        let rows = tree.visible(base, "", Files::All);
        assert!(rows.iter().all(|row| row.path != Path::new("/escape.wav")));
        assert!(tree.status.is_some());
        let before = tree.nodes.len();
        tree.close(Path::new("/root0"));
        assert!(tree.nodes.len() < before);
    }

    #[test]
    fn source_file_kind_filters_apply_to_inline_children_and_flat_snapshots_are_shared() {
        let root = PathBuf::from("/tree");
        let mut tree = Tree::default();
        let base = tree.roots(vec![root.clone()]);
        assert!(Arc::ptr_eq(
            &base,
            &tree.visible(base.clone(), "", Files::Samples)
        ));
        tree.open(root.clone(), false);
        let request = tree.next().unwrap();
        tree.complete(
            request,
            Ok(listing(
                &root,
                vec![
                    entry(root.join("kick.wav"), EntryKind::WaveAudio),
                    entry(root.join("song.hex"), EntryKind::Project),
                    entry(root.join("nested"), EntryKind::Directory),
                ],
            )),
        );
        let samples = tree.visible(base.clone(), "", Files::Samples);
        assert_eq!(samples.len(), 3);
        assert!(!samples.iter().any(|entry| entry.kind == EntryKind::Project));
        let projects = tree.visible(base.clone(), "", Files::Projects);
        assert_eq!(projects.len(), 3);
        assert!(
            !projects
                .iter()
                .any(|entry| entry.kind == EntryKind::WaveAudio)
        );
        assert_eq!(tree.visible(base, "", Files::All).len(), 4);
    }

    #[test]
    fn exhausted_request_identity_refuses_work_without_a_false_loading_state() {
        let root = PathBuf::from("/tree");
        let mut tree = Tree {
            next_serial: u64::MAX,
            ..Tree::default()
        };
        tree.open(root.clone(), false);
        assert!(tree.next().is_none());
        assert_eq!(tree.requests_started, 0);
        assert_ne!(tree.disclosure(&root), Disclosure::Loading);
        assert!(tree.status.as_ref().unwrap().contains("counter exhausted"));
    }

    #[cfg(unix)]
    #[test]
    fn native_non_utf8_paths_cannot_alias_element_identity() {
        use std::os::unix::ffi::OsStringExt;
        let first = PathBuf::from(std::ffi::OsString::from_vec(b"/root/\xff.wav".to_vec()));
        let second = PathBuf::from(std::ffi::OsString::from_vec(b"/root/\xfe.wav".to_vec()));
        assert_eq!(first.to_string_lossy(), second.to_string_lossy());
        assert_ne!(row_id(&first), row_id(&second));
    }
}
