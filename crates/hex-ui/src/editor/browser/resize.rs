//! Browser-owned sizing policy on the shared Window pointer lease. Outer sizing
//! remains panel-relative/absolute; sidebar sizing keeps its collapse hysteresis.
//! Cancellation retains the last width. No canonical edit or pending ACK is owned.
use super::{EditorView, clamped_panel_width};
use gpui::{
    Context, CursorStyle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Window,
};

#[derive(Clone, Copy)]
pub(super) enum Divider {
    Outer,
    Sidebar,
}
impl Divider {
    fn active(self, view: &EditorView) -> bool {
        match self {
            Self::Outer => view.browser.resizing,
            Self::Sidebar => view.browser.layout.sidebar_drag.is_some(),
        }
    }
    fn clear(self, view: &mut EditorView) {
        match self {
            Self::Outer => view.browser.resizing = false,
            Self::Sidebar => view.browser.layout.sidebar_drag = None,
        }
    }
}
impl EditorView {
    pub(super) fn begin_browser_resize(
        &mut self,
        divider: Divider,
        event: &MouseDownEvent,
        initial: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.browser.open
            || event.button != MouseButton::Left
            || !initial.is_finite()
            || !f32::from(event.position.x).is_finite()
            || !f32::from(event.position.y).is_finite()
        {
            return;
        }
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        let focus = self.browser.results_focus.clone();
        if !window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            CursorStyle::ResizeColumn,
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|e| e.button == MouseButton::Left);
                let Some(position) = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !divider.active(view) {
                        return;
                    }
                    if !view.browser.open
                        || !focus.is_focused(window)
                        || !f32::from(position.x).is_finite()
                        || !f32::from(position.y).is_finite()
                    {
                        divider.clear(view);
                        cx.notify();
                        return;
                    }
                    if moved.is_some() {
                        let available = f32::from(window.bounds().size.width);
                        if !available.is_finite() || available <= 0. {
                            divider.clear(view);
                        } else {
                            let x = f32::from(position.x);
                            match divider {
                                Divider::Outer => {
                                    view.browser.width = clamped_panel_width(
                                        view.browser.layout.base_width_for_total(
                                            x - hex_widgets::PANEL_FRAME_INSET,
                                        ),
                                        available,
                                    )
                                }
                                Divider::Sidebar => {
                                    let width = clamped_panel_width(view.browser.width, available);
                                    view.browser.layout.update_sidebar_resize(x, width);
                                }
                            }
                        }
                    } else {
                        // Existing browser policy uses the last movement value;
                        // release ends input without another width adjustment.
                        divider.clear(view);
                    }
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if divider.active(view) {
                        divider.clear(view);
                        cx.notify();
                    }
                });
            },
            cx,
        ) {
            return;
        }
        self.browser.resizing = false;
        self.browser.layout.sidebar_drag = None;
        match divider {
            Divider::Outer => {
                self.browser.width = initial;
                self.browser.resizing = true;
            }
            Divider::Sidebar => self
                .browser
                .layout
                .begin_sidebar_resize(f32::from(event.position.x), initial),
        }
        self.browser.results_focus.focus(window);
        cx.notify();
    }
    pub(super) fn renew_browser_resize(
        &self,
        divider: Divider,
        window: &mut Window,
        owner: gpui::EntityId,
    ) {
        if divider.active(self)
            && self.browser.open
            && self.browser.results_focus.is_focused(window)
        {
            window.renew_pointer_capture(owner, CursorStyle::ResizeColumn);
        }
    }
}
