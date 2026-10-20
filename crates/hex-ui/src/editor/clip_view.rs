//! Shared compact detail pane for Arrangement and Session audio/MIDI selections.
use super::{EditorMode, EditorView};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_session::snapshot_exports::{
    SnapshotTransportError, ui_consumers::SnapshotSessionCellContentRead,
};

pub(super) const CLIP_DETAIL_BODY_HEIGHT: f32 = 228.0;

fn cancel_eq8_audition(view: &mut EditorView) {
    if let Some(audio) = view.audio.as_mut() {
        let _ = audio.control.audition_cancel();
    }
    super::device_panel::clear_eq8_audition_gesture_flags(view);
}

impl EditorView {
    /// Change the detail content in response to explicit clip focus. Pane
    /// visibility is a separate user choice and is intentionally untouched.
    pub(super) fn focus_clip_details(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.device_panel_active && !super::device_panel::finalize_controls(self, cx) {
            return false;
        }
        self.cancel_midi_note_gesture();
        cancel_eq8_audition(self);
        self.device_panel_active = false;
        if self.midi_focus.is_focused(window) || self.audio_event_focus.is_focused(window) {
            self.focus.focus(window);
        }
        true
    }

    /// Track focus selects devices without implicitly opening a closed pane.
    pub(super) fn focus_track_details(&mut self, window: &mut gpui::Window) {
        self.cancel_midi_note_gesture();
        cancel_eq8_audition(self);
        self.focused_return = None;
        self.device_panel_active = true;
        if self.midi_focus.is_focused(window) || self.audio_event_focus.is_focused(window) {
            self.focus.focus(window);
        }
    }

    /// Return focus changes the device owner without changing the regular-track
    /// selection used by clip and time-range editing.
    pub(super) fn focus_return_details(
        &mut self,
        return_track: hex_project::ReturnId,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.device_panel_active && !super::device_panel::finalize_controls(self, cx) {
            return false;
        }
        self.cancel_midi_note_gesture();
        cancel_eq8_audition(self);
        self.focused_return = Some(return_track);
        self.device_panel_active = true;
        if self.midi_focus.is_focused(window) || self.audio_event_focus.is_focused(window) {
            self.focus.focus(window);
        }
        true
    }

    /// An explicit focus with no selected track or clip leaves an open pane
    /// showing its compact empty state.
    pub(super) fn focus_empty_details(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.focus_clip_details(window, cx)
    }

    fn conversion_selection(
        &self,
    ) -> Result<Option<(hex_project::ClipId, bool, u64)>, SnapshotTransportError> {
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };
        state.read_with(|snapshot| {
            let project = snapshot.model();
            let (clip, midi) = if self.mode == EditorMode::Session {
                let (track, scene) = self.selected_cell?;
                match project.session_cell(track, scene) {
                    SnapshotSessionCellContentRead::Midi(clip) => (clip.id(), true),
                    SnapshotSessionCellContentRead::Audio(clip) => (clip.id(), false),
                    SnapshotSessionCellContentRead::Stop
                    | SnapshotSessionCellContentRead::Empty => return None,
                }
            } else {
                let clip = self.selected_clip?;
                if project.midi_clip(clip).is_some() {
                    (clip, true)
                } else if project.tracks().any(|track| track.clip(clip).is_some()) {
                    (clip, false)
                } else {
                    return None;
                }
            };
            Some((clip, midi, snapshot.revision()))
        })
    }

    pub(super) fn clip_detail_view(
        &mut self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.clip_view_open;
        let facts = match self.state.as_ref() {
            Some(state) => state.read_with(|snapshot| {
                let project = snapshot.model();
                let midi = self
                    .selected_clip
                    .is_some_and(|clip| project.midi_source(clip).is_some());
                let audio = self.mode == EditorMode::Session
                    && self.selected_cell.is_some_and(|(track, scene)| {
                        matches!(
                            project.session_cell(track, scene),
                            SnapshotSessionCellContentRead::Audio(_)
                        )
                    });
                (midi, audio)
            }),
            None => Ok((false, false)),
        };
        let Ok((selected_midi, has_session_audio)) = facts else {
            return div()
                .id("clip-detail-read-unavailable")
                .child(theme::text("clip read unavailable"));
        };
        let detail = if open {
            if self.device_panel_active {
                Some(
                    super::device_panel::view(self, Some(window), cx)
                        .map(|element| div().child(element)),
                )
            } else if has_session_audio {
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
        if detail.is_some() && !self.device_panel_active {
            self.clip_detail_body_height = if has_session_audio {
                super::session_audio_panel::DETAIL_HEIGHT
            } else {
                CLIP_DETAIL_BODY_HEIGHT
            };
        }
        self.project_spectrum.detail_height = detail.as_ref().map(|_| {
            if self.device_panel_active {
                super::device_panel::DEVICE_PANEL_HEIGHT
            } else {
                self.clip_detail_body_height
            }
        });
        let label = if self.device_panel_active {
            "devices"
        } else if !open {
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
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
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
                .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
                .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
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
                    if view.clip_view_open && !super::device_panel::finalize_controls(view, cx) {
                        cx.notify();
                        return;
                    }
                    view.clip_view_open = !view.clip_view_open;
                    if !view.clip_view_open {
                        view.audio_event_editor.gesture = None;
                        view.cancel_midi_note_gesture();
                        cancel_eq8_audition(view);
                        if view.midi_focus.is_focused(window)
                            || view.audio_event_focus.is_focused(window)
                        {
                            view.focus.focus(window);
                        }
                    }
                    cx.notify();
                })),
        );
        row = row.child(
            div()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text(label)),
        );
        if !self.device_panel_active
            && let Ok(Some((clip, to_audio, revision))) = self.conversion_selection()
        {
            let enabled = !self.edit_pending();
            let generation = self.project_generation;
            row = row.child(
                div()
                    .id("convert-clip")
                    .debug_selector(|| "convert-clip".into())
                    .h(px(18.0))
                    .px(px(5.0))
                    .flex()
                    .items_center()
                    .text_color(rgb(theme::color(if enabled {
                        theme::PaletteRole::SecondaryText
                    } else {
                        theme::PaletteRole::Muted
                    })))
                    .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
                    .child(theme::text(if to_audio { "→ audio" } else { "→ midi" }))
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::transport::MiniTooltip {
                            description: if to_audio {
                                "bounce clip to audio in place"
                            } else {
                                "convert to empty editable midi clip"
                            }
                            .into(),
                        })
                        .into()
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if !enabled
                            || view.edit_pending()
                            || view.project_generation != generation
                            || !matches!(view.conversion_selection(), Ok(Some(current)) if current == (clip, to_audio, revision))
                        {
                            return;
                        }
                        if !view.focus_clip_details(window, cx) {
                            return;
                        }
                        view.cancel_time_selection_gesture();
                        view.clip_view_open = true;
                        if view.enqueue(hex_session::service::Request::ConvertClip {
                            project_generation: generation,
                            revision,
                            clip,
                            to_audio,
                        }) {
                            view.status = if to_audio {
                                "bouncing clip to audio"
                            } else {
                                "converting clip to midi"
                            }
                            .into();
                        }
                        cx.notify();
                    })),
            );
        }
        row = row.child(
            div()
                .id("clip-detail-tab")
                .debug_selector(|| "clip-detail-tab".into())
                .h(px(18.0))
                .px(px(5.0))
                .flex()
                .items_center()
                .text_color(rgb(if self.device_panel_active {
                    theme::color(theme::PaletteRole::Muted)
                } else {
                    theme::color(theme::PaletteRole::Secondary)
                }))
                .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
                .cursor(gpui::CursorStyle::PointingHand)
                .child(theme::text("clip"))
                .on_click(cx.listener(|view, _, window, cx| {
                    if view.focus_clip_details(window, cx) {
                        view.clip_view_open = true;
                    }
                    cx.notify();
                })),
        );
        row = row.child(
            div()
                .id("device-detail-tab")
                .debug_selector(|| "device-detail-tab".into())
                .h(px(18.0))
                .px(px(5.0))
                .flex()
                .items_center()
                .text_color(rgb(if self.device_panel_active {
                    theme::color(theme::PaletteRole::SecondaryText)
                } else {
                    theme::color(theme::PaletteRole::Muted)
                }))
                .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
                .cursor(gpui::CursorStyle::PointingHand)
                .child(theme::text("devices"))
                .on_click(cx.listener(|view, _, window, cx| {
                    if super::device_panel::finalize_controls(view, cx) {
                        view.focus_track_details(window);
                        view.clip_view_open = true;
                    }
                    cx.notify();
                })),
        );
        let mut panel = div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .child(row)
            .debug_selector(|| "clip-view".into());
        if open {
            if let Some(detail) = detail {
                let preferred = if self.device_panel_active {
                    super::device_panel::DEVICE_PANEL_HEIGHT
                } else {
                    self.clip_detail_body_height
                };
                let height = self.project_spectrum_detail_height(window, preferred);
                if height < preferred {
                    panel = panel.child(
                        div()
                            .id("clip-detail-viewport")
                            .debug_selector(|| "clip-detail-viewport".into())
                            .h(px(height))
                            .flex_shrink_0()
                            .overflow_y_scroll()
                            .child(detail),
                    );
                } else {
                    panel = panel.child(detail);
                }
            } else {
                panel = panel.child(
                    div()
                        .h(px(self.project_spectrum_detail_height(
                            window,
                            self.clip_detail_body_height,
                        )))
                        .flex_shrink_0()
                        .px(px(6.0))
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text("no clip selected"))
                        .debug_selector(|| "clip-detail-empty".into()),
                );
            }
        }
        panel
    }
}
