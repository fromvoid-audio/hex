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
                    let _test_environment = test_environment;
                    std::fs::metadata(&path).and_then(|metadata| {
                        if metadata.is_dir() {
                            Ok(())
                        } else {
                            Err(std::io::Error::other("not a directory"))
                        }
                    })
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
