//! Shared compact detail pane for Arrangement and Session audio/MIDI selections.
use super::{EditorMode, EditorView};
use crate::theme::{self, Palette};
use gpui::{Context, Div, div, prelude::*, px, rgb};

impl EditorView {
    pub(super) fn clip_detail_view(&mut self, cx: &mut Context<Self>) -> Div {
        let open = self.clip_view_open;
        let selected_midi = self.state.as_ref().is_some_and(|state| {
            self.selected_clip
                .is_some_and(|clip| state.project.midi_source(clip).is_some())
        });
        let has_session_audio = self.mode == EditorMode::Session
            && self.selected_cell.is_some_and(|(track, scene)| {
                self.state.as_ref().is_some_and(|state| {
                    matches!(
                        state.project.session_cell(track, scene),
                        Some(hex_project::SessionCellContent::Audio(_))
                    )
                })
            });
        let detail = if open {
            if has_session_audio {
                super::session_audio_panel::view(self, cx).map(|element| div().child(element))
            } else if selected_midi {
                self.midi_editor(cx).map(|element| div().child(element))
            } else if self.mode == EditorMode::Arrangement {
                super::arrangement_audio_panel::view(self, cx).map(|element| div().child(element))
            } else {
                None
            }
        } else {
            None
        };
        let label = if !open {
            "clip view"
        } else if has_session_audio {
            "session audio"
        } else if selected_midi {
            "midi clip"
        } else if self.mode == EditorMode::Arrangement && self.selected_clip.is_some() {
            "audio clip"
        } else {
            "clip view · no clip selected"
        };
        let mut row = div()
            .id("clip-view-toggle-row")
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(5.0))
            .px(px(5.0))
            .bg(rgb(Palette::SURFACE))
            .border_t_1()
            .border_color(rgb(Palette::BORDER))
            .text_xs()
            .debug_selector(|| "clip-view-toggle-row".into());
        row = row.child(
            div()
                .id("clip-view-toggle")
                .size(px(18.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(Palette::CYAN))
                .hover(|style| style.bg(rgb(Palette::RAISED)))
                .debug_selector(|| "clip-view-toggle".into())
                .tooltip(move |_, cx| {
                    cx.new(|_| super::transport::MiniTooltip {
                        description: if open {
                            "hide clip view"
                        } else {
                            "show selected clip details"
                        }
                        .into(),
                    })
                    .into()
                })
                .child(theme::text(if open { "⌄" } else { "⌃" }))
                .on_click(cx.listener(|view, _, window, cx| {
                    view.clip_view_open = !view.clip_view_open;
                    if !view.clip_view_open {
                        view.cancel_midi_note_gesture();
                        if view.midi_focus.is_focused(window) {
                            view.focus.focus(window);
                        }
                    }
                    cx.notify();
                })),
        );
        row = row.child(
            div()
                .text_color(rgb(Palette::MUTED))
                .child(theme::text(label)),
        );
        let mut panel = div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(Palette::SURFACE))
            .child(row)
            .debug_selector(|| "clip-view".into());
        if open {
            if let Some(detail) = detail {
                panel = panel.child(detail);
            } else {
                panel = panel.child(
                    div()
                        .h(px(18.0))
                        .flex_shrink_0()
                        .px(px(6.0))
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(rgb(Palette::MUTED))
                        .child(theme::text("no clip selected")),
                );
            }
        }
        panel
    }
}
