//! Session-local listening notes use the audible playhead and never issue transport or project edits.
use super::*;
use gpui::{KeyDownEvent, Modifiers};

impl EditorView {
    pub(in crate::editor) fn review_marker_key(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key != "i"
            || event.keystroke.modifiers
                != (Modifiers {
                    platform: true,
                    ..Modifiers::default()
                })
            || self.mode != EditorMode::Arrangement
            || self.state.is_none()
            || self.selection_delete_blocked()
        {
            return false;
        }
        let contexts = window.context_stack();
        if !contexts.iter().any(|c| c.contains("hex_editor"))
            || contexts
                .iter()
                .any(|c| c.contains("hex_text_input") || c.contains("hex_text_editing"))
        {
            return false;
        }
        if !event.is_held
            && self
                .locators
                .review_frames
                .insert(self.audible_cursor_position())
        {
            cx.notify();
        }
        true
    }

    pub(in crate::editor) fn review_marker_layer(
        &self,
        viewport: Viewport,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let mut frames = self
            .locators
            .review_frames
            .range(viewport.start..viewport.end()?)
            .peekable();
        frames.peek()?;
        let color = rgb(theme::color(theme::PaletteRole::WarmAccent));
        let generation = self.project_generation;
        Some(
            div()
                .absolute()
                .inset_0()
                .right(px(self.track_header_width()))
                .overflow_hidden()
                .children(frames.map(|&frame| {
                    let x = position(frame, viewport);
                    let handle = |top: bool| {
                        let edge = if top {
                            "review-marker-top"
                        } else {
                            "review-marker-bottom"
                        };
                        div()
                            .id((edge, frame))
                            .debug_selector(move || format!("{edge}-{frame}"))
                            .absolute()
                            .ml(px(-6.0))
                            .when(top, |el| el.top_0())
                            .when(!top, |el| el.bottom_0())
                            .size(px(13.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .occlude()
                            .cursor_pointer()
                            .child(
                                div()
                                    .size(px(7.0))
                                    .border_1()
                                    .border_color(color)
                                    .bg(rgb(theme::color(theme::PaletteRole::Background))),
                            )
                            .tooltip(|_, cx| {
                                cx.new(|_| crate::editor::transport::MiniTooltip {
                                    kind: crate::tooltips::Kind::Identification,
                                    description: "remove review marker".into(),
                                })
                                .into()
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                                if event.standard_click()
                                    && view.project_generation == generation
                                    && view.locators.review_frames.remove(&frame)
                                {
                                    cx.notify();
                                }
                                cx.stop_propagation();
                            }))
                    };
                    div()
                        .absolute()
                        .left(relative(x))
                        .top_0()
                        .bottom_0()
                        .w(px(1.0))
                        .child(div().absolute().inset_0().bg(color).opacity(0.55))
                        .children([handle(true), handle(false)])
                })),
        )
    }
}
