//! Internal folder bookmarks. Filesystem validation stays on the existing worker executor.
use super::*;

#[derive(Clone)]
pub(super) struct FolderDrag {
    pub(super) path: PathBuf,
    pub(super) label: String,
    scan_generation: u64,
    section: BrowserSection,
    directory: PathBuf,
}

pub(super) struct FolderGhost(pub(super) String);

impl gpui::Render for FolderGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "browser-folder-bookmark-ghost".into())
            .px_1()
            .h(px(ROW_HEIGHT))
            .max_w(px(220.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(theme::text(format!("folder · {}", self.0)))
    }
}

impl EditorView {
    pub(super) fn browser_folder_drag(&self, path: PathBuf, label: String) -> FolderDrag {
        FolderDrag {
            path,
            label,
            scan_generation: self.browser.scans.generation,
            section: self.browser.section.clone(),
            directory: self.browser.directory.clone(),
        }
    }

    pub(super) fn folder_drag_current(&self, drag: &FolderDrag, cx: &App) -> bool {
        drag.scan_generation == self.browser.scans.generation
            && drag.section == self.browser.section
            && drag.directory == self.browser.directory
            && self
                .browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.path == drag.path && entry.kind == EntryKind::Directory)
            && !self.browser_place_unavailable()
    }

    pub(super) fn bookmark_browser_folder(&mut self, drag: &FolderDrag, cx: &mut Context<Self>) {
        if !self.folder_drag_current(drag, cx) || self.browser.folder_bookmark_pending {
            return;
        }
        if !self.browser.preferences_writable {
            // Preserve the existing load/write diagnostic and malformed bytes.
            return;
        }
        self.browser.folder_bookmark_pending = true;
        let drag = drag.clone();
        let path = drag.path.clone();
        #[cfg(test)]
        let test_environment = Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let gate = test_environment.before_folder_validation().await;
                    let result = std::fs::metadata(&path).and_then(|metadata| {
                        if metadata.is_dir() {
                            Ok(())
                        } else {
                            Err(std::io::Error::other("not a directory"))
                        }
                    });
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.did_validate(result.is_ok(), test_environment.root().is_dir());
                    }
                    result
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser.folder_bookmark_pending = false;
                if !view.folder_drag_current(&drag, cx) {
                    cx.notify();
                    return;
                }
                match result {
                    Err(error) => {
                        view.browser.places_status = Some(format!(
                            "could not bookmark folder · {}",
                            error.to_string().chars().take(160).collect::<String>()
                        ));
                    }
                    Ok(()) if view.browser.places.contains(&drag.path) => {}
                    Ok(()) if view.browser.places.len() >= PLACE_CAPACITY => {
                        view.browser.places_status = Some("browser folder limit reached".into());
                    }
                    Ok(()) if view.browser.preferences_writable => {
                        view.browser.places.push(drag.path);
                        view.queue_browser_places_save(cx);
                        view.refresh_sample_catalog(cx);
                    }
                    Ok(()) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl EditorView {
    /// Custom places can be pointed at a moved folder; Home and Music are fixed.
    pub(in crate::editor) fn browser_place_relocatable(&self, path: &std::path::Path) -> bool {
        !self.browser.preferences_loading
            && self.browser.preferences_writable
            && self.browser.places.iter().any(|place| place == path)
            && path != self.browser.home
            && path != self.browser.music
    }

    /// Reported by the last completed sample scan; no per-frame filesystem checks.
    pub(super) fn browser_place_missing(&self, path: &std::path::Path) -> bool {
        self.browser.samples.missing_roots.contains(path)
    }

    pub(in crate::editor) fn prompt_relocate_browser_place(
        &mut self,
        old: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if !self.browser_place_relocatable(&old) {
            return;
        }
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("relocate place".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                match picked {
                    Ok(Ok(Some(mut paths))) => {
                        if let Some(new) = paths.pop() {
                            view.relocate_browser_place(old, new, cx);
                        }
                    }
                    Ok(Err(error)) => {
                        view.error = Some(
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string(),
                            )
                            .into(),
                        )
                    }
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Point a place at its moved folder in place: list position, icon and hidden
    /// state follow it, cataloged records are re-keyed by relative path, and a
    /// rescan confirms the new folder. Sample files are never touched.
    pub(in crate::editor) fn relocate_browser_place(
        &mut self,
        old: PathBuf,
        new: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if !self.browser_place_relocatable(&old) || old == new {
            return;
        }
        if self.browser.places.contains(&new)
            || new == self.browser.home
            || new == self.browser.music
        {
            self.browser.places_status = Some("folder is already a place".into());
            cx.notify();
            return;
        }
        let Some(index) = self.browser.places.iter().position(|place| *place == old) else {
            return;
        };
        if let Err(error) = self.browser.source_preferences.relocate(&old, new.clone()) {
            self.browser.places_status = Some(places_error("could not relocate place", &error));
            cx.notify();
            return;
        }
        self.browser.places[index] = new.clone();
        if let Some(icon) = self.browser.place_icons.remove(&old) {
            self.browser.place_icons.insert(new.clone(), icon);
        }
        let mut catalog = self.browser.samples.catalog.clone();
        catalog.relocate_root(&old, &new);
        self.browser.samples.replace(catalog);
        self.browser.samples.missing_roots.remove(&old);
        if self.browser.section == BrowserSection::Place(old) {
            self.select_browser_source(BrowserSection::Place(new), cx);
        }
        self.queue_browser_places_save(cx);
        self.refresh_sample_catalog(cx);
        cx.notify();
    }
}
