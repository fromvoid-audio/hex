//! Embedded picker icons and path-keyed assignment; persistence stays off the UI thread.
use super::super::context_menu::EditorContextAction;
use super::*;
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Pixels, Point};

// Stable preference IDs and labels remain independent of the editable artwork.
const ICONS: [(&str, &str, &str); 7] = [
    ("folder", "icons/browser-place/folder.svg", "folder · □"),
    ("samples", "icons/browser-place/samples.svg", "samples · ♫"),
    ("devices", "icons/browser-place/devices.svg", "devices · ⌘"),
    (
        "projects",
        "icons/browser-place/projects.svg",
        "projects · ▤",
    ),
    (
        "collection",
        "icons/browser-place/collection.svg",
        "collection · ◈",
    ),
    (
        "waveform",
        "icons/browser-place/waveform.svg",
        "waveform · ≋",
    ),
    ("search", "icons/browser-place/search.svg", "search · ⌕"),
];

pub(super) fn known(id: &str) -> bool {
    ICONS.iter().any(|(candidate, _, _)| *candidate == id)
}

impl EditorView {
    pub(super) fn browser_place_icon_path(&self, path: &std::path::Path) -> &'static str {
        self.browser_place_icon_id(path)
            .and_then(|id| ICONS.iter().find(|(candidate, _, _)| *candidate == id))
            .map_or(ICONS[0].1, |(_, path, _)| *path)
    }

    pub(in crate::editor) fn browser_place_icon_assignable(&self, path: &std::path::Path) -> bool {
        self.browser.places.iter().any(|place| place == path)
            && self.browser_place_visible(path)
            && !self.browser.preferences_loading
            && self.browser.preferences_writable
    }

    pub(super) fn open_browser_place_icons(
        &mut self,
        path: PathBuf,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.browser_place_icon_assignable(&path) {
            if self.browser.preferences_loading {
                self.browser.places_status =
                    Some("place icons unavailable · loading browser places…".into());
                cx.notify();
            }
            // Preserve the original preference load error; never overwrite that file.
            return;
        }
        self.cancel_time_selection_gesture();
        let selected = self
            .browser_place_icon_id(&path)
            .and_then(|id| ICONS.iter().position(|(candidate, _, _)| *candidate == id))
            .map_or(0, |index| index + 1);
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: Some(ICONS.iter().map(|(_, path, _)| *path).collect()),
            selected,
            focus,
            previous_focus,
            items: std::iter::once(MenuItem {
                label: "default icon",
                swatch: None,
                checked: selected == 0,
                action: EditorContextAction::PlaceIcon {
                    path: path.clone(),
                    id: None,
                },
            })
            .chain(
                ICONS
                    .iter()
                    .enumerate()
                    .map(|(index, (id, _, label))| MenuItem {
                        label,
                        swatch: None,
                        checked: selected == index + 1,
                        action: EditorContextAction::PlaceIcon {
                            path: path.clone(),
                            id: Some(*id),
                        },
                    }),
            )
            .chain(std::iter::once(MenuItem {
                label: "remove",
                swatch: None,
                checked: false,
                action: EditorContextAction::RemoveBrowserSource(BrowserSection::Place(
                    path.clone(),
                )),
            }))
            .chain(self.browser_place_relocatable(&path).then(|| MenuItem {
                label: "relocate…",
                swatch: None,
                checked: false,
                action: EditorContextAction::RelocateBrowserPlace(path.clone()),
            }))
            .collect(),
        });
        cx.notify();
    }

    pub(in crate::editor) fn set_browser_place_icon(
        &mut self,
        path: PathBuf,
        id: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if !self.browser_place_icon_assignable(&path) || id.is_some_and(|id| !known(id)) {
            return;
        }
        if let Some(source) = self.browser_builtin_place(&path) {
            // Validation is shared with custom icons; unknown saved IDs remain untouched until chosen/reset.
            if let Err(error) = self.browser.source_preferences.set_builtin_icon(source, id) {
                self.browser.places_status =
                    Some(places_error("could not set source icon", &error));
                cx.notify();
                return;
            }
        } else if let Some(id) = id {
            self.browser.place_icons.insert(path, id.into());
        } else {
            self.browser.place_icons.remove(&path);
        }
        self.queue_browser_places_save(cx);
        cx.notify();
    }
}

impl EditorView {
    fn browser_builtin_place(&self, path: &std::path::Path) -> Option<BrowserBuiltin> {
        if path == self.browser.music {
            Some(BrowserBuiltin::Music)
        } else if path == self.browser.home {
            Some(BrowserBuiltin::Home)
        } else {
            None
        }
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

    pub(in crate::editor) fn remove_browser_source(
        &mut self,
        source: BrowserSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser_source_removable(&source) {
            self.set_browser_source_visible(source, false, cx);
            self.browser.results_focus.focus(window);
            window.prevent_default();
        }
    }

    pub(in crate::editor) fn browser_source_removable(&self, source: &BrowserSection) -> bool {
        !self.browser.preferences_loading
            && self.browser.preferences_writable
            && match source {
                BrowserSection::Place(path) => {
                    self.browser.places.contains(path) && self.browser_place_visible(path)
                }
                BrowserSection::Samples => self.browser.layout.samples_visible(),
                _ => false,
            }
    }

    pub(super) fn set_browser_source_visible(
        &mut self,
        source: BrowserSection,
        visible: bool,
        cx: &mut Context<Self>,
    ) {
        if self.browser.preferences_loading || !self.browser.preferences_writable {
            return;
        }
        match source {
            BrowserSection::Place(ref path) if self.browser.places.contains(path) => {
                if let Some(builtin) = self.browser_builtin_place(path) {
                    self.browser
                        .source_preferences
                        .set_builtin_hidden(builtin, !visible);
                } else {
                    if let Err(error) = self
                        .browser
                        .source_preferences
                        .set_place_hidden(path, !visible)
                    {
                        self.browser.places_status =
                            Some(places_error("could not hide source", &error));
                        cx.notify();
                        return;
                    }
                }
            }
            BrowserSection::Samples => {
                self.browser.layout.set_samples_visible(visible);
                self.browser
                    .source_preferences
                    .set_builtin_hidden(BrowserBuiltin::Samples, !visible);
            }
            _ => return,
        }
        self.reconcile_browser_source_visibility(cx);
        self.queue_browser_places_save(cx);
        cx.notify();
    }

    pub(super) fn reconcile_browser_source_visibility(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn open_browser_samples_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.browser_source_removable(&BrowserSection::Samples) {
            return;
        }
        self.cancel_time_selection_gesture();
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: None,
            selected: 0,
            focus,
            previous_focus,
            items: vec![MenuItem {
                label: "remove",
                swatch: None,
                checked: false,
                action: EditorContextAction::RemoveBrowserSource(BrowserSection::Samples),
            }],
        });
        cx.notify();
    }
}
