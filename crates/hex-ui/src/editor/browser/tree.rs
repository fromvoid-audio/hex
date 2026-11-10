//! Bounded inline folder snapshots. Only the worker calls the filesystem.
use super::*;

mod state;
use state::MAX_DEPTH;
pub(super) use state::{Disclosure, Files, Tree, row_id};

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
