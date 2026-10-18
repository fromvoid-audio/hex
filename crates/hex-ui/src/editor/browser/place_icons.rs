//! Bundled picker glyphs and path-keyed assignment; persistence stays off the UI thread.
use super::super::context_menu::EditorContextAction;
use super::*;
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Pixels, Point};

// Existing browser glyphs, rendered in bundled Ioskeley Mono. Font provenance
// and license: assets/fonts/ioskeley-mono/source.json and OFL.txt.
const ICONS: [(&str, &str, &str); 7] = [
    ("folder", "□", "folder · □"),
    ("samples", "♫", "samples · ♫"),
    ("devices", "⌘", "devices · ⌘"),
    ("projects", "▤", "projects · ▤"),
    ("collection", "◈", "collection · ◈"),
    ("waveform", "≋", "waveform · ≋"),
    ("search", "⌕", "search · ⌕"),
];

pub(super) fn known(id: &str) -> bool {
    ICONS.iter().any(|(candidate, _, _)| *candidate == id)
}

impl EditorView {
    pub(super) fn browser_place_glyph(&self, path: &std::path::Path) -> &'static str {
        if path == self.browser.home || path == self.browser.music {
            return ICONS[0].1;
        }
        self.browser
            .place_icons
            .get(path)
            .and_then(|id| ICONS.iter().find(|(candidate, _, _)| candidate == id))
            .map_or(ICONS[0].1, |(_, glyph, _)| *glyph)
    }

    pub(in crate::editor) fn browser_place_icon_assignable(&self, path: &std::path::Path) -> bool {
        path != self.browser.home
            && path != self.browser.music
            && self.browser.places.iter().any(|place| place == path)
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
            .browser
            .place_icons
            .get(&path)
            .and_then(|id| ICONS.iter().position(|(candidate, _, _)| candidate == id))
            .unwrap_or(0);
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            position,
            selected,
            focus,
            previous_focus,
            items: ICONS
                .iter()
                .enumerate()
                .map(|(index, (id, _, label))| MenuItem {
                    label,
                    swatch: None,
                    checked: selected == index,
                    action: EditorContextAction::PlaceIcon {
                        path: path.clone(),
                        id: (index != 0).then_some(*id),
                    },
                })
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
        if let Some(id) = id {
            self.browser.place_icons.insert(path, id.into());
        } else {
            self.browser.place_icons.remove(&path);
        }
        self.queue_browser_places_save(cx);
        cx.notify();
    }
}
