//! View-only navigation of the current filtered page. The UniformList owns
//! scroll offsets and virtualization; prepaint reads its latest layout metrics
//! after the list, so wheel, deferred navigation and resizing share one position.
use super::Viewer;
use crate::vertical_scrollbar::{self, Config, Target};
use gpui::{Context, Div, MouseDownEvent, Window};

pub(super) use crate::vertical_scrollbar::State;

impl Viewer {
    pub(super) fn log_scrollbar(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        vertical_scrollbar::element(
            &self.scrollbar,
            self.scroll.clone(),
            Config {
                id: "log-scrollbar".into(),
                debug_selector: "log-scrollbar",
                generation: self.generation,
                rows: self.rows.len(),
                paint_unscrollable_thumb: true,
            },
            cx,
            |view, event: &MouseDownEvent, window: &mut Window, cx| {
                cx.stop_propagation();
                vertical_scrollbar::begin_drag(view, event.position, window, cx, |view| {
                    Some(Target {
                        state: &mut view.scrollbar,
                        scroll: &view.scroll,
                        generation: view.generation,
                        rows: view.rows.len(),
                    })
                });
            },
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "scrollbar_tests.rs"]
mod tests;
