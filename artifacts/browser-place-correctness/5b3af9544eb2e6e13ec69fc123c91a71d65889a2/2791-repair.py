from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser.rs')
s=p.read_text()
def replace(old,new):
 global s
 assert s.count(old)==1,(old[:80],s.count(old))
 s=s.replace(old,new)
replace('    loading: bool,\n    scans: ScanCoordinator,','    loading: bool,\n    pending_selection: Option<BrowserItemKey>,\n    scans: ScanCoordinator,')
replace('            loading: false,\n            scans: ScanCoordinator::default(),','            loading: false,\n            pending_selection: None,\n            scans: ScanCoordinator::default(),')
replace('        let start = self.browser.scans.request(directory.clone());','''        if matches!(self.browser.section, BrowserSection::Place(_)) {
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
        let start = self.browser.scans.request(directory.clone());''')
replace('    fn reset_browser_results(&mut self) {','    fn reset_browser_results(&mut self) {\n        self.browser.pending_selection = None;')
replace('    fn play_browser_sample_from(&mut self, path: PathBuf, fraction: f64, cx: &mut Context<Self>) {','''    fn play_browser_sample_from(&mut self, path: PathBuf, fraction: f64, cx: &mut Context<Self>) {
        // Previously painted controls can retain a path across source validation.
        if self.browser_place_unavailable() {
            return;
        }''')
replace('            let selection = self.browser_selected_item(cx);\n            self.browser.samples.invalidate_listing();','''            let selection = if self.browser.loading
                && matches!(self.browser.section, BrowserSection::Place(_))
            {
                self.browser.pending_selection.take()
            } else {
                self.browser_selected_item(cx)
            };
            self.browser.samples.invalidate_listing();''')
replace('    fn browser_visible_entries(&self, cx: &App) -> Arc<Vec<LibraryEntry>> {','''    fn browser_place_unavailable(&self) -> bool {
        matches!(self.browser.section, BrowserSection::Place(_))
            && (self.browser.loading || self.browser.error.is_some())
    }

    fn browser_visible_entries(&self, cx: &App) -> Arc<Vec<LibraryEntry>> {
        if self.browser_place_unavailable() {
            return Arc::new(Vec::new());
        }''')
replace('            BrowserSection::Place(_) if self.browser.error.is_some() => Arc::new(Vec::new()),\n','')
replace('''                && self
                    .browser
                    .listing
                    .as_ref()
                    .is_none_or(|listing| listing.directory != path) =>''','''                && (matches!(self.browser.section, BrowserSection::Place(_))
                    || self
                        .browser
                        .listing
                        .as_ref()
                        .is_none_or(|listing| listing.directory != path)) =>''')
replace('    fn import_browser_audio(&mut self, path: PathBuf) {','''    fn import_browser_audio(&mut self, path: PathBuf) {
        if self.browser_place_unavailable() {
            return;
        }''')
replace('''                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.browser.selected_index = index;''','''                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    if view.browser_place_unavailable() {
                        cx.stop_propagation();
                        return;
                    }
                    view.browser.selected_index = index;''')
replace('    fn open_browser_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {','''    fn open_browser_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.browser_place_unavailable() {
            return;
        }''')
p.write_text(s)
p=Path('crates/hex-library/src/catalog.rs');s=p.read_text()
s=s.replace('            let query = query.to_lowercase();\n            let extra = {','            let query = query.to_lowercase();\n            let terms: Vec<_> = query.split_whitespace().collect();\n            let extra = {')
s=s.replace('                    .filter(|entry| entry.name.to_lowercase().contains(&query))','''                    .filter(|entry| {
                        if source.is_some() {
                            matches_path_terms(&entry.path, &terms)
                        } else {
                            entry.name.to_lowercase().contains(&query)
                        }
                    })''')
old='''                if terms.is_empty() {
                    return true;
                }
                let path = entry.path.to_string_lossy().to_lowercase();
                terms.iter().all(|term| path.contains(term))'''
assert old in s
s=s.replace(old,'                matches_path_terms(&entry.path, &terms)')
anchor='/// Scan Music and explicitly added places'
s=s.replace(anchor,'''fn matches_path_terms(path: &Path, terms: &[&str]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let path = path.to_string_lossy().to_lowercase();
    terms.iter().all(|term| path.contains(term))
}

'''+anchor)
p.write_text(s)
