//! Bundled picker glyphs and path-keyed assignment; persistence stays off the UI thread.
use super::*;
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{AnyElement, Pixels, Point, anchored, deferred, point};

// The same glyphs already used by the browser, rendered in bundled Ioskeley Mono.
// Font provenance/license: assets/fonts/ioskeley-mono/source.json and OFL.txt.
const ICONS: [(&str, &str, &str); 7] = [
    ("folder", "□", "folder · □"),
    ("samples", "♫", "samples · ♫"),
    ("devices", "⌘", "devices · ⌘"),
    ("projects", "▤", "projects · ▤"),
    ("collection", "◈", "collection · ◈"),
    ("waveform", "≋", "waveform · ≋"),
    ("search", "⌕", "search · ⌕"),
];

pub(super) struct Picker {
    path: PathBuf,
    menu: PopupMenu<usize>,
}

pub(super) fn known(id: &str) -> bool {
    ICONS.iter().any(|(candidate, _, _)| *candidate == id)
}

impl EditorView {
    pub(super) fn browser_place_glyph(&self, path: &std::path::Path) -> &'static str {
        self.browser
            .place_icons
            .get(path)
            .and_then(|id| ICONS.iter().find(|(candidate, _, _)| candidate == id))
            .map_or(ICONS[0].1, |(_, glyph, _)| *glyph)
    }

    pub(super) fn open_browser_place_icons(
        &mut self,
        path: PathBuf,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if path == self.browser.home
            || path == self.browser.music
            || !self.browser.places.contains(&path)
        {
            return;
        }
        if self.browser.preferences_loading {
            self.browser.places_status =
                Some("place icons unavailable · loading browser places…".into());
            cx.notify();
            return;
        }
        if !self.browser.preferences_writable {
            // Keep the original load error visible; don't overwrite a malformed file.
            return;
        }
        let selected = self
            .browser
            .place_icons
            .get(&path)
            .and_then(|id| ICONS.iter().position(|(candidate, _, _)| candidate == id))
            .unwrap_or(0);
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.browser.place_icon_picker = Some(Picker {
            path,
            menu: PopupMenu {
                position,
                selected,
                focus,
                previous_focus,
                items: ICONS
                    .iter()
                    .enumerate()
                    .map(|(index, (_, _, label))| MenuItem {
                        label,
                        swatch: None,
                        checked: selected == index,
                        action: index,
                    })
                    .collect(),
            },
        });
        cx.notify();
    }

    pub(super) fn dismiss_browser_place_icons(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(picker) = self.browser.place_icon_picker.take() {
            if picker.menu.focus.is_focused(window) {
                if let Some(focus) = picker.menu.previous_focus {
                    focus.focus(window);
                } else {
                    self.focus.focus(window);
                }
            }
            cx.notify();
        }
    }

    fn choose_browser_place_icon(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .browser
            .place_icon_picker
            .as_ref()
            .map(|picker| picker.path.clone())
        else {
            return;
        };
        self.dismiss_browser_place_icons(window, cx);
        let Some((id, _, _)) = ICONS.get(index) else {
            return;
        };
        if self.browser.preferences_loading
            || !self.browser.preferences_writable
            || !self.browser.places.contains(&path)
        {
            return;
        }
        if index == 0 {
            self.browser.place_icons.remove(&path);
        } else {
            self.browser.place_icons.insert(path, (*id).into());
        }
        self.queue_browser_places_save(cx);
        cx.notify();
    }

    fn navigate_browser_place_icons(&mut self, next: bool, cx: &mut Context<Self>) {
        if let Some(picker) = self.browser.place_icon_picker.as_mut() {
            let count = picker.menu.items.len();
            picker.menu.selected = if next {
                (picker.menu.selected + 1) % count
            } else {
                (picker.menu.selected + count - 1) % count
            };
            cx.notify();
        }
    }

    pub(super) fn browser_place_icon_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.browser.place_icon_picker.as_ref()?;
        let panel = crate::context_menu::popup(
            &picker.menu,
            window,
            cx,
            Self::choose_browser_place_icon,
            Self::dismiss_browser_place_icons,
            Self::navigate_browser_place_icons,
        )
        .key_context("hex_context_menu")
        .debug_selector(|| "browser-place-icon-picker".into());
        // Defer above the browser clipping/arrangement. The popup's coordinates and
        // outside-click hit area are window-local, independent of browser origin.
        let control = div()
            .w(window.bounds().size.width)
            .h(window.bounds().size.height)
            .child(panel);
        Some(
            deferred(anchored().position(point(px(0.0), px(0.0))).child(control))
                .with_priority(100)
                .into_any_element(),
        )
    }
}
