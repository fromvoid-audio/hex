//! Browser-only navigation of the current virtualized results. The UniformList owns
//! scroll offsets and virtualization; prepaint reads its latest layout metrics
//! after the list, so wheel, deferred navigation and resizing share one position.
use super::EditorView;
use crate::vertical_scrollbar::{self, Config, Target};
use gpui::{Context, Div, MouseDownEvent, Window, div, prelude::*};

pub(super) use crate::vertical_scrollbar::State;

impl EditorView {
    pub(super) fn browser_results(
        &mut self,
        list: impl IntoElement,
        rows: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        self.browser.scrollbar.sync_rows(rows);
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .child(div().flex_1().min_w_0().h_full().child(list))
            .child(self.browser_scrollbar(cx))
    }

    pub(super) fn browser_scrollbar(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        vertical_scrollbar::element(
            &self.browser.scrollbar,
            self.browser.scroll.clone(),
            Config {
                id: "browser-scrollbar".into(),
                debug_selector: "browser-scrollbar",
                generation: self.browser.scrollbar.generation(),
                rows: self.browser.scrollbar.rows(),
                paint_unscrollable_thumb: false,
            },
            cx,
            |view, event: &MouseDownEvent, window: &mut Window, cx| {
                cx.stop_propagation();
                // Match other non-text browser controls without changing
                // the selected row, query or audition.
                view.browser.results_focus.focus(window);
                window.prevent_default();
                vertical_scrollbar::begin_drag(view, event.position, window, cx, |view| {
                    let generation = view.browser.scrollbar.generation();
                    let rows = view.browser.scrollbar.rows();
                    Some(Target {
                        state: &mut view.browser.scrollbar,
                        scroll: &view.browser.scroll,
                        generation,
                        rows,
                    })
                });
            },
        )
    }
}
