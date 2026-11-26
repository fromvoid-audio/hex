//! Browser-owned sizing policy on the shared Window pointer lease. Outer sizing
//! remains panel-relative/absolute; sidebar sizing keeps its collapse hysteresis.
//! Cancellation retains the last width. No canonical edit or pending ACK is owned.
use super::EditorView;
use gpui::{
    Context, CursorStyle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Window,
};

#[derive(Clone, Copy)]
pub(super) enum Divider {
    Outer,
    Sidebar,
    MiniTags,
}
impl Divider {
    fn active(self, view: &EditorView) -> bool {
        match self {
            Self::Outer => view.browser.resizing,
            Self::Sidebar => view.browser.layout.sidebar_drag.is_some(),
            Self::MiniTags => view.browser.layout.mini_tags_drag.is_some(),
        }
    }
    fn clear(self, view: &mut EditorView) {
        match self {
            Self::Outer => view.browser.resizing = false,
            Self::Sidebar => view.browser.layout.sidebar_drag = None,
            Self::MiniTags => view.browser.layout.mini_tags_drag = None,
        }
    }
    fn cursor(self) -> CursorStyle {
        match self {
            Self::Outer | Self::Sidebar => CursorStyle::ResizeColumn,
            Self::MiniTags => CursorStyle::ResizeRow,
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
            divider.cursor(),
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
                                    view.browser.width = view.clamped_browser_width(
                                        view.browser.layout.base_width_for_total(
                                            x - hex_widgets::PANEL_FRAME_INSET,
                                        ),
                                        available,
                                    )
                                }
                                Divider::Sidebar => {
                                    let width =
                                        view.clamped_browser_width(view.browser.width, available);
                                    view.browser.layout.update_sidebar_resize(x, width);
                                }
                                Divider::MiniTags => {
                                    let y = f32::from(position.y);
                                    let window_height = f32::from(window.bounds().size.height);
                                    view.browser
                                        .layout
                                        .update_mini_tags_resize(y, window_height);
                                }
                            }
                        }
                    } else {
                        // Existing browser policy uses the last movement value;
                        // release ends input without another width adjustment.
                        divider.clear(view);
                        if matches!(divider, Divider::Sidebar) {
                            view.browser.layout.sidebar_width_customized = true;
                            view.sync_sidebar_preferences_and_save(cx);
                        }
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
        self.browser.layout.mini_tags_drag = None;
        match divider {
            Divider::Outer => {
                self.browser.width = initial;
                self.browser.resizing = true;
            }
            Divider::Sidebar => self
                .browser
                .layout
                .begin_sidebar_resize(f32::from(event.position.x), initial),
            Divider::MiniTags => self
                .browser
                .layout
                .begin_mini_tags_resize(f32::from(event.position.y), initial),
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
            window.renew_pointer_capture(owner, divider.cursor());
        }
    }
}
