//! Shared compact detail pane for Arrangement and Session audio/MIDI selections.
use super::{EditorMode, EditorView};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};

pub(super) const CLIP_DETAIL_BODY_HEIGHT: f32 = 228.0;

fn cancel_eq8_audition(view: &mut EditorView) {
    if let Some(audio) = view.audio.as_mut() {
        let _ = audio.control.audition_cancel();
    }
    super::device_panel::clear_eq8_audition_gesture_flags(view);
}

impl EditorView {
    /// Clip editor Arrangement audio subject (`detail_clip_id`) and
    /// start/duration/source offset/length in project frames. Workspace
    /// selection remains a separate query.
    pub(super) fn detail_audio_clip_data(
        &self,
    ) -> Option<(
        hex_project::TrackId,
        hex_project::ClipId,
        u64,
        u64,
        u64,
        u64,
    )> {
        if self.selected_clip.is_some() {
            return self.selected_clip_data();
        }
        let clip_id = self.detail_clip_id()?;
        let state = self.state.as_ref()?;
        state.project.tracks().iter().find_map(|track| {
            let clip = track.clips().iter().find(|clip| clip.id() == clip_id)?;
            Some((
                track.id(),
                clip_id,
                clip.start(),
                clip.duration(),
                clip.source_offset(),
                clip.source_length(),
            ))
        })
    }

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
        self.detail_context = None;
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
        if !self.focus_clip_details(window, cx) {
            return false;
        }
        true
    }

    pub(super) fn clip_detail_view(
        &mut self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Div {
        self.reconcile_clip_detail_resize(window, cx);
        self.reconcile_velocity_size(window, cx);
        self.reconcile_piano_pitch_zoom(window, cx);
        self.reconcile_piano_pan();
        self.reconcile_midi_ruler();
        self.reconcile_audio_loop();
        let open = self.clip_view_open;
        let selected_midi = self.state.as_ref().is_some_and(|state| {
            self.detail_clip_id()
                .is_some_and(|clip| state.project.midi_source(clip).is_some())
        });
        // A conversion can retain ClipId while changing its source kind. Retire
        // that MIDI viewport too, even when no new MIDI editor is rendered.
        if self.piano_view_clip.is_some_and(|(generation, clip)| {
            generation != self.project_generation
                || self.detail_clip_id() != Some(clip)
                || !selected_midi
        }) {
            self.piano_view_clip = None;
            self.piano_zoom_wheel_accum = 0.0;
        }
        let has_session_audio = self.mode == EditorMode::Session
            && self.selected_cell.is_some_and(|(track, scene)| {
                self.state.as_ref().is_some_and(|state| {
                    matches!(
                        state.project.session_cell(track, scene),
                        Some(hex_project::SessionCellContent::Audio(_))
                    )
                })
            });
        let natural = if has_session_audio {
            super::session_audio_panel::DETAIL_HEIGHT
        } else if selected_midi {
            CLIP_DETAIL_BODY_HEIGHT + self.midi_velocity.visible_height()
        } else {
            CLIP_DETAIL_BODY_HEIGHT
        };
        let minimum = if has_session_audio {
            super::session_audio_panel::DETAIL_HEIGHT
        } else {
            super::clip_detail_resize::MIN_HEIGHT
                + if selected_midi {
                    self.midi_velocity.visible_height()
                } else {
                    0.0
                }
        };
        let preferred = self.clip_detail_resize.preferred(natural, minimum);
        self.project_spectrum.detail_height = open.then_some(if self.device_panel_active {
            super::device_panel::DEVICE_PANEL_HEIGHT
        } else {
            preferred
        });
        // Below a useful content size, keep real controls/canvas intact in the
        // existing scroll viewport rather than letting intrinsic children crush tracks.
        self.clip_detail_body_height = self
            .project_spectrum_detail_height(window, preferred)
            .max(minimum);
        let detail = if open {
            if self.device_panel_active {
                Some(
                    super::device_panel::view(self, Some(window), cx)
                        .map(|element| div().child(element)),
                )
            } else if has_session_audio {
                super::session_audio_panel::view(self, cx).map(|element| div().child(element))
            } else if selected_midi {
                self.midi_editor(window, cx)
                    .map(|element| div().child(element))
            } else if self.mode == EditorMode::Arrangement {
                super::arrangement_audio_panel::view(self, window, cx)
                    .map(|element| div().child(element))
            } else {
                None
            }
        } else {
            None
        };
        if detail.is_none() {
            self.project_spectrum.detail_height = None;
            self.clip_detail_body_height = self.project_spectrum_detail_height(window, preferred);
        }
        let label = if self.device_panel_active {
            "devices"
        } else if !open {
            "clip view"
        } else if has_session_audio {
            "session audio"
        } else if selected_midi {
            "midi clip"
        } else if self.mode == EditorMode::Arrangement && self.detail_clip_id().is_some() {
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
                        kind: crate::tooltips::Kind::Identification,
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
            .children(
                (open && !self.device_panel_active).then(|| self.clip_detail_resize_handle(cx)),
            )
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

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_view_resize_tests.rs"]
mod resize_tests;
