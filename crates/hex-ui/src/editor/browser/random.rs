//! One random rank in the current filtered file projection. This owner never
//! builds another pool, inserts project content or prepares audio. Existing row
//! offsets/history, UniformList and selection audition retain those contracts.
use super::random_rank::audio_index;
use super::*;
use gpui::{Bounds, Pixels, fill, point, size};

impl EditorView {
    pub(in crate::editor) fn browser_random_sample(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.browser.open || self.browser_place_unavailable() {
            return;
        }
        let entries = self.browser_visible_entries(cx);
        let count = entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::WaveAudio)
            .count();
        if count == 0 {
            return;
        }
        let Some(index) = audio_index(&entries, fastrand::usize(..count)) else {
            return;
        };
        let Some(row) = self.browser_rows(cx).files.start.checked_add(index) else {
            return;
        };
        let Some(entry) = entries.get(index) else {
            return;
        };
        let path = entry.path.clone();
        self.checkpoint_browser_history(cx);
        self.browser.selected_index = row;
        self.browser.results_focused = true;
        self.browser
            .scroll
            .scroll_to_item(row, gpui::ScrollStrategy::Top);
        self.browser.results_focus.focus(window);
        self.browser_selection_audition(Some(path), cx);
        cx.notify();
    }

    pub(super) fn browser_random_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let enabled = !self.browser_place_unavailable()
            && self
                .browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.kind == EntryKind::WaveAudio);
        let description = theme::text("random sample · r");
        // Draw the die, so its pips do not depend on font glyph coverage.
        let icon = canvas(
            |bounds, _, _| bounds,
            |_, bounds: Bounds<Pixels>, window: &mut Window, _| {
                let background = theme::color(theme::PaletteRole::Surface);
                let ink = theme::readable_text(background, theme::color(theme::PaletteRole::Muted));
                let face = Bounds::new(
                    bounds.origin + point(px(1.0), px(1.0)),
                    size(px(12.0), px(12.0)),
                );
                window.paint_quad(fill(face, rgb(ink)).corner_radii(px(2.0)));
                for (x, y) in [(3.0, 3.0), (9.0, 3.0), (6.0, 6.0), (3.0, 9.0), (9.0, 9.0)] {
                    let pip =
                        Bounds::new(bounds.origin + point(px(x), px(y)), size(px(2.0), px(2.0)));
                    window.paint_quad(fill(pip, rgb(background)).corner_radii(px(1.0)));
                }
            },
        )
        .size(px(14.0));
        hex_widgets::action_surface(theme::action_style())
            .rounded_full()
            .child(icon)
            .id("browser-random-sample")
            .debug_selector(|| "browser-random-sample".into())
            .w(px(22.0))
            .h(px(22.0))
            .when(!enabled, |style| style.opacity(0.4))
            .tooltip(move |_, cx| {
                cx.new(|_| super::super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: description.clone(),
                })
                .into()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    cx.stop_propagation();
                    window.prevent_default();
                    view.browser_random_sample(window, cx);
                }),
            )
    }
}
