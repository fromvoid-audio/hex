use super::{
    AddMidiClip, AddMidiTrack, AddTrack, AdditiveSolo, CancelDragOrStop, CopyTimeSelection,
    CutTimeSelection, DeleteClip, DuplicateClip, EditorView, ExportProject, FitTimeline,
    ImportAudio, InsertTimeSelection, MidiOctaveDown, MidiOctaveUp, MoveLeft, MoveRight,
    NewProject, NewStreamingProject, NextTrack, OpenProject, OpenRecentProject,
    OpenRelinkedProject, OpenStreamingProject, PanTimelineLeft, PanTimelineRight,
    PasteTimeSelection, PreviousTrack, Quit, RecoverPreviousSave, RecoverSavedProject,
    RecoverUntitledProject, Redo, RemoveTrack, SaveAsProject, SaveProject, SelectAllMidiNotes,
    SetLoopFromClip, SplitClip, Stop, ToggleLoop, ToggleMidiDrawMode, TogglePlay, ToggleSolo,
    TrimLeft, TrimRight, Undo, ZoomIn, ZoomOut,
};
use crate::theme;
use gpui::{Context, MouseButton, MouseUpEvent, Window, div, prelude::*, px, rgb};
use hex_project::Command;
use hex_session::service::Request as SessionRequest;
use std::sync::atomic::Ordering::Relaxed;

impl EditorView {
    pub(super) fn step_frames(&self) -> u64 {
        self.state
            .as_ref()
            .map_or(12_000, |state| u64::from(state.project.sample_rate()) / 4)
    }

    pub(super) fn nudge_grid_context(
        &self,
        platform: bool,
    ) -> Option<(
        super::musical_time::SnapGrid,
        super::viewport::Viewport,
        f32,
    )> {
        let grid = self.snap_grid.with_platform_modifier(platform);
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        if grid == super::musical_time::SnapGrid::Off {
            return Some((grid, viewport, 1.0));
        }
        let bounds = self
            .arrangement_bounds
            .filter(|_| self.arrangement_bounds_epoch == self.drag_layout_generation)?;
        let width = bounds.width - self.track_header_width();
        (width.is_finite() && width > 0.0).then_some((grid, viewport, width))
    }

    pub(super) fn nudge_audio_clip(
        &mut self,
        clip: hex_project::ClipId,
        right: bool,
        platform: bool,
    ) {
        if self.edit_pending() {
            return;
        }
        let Some((grid, viewport, width)) = self.nudge_grid_context(platform) else {
            return;
        };
        if grid == super::musical_time::SnapGrid::Off {
            let step = self.step_frames() as i64;
            self.enqueue(SessionRequest::Edit(Command::NudgeClip {
                clip,
                frames: if right { step } else { -step },
            }));
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(item) = state.project.clip(clip) else {
            return;
        };
        let Some(start) = grid.adjacent_frame(&state.project, item.start(), viewport, width, right)
        else {
            return;
        };
        if start.checked_add(item.duration()).is_none() {
            self.error = Some("clip nudge exceeds the timeline range".into());
            return;
        }
        let Some(track) = state
            .project
            .tracks()
            .iter()
            .find(|track| track.clips().iter().any(|item| item.id() == clip))
            .map(|track| track.id())
        else {
            return;
        };
        self.enqueue(SessionRequest::Edit(Command::MoveClip {
            clip,
            track,
            start,
        }));
    }

    fn status_bar(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut bar = div()
            .id("editor-status-bar")
            .debug_selector(|| "editor-status-bar".into())
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_xs();
        if let Some(error) = &self.error {
            bar = bar.child(
                div()
                    .id("editor-status-message")
                    .debug_selector(|| "editor-status-message".into())
                    .flex_1()
                    .min_w(px(50.0))
                    .truncate()
                    .tooltip({
                        let message = error.clone();
                        move |_, cx| {
                            cx.new(|_| super::display_error::ErrorTooltip(message.clone()))
                                .into()
                        }
                    })
                    .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                    .child(error.display_text()),
            );
        } else {
            bar = bar.child(
                div()
                    .id("editor-status-message")
                    .debug_selector(|| "editor-status-message".into())
                    .flex_1()
                    .min_w(px(50.0))
                    .truncate()
                    .tooltip({
                        let message = self.status.clone();
                        move |_, cx| {
                            cx.new(|_| super::transport::MiniTooltip {
                                description: message.clone().into(),
                            })
                            .into()
                        }
                    })
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(&self.status)),
            );
        }
        bar = bar.child(self.controller_input_row(cx));
        if self.audio_batch.is_some() {
            bar = bar.child(
                div()
                    .id("cancel-audio-import")
                    .debug_selector(|| "cancel-audio-import".into())
                    .px_1()
                    .cursor_pointer()
                    .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                    .child(theme::text("cancel import"))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.cancel_audio_batch(cx);
                    })),
            );
        }
        let unsaved = self.state.as_ref().is_some_and(|state| state.unsaved);
        let revision_label = self.audio_admission.label();
        let pending_commands = self
            .worker_requests_pending
            .saturating_add(self.pending_requests.len());
        let revision_label = if pending_commands == 0 {
            revision_label
        } else {
            format!("{revision_label} · ctl {pending_commands} pending")
        };
        bar = bar.child(
            div()
                .id("editor-revision-status")
                .debug_selector(|| "editor-revision-status".into())
                .w(px(176.0))
                .min_w(px(64.0))
                .truncate()
                .text_right()
                .tooltip({
                    let description = self.audio_admission.description();
                    move |_, cx| {
                        cx.new(|_| super::transport::MiniTooltip {
                            description: description.clone().into(),
                        })
                        .into()
                    }
                })
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text(revision_label)),
        );
        let recovery_pending = self.recovery.generation().is_some();
        let activity = [
            recovery_pending.then_some("audio recovery pending"),
            self.export_pending.then_some("export pending"),
            self.saving_project.then_some("save pending"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let activity_label = activity
            .iter()
            .map(|label| match *label {
                "audio recovery pending" => "recovery",
                "export pending" => "export",
                "save pending" => "save",
                _ => "",
            })
            .collect::<Vec<_>>()
            .join(" · ");
        bar = bar
            .child(status_slot(
                "editor-activity-status",
                104.0,
                if recovery_pending {
                    theme::color(theme::PaletteRole::Secondary)
                } else {
                    theme::color(theme::PaletteRole::Muted)
                },
                activity_label,
                activity.join(" · "),
            ))
            .child(status_slot(
                "editor-unsaved-status",
                54.0,
                theme::color(theme::PaletteRole::Primary),
                if unsaved { "unsaved *" } else { "" }.to_owned(),
                if unsaved {
                    "unsaved changes".to_owned()
                } else {
                    String::new()
                },
            ));
        let meter_input = self.output_meter_frame.map_or(
            crate::output_meter::MeterInput::Offline,
            super::refresh::AudioFrame::meter_input,
        );
        bar = bar.child(
            crate::output_meter::render(meter_input).tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: crate::output_meter::description(meter_input).into(),
                })
                .into()
            }),
        );
        if let Some(status) = self.audio_status() {
            let applied_audio = status.applied_revision.load(Relaxed);
            let misses = status.deadline_misses.load(Relaxed);
            let callback_us = status.callback_max_ns.load(Relaxed) / 1_000;
            let render_fault = status.render_fault.load(Relaxed);
            let diagnostic = format!(
                "out {applied_audio} · cb {callback_us}µs · miss {misses}{}",
                if render_fault { " · render fault" } else { "" }
            );
            let diagnostic_label = if render_fault {
                format!("render fault · miss {misses}")
            } else if misses > 0 {
                format!("miss {misses} · out {applied_audio}")
            } else {
                format!("out {applied_audio} · {callback_us}µs")
            };
            bar = bar.child(
                div()
                    .id("editor-audio-diagnostics")
                    .debug_selector(|| "editor-audio-diagnostics".into())
                    .w(px(168.0))
                    .min_w(px(64.0))
                    .truncate()
                    .text_right()
                    .tooltip({
                        let description = diagnostic.clone();
                        move |_, cx| {
                            cx.new(|_| super::transport::MiniTooltip {
                                description: description.clone().into(),
                            })
                            .into()
                        }
                    })
                    .text_color(rgb(if misses == 0 && !render_fault {
                        theme::color(theme::PaletteRole::Muted)
                    } else {
                        theme::color(theme::PaletteRole::Primary)
                    }))
                    .child(theme::text(diagnostic_label)),
            );
        } else {
            bar = bar.child(
                div()
                    .id("editor-audio-diagnostics")
                    .debug_selector(|| "editor-audio-diagnostics".into())
                    .w(px(168.0))
                    .min_w(px(64.0))
                    .truncate()
                    .text_right()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("offline")),
            );
        }
        bar.child(super::device_panel::bottom_bar(self, cx))
    }
}

fn status_slot(
    id: &'static str,
    width: f32,
    color: u32,
    label: String,
    full_status: String,
) -> gpui::Stateful<gpui::Div> {
    let mut slot = div()
        .id(id)
        .debug_selector(move || id.into())
        .w(px(width))
        .h_full()
        .min_w(px(32.0))
        .truncate()
        .text_right();
    if !label.is_empty() {
        slot = slot
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: full_status.clone().into(),
                })
                .into()
            })
            .text_color(rgb(color))
            .child(theme::text(label));
    }
    slot
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.reconcile_track_resize(window, cx);
        self.reconcile_clip_name(window, cx);
        self.reconcile_clip_selection();
        self.reconcile_fade_gesture();
        if self.clip_view_open && self.gain_lane.track.is_some() {
            self.cancel_gain_draft(cx);
            self.gain_lane.track = None;
        }
        if let Some(state) = self.state.as_ref() {
            if let Some(track) = self.selected_track {
                self.gain_lane.change_owner(track, &state.project);
            }
        }
        self.clip_content_cache
            .reconcile(self.project_generation, self.state.as_ref(), cx);
        self.drag_layout_generation = self.drag_layout_generation.wrapping_add(1);
        let clip_view = self.clip_detail_view(window, cx);
        let workspace_content = if self.mode == super::EditorMode::Session {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.session_grid(window, cx))
                .child(clip_view)
                .child(self.mixer_strip(cx, true))
                .child(self.routing_controls(cx))
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.arrangement(window, cx))
                .child(clip_view)
                .child(self.mixer_strip(cx, false))
                .child(self.routing_controls(cx))
        };
        let workspace = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(self.browser_view(window, cx))
            .child(workspace_content.flex_1().min_w_0());
        let modifier_entity = cx.entity();
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .font_family(theme::FONT_FAMILY)
            .child(self.fade_pointer_events(cx))
            .track_focus(&self.focus)
            .key_context(if self.clip_name_edit.is_some() {
                "hex_clip_name hex_text_editing"
            } else if self.theme_settings.open {
                "hex_theme_settings"
            } else if self.controller_input_is_open() {
                "hex_controller_input"
            } else if self.context_menu.is_some() {
                "hex_context_menu"
            } else if self.gain_numeric_editing(cx) || self.sample_bpm_editing(cx) {
                "hex_gain_numeric_editor"
            } else {
                self.browser_key_context(window, cx)
            })
            .capture_any_mouse_down(cx.listener(|view, event, _, cx| {
                super::device_panel::capture_navigation_pointer_down(view, event, cx);
            }))
            .capture_any_mouse_up(cx.listener(|view, event: &MouseUpEvent, window, cx| {
                if event.button == MouseButton::Left && view.active_fade_drag.is_some() {
                    view.update_fade_gesture(event.position, event.modifiers.platform, cx);
                    view.finish_fade_gesture(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if event.button == MouseButton::Left
                    && let Some(drag) = view.active_trim_drag.clone()
                {
                    drag.snap_modifier.set(event.modifiers.platform);
                    drag.stretch_modifier.set(event.modifiers.shift);
                    drag.last_pointer_x.set(Some(f32::from(event.position.x)));
                    view.update_trim_preview(&drag, f32::from(event.position.x), cx);
                }
                if event.button == MouseButton::Left
                    && let Some(drag) = view.active_midi_trim_drag.clone()
                {
                    drag.snap_modifier.set(event.modifiers.platform);
                    drag.last_pointer_x.set(Some(f32::from(event.position.x)));
                    view.update_midi_trim_preview(&drag, f32::from(event.position.x), cx);
                }
            }))
            .on_modifiers_changed(move |event, window, cx| {
                modifier_entity.update(cx, |view, cx| {
                    view.refresh_drag_modifiers(
                        event.modifiers.platform,
                        event.modifiers.alt,
                        window,
                        cx,
                    );
                    if view.refresh_time_gesture_modifiers(
                        event.modifiers.platform,
                        event.modifiers.alt,
                    ) {
                        cx.notify();
                    }
                    if view.refresh_audio_event_modifiers(event.modifiers.platform) {
                        cx.notify();
                    }
                    view.update_fade_gesture(window.mouse_position(), event.modifiers.platform, cx);
                    view.refresh_trim_snap_modifier(
                        event.modifiers.platform,
                        event.modifiers.shift,
                        cx,
                    );
                    view.refresh_midi_trim_snap_modifier(event.modifiers.platform, cx);
                });
                window.refresh();
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    let device_drag_canceled = view.cancel_active_native_device_drag();
                    if device_drag_canceled {
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                        cx.notify();
                    }
                    if view.active_clip_drag.take().is_some() {
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                        cx.notify();
                    }
                    view.cancel_trim_gesture(cx);
                    if let Some(drag) = view.active_midi_trim_drag.take() {
                        drag.preview.borrow_mut().take();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    if view.cancel_trim_gesture(cx) | view.cancel_active_native_device_drag() {
                        cx.stop_active_drag(window);
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                        cx.notify();
                    }
                }),
            )
            .capture_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                if !view.fade_key_down(event, window, cx) {
                    view.computer_keyboard_down(event, window, cx);
                }
            }))
            .capture_key_up(cx.listener(|view, event: &gpui::KeyUpEvent, _, cx| {
                if event.keystroke.key == "f" && view.fade_key_held {
                    view.fade_key_held = false;
                    cx.notify();
                    cx.stop_propagation();
                }
                if view.computer_keyboard_up(&event.keystroke.key) {
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && (view.cancel_trim_gesture(cx) | view.cancel_active_native_device_drag())
                {
                    cx.stop_active_drag(window);
                    cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(|view, _: &super::SearchBrowser, window, cx| {
                view.search_browser(window, cx);
            }))
            .on_action(cx.listener(|view, _: &super::RenameClip, window, cx| {
                view.begin_clip_name(window, cx);
            }))
            .on_action(cx.listener(|view, _: &super::CommitClipName, window, cx| {
                view.finish_clip_name(true, window, cx);
            }))
            .on_action(cx.listener(|view, _: &super::CancelClipName, window, cx| {
                view.finish_clip_name(false, window, cx);
            }))
            .on_action(cx.listener(|view, _: &super::ToggleBrowser, window, cx| {
                view.toggle_browser(window, cx);
            }))
            .on_action(cx.listener(|view, _: &super::OpenSettings, window, cx| {
                view.open_theme_settings(window, cx);
            }))
            .on_action(cx.listener(|view, _: &super::BrowserPrevious, window, cx| {
                view.browser_previous(window, cx);
            }))
            .on_action(cx.listener(|view, _: &super::BrowserNext, window, cx| {
                view.browser_next(window, cx);
            }))
            .on_action(
                cx.listener(|view, _: &super::BrowserPreviewSelected, window, cx| {
                    view.browser_preview_selected(window, cx);
                }),
            )
            .on_action(cx.listener(|view, _: &super::BrowserActivate, window, cx| {
                view.browser_activate(window, cx);
            }))
            .on_action(cx.listener(|view, _: &super::BrowserEscape, window, cx| {
                if view.cancel_trim_gesture(cx) | view.cancel_track_resize(cx) {
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    return;
                }
                if view.active_fade_drag.take().is_some() {
                    cx.stop_propagation();
                    cx.notify();
                } else if view.active_native_device_drag.is_some() {
                    cx.stop_active_drag(window);
                    view.cancel_active_native_device_drag();
                    cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                    cx.stop_propagation();
                    cx.notify();
                } else {
                    view.browser_escape(window, cx);
                }
            }))
            .on_action(
                cx.listener(|view, action: &super::ThemeSettingsEscape, window, cx| {
                    super::theme_settings::handle_escape_action(view, action, window, cx);
                }),
            )
            .capture_action(
                cx.listener(|view, _: &hex_widgets::NumericEscape, window, cx| {
                    if view.theme_settings.open {
                        view.cancel_theme_settings(window, cx);
                        cx.stop_propagation();
                    } else if view.cancel_gain_draft(cx) {
                        view.focus.focus(window);
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_action(cx.listener(|view, _: &TogglePlay, _, cx| {
                view.toggle_play();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &Stop, window, cx| {
                cx.stop_active_drag(window);
                view.active_clip_drag = None;
                view.cancel_active_native_device_drag();
                view.cancel_arrangement_pan();
                view.device_panel.navigation.cancel();
                view.stop();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &CancelDragOrStop, window, cx| {
                if view.cancel_trim_gesture(cx) | view.cancel_track_resize(cx) {
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    return;
                }
                if view.active_fade_drag.take().is_some() {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if view.cancel_audio_batch(cx) {
                    cx.stop_propagation();
                    return;
                }
                view.cancel_scrub();
                let gain_canceled = view.cancel_gain_draft(cx);
                let midi_canceled = view.cancel_midi_note_gesture();
                let time_selection_canceled =
                    view.cancel_time_selection_gesture() | view.cancel_clip_paint();
                let selection_cleared = if !midi_canceled && view.midi_focus.is_focused(window) {
                    view.midi_marquee = None;
                    view.midi_selection.clear()
                } else {
                    false
                };
                let drag_stopped = cx.stop_active_drag(window);
                let device_drag_stopped = view.cancel_active_native_device_drag();
                let pan_canceled =
                    view.cancel_arrangement_pan() | view.device_panel.navigation.cancel();
                if drag_stopped {
                    view.active_clip_drag = None;
                }
                if !gain_canceled
                    && !midi_canceled
                    && !selection_cleared
                    && !time_selection_canceled
                    && !drag_stopped
                    && !device_drag_stopped
                    && !pan_canceled
                {
                    view.stop();
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &SelectAllMidiNotes, _, cx| {
                view.select_all_midi_notes();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &ToggleMidiDrawMode, _, cx| {
                view.midi_draw_mode = !view.midi_draw_mode;
                view.cancel_midi_note_gesture();
                view.midi_group_gesture = None;
                view.midi_marquee = None;
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &ImportAudio, _, cx| view.import_dialog(cx)))
            .on_action(cx.listener(|view, _: &NewProject, _, cx| {
                view.new_project(hex_session::DocumentAudioMode::Resident);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &NewStreamingProject, _, cx| {
                view.new_project(hex_session::DocumentAudioMode::VerifiedStreaming);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &OpenProject, _, cx| {
                view.open_dialog(hex_session::DocumentAudioMode::Resident, cx)
            }))
            .on_action(cx.listener(|view, _: &OpenStreamingProject, _, cx| {
                view.open_dialog(hex_session::DocumentAudioMode::VerifiedStreaming, cx)
            }))
            .on_action(cx.listener(|view, _: &OpenRelinkedProject, window, cx| {
                view.project_recovery_dialog(super::project_recovery::OpenKind::Relink, window, cx)
            }))
            .on_action(cx.listener(|view, _: &RecoverUntitledProject, window, cx| {
                view.project_recovery_dialog(
                    super::project_recovery::OpenKind::Untitled,
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|view, _: &RecoverSavedProject, window, cx| {
                view.project_recovery_dialog(
                    super::project_recovery::OpenKind::SavedEdits,
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|view, _: &RecoverPreviousSave, window, cx| {
                view.project_recovery_dialog(
                    super::project_recovery::OpenKind::PreviousSave,
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|view, action: &OpenRecentProject, _, cx| {
                if super::device_panel::finalize_controls(view, cx) {
                    view.open_recent_project(action);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &SaveProject, _, cx| {
                view.save_project(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &SaveAsProject, _, cx| {
                view.save_as_dialog(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &Undo, _, cx| {
                if !view.cancel_gain_draft(cx) && super::device_panel::finalize_controls(view, cx) {
                    view.cancel_duplicate_chain();
                    view.enqueue_device_barrier(SessionRequest::Undo);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &Redo, _, cx| {
                if super::device_panel::finalize_controls(view, cx) {
                    view.cancel_duplicate_chain();
                    view.enqueue_device_barrier(SessionRequest::Redo);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &MoveLeft, window, cx| {
                view.cancel_duplicate_chain();
                match super::session_navigation::arrow_target(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    super::session_navigation::ArrowTarget::Piano => {
                        view.nudge_selected_note(false);
                    }
                    super::session_navigation::ArrowTarget::Session => {
                        view.navigate_session(
                            super::session_navigation::Direction::Left,
                            window,
                            cx,
                        );
                    }
                    super::session_navigation::ArrowTarget::Arrangement => {
                        if let Some(clip) = view.selected_arrangement_midi_clip_data() {
                            view.nudge_midi_clip(clip, false, window.modifiers().platform);
                        } else if let Some((_, clip, ..)) = view.selected_clip_data() {
                            view.nudge_audio_clip(clip, false, window.modifiers().platform);
                        }
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &MoveRight, window, cx| {
                view.cancel_duplicate_chain();
                match super::session_navigation::arrow_target(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    super::session_navigation::ArrowTarget::Piano => {
                        view.nudge_selected_note(true);
                    }
                    super::session_navigation::ArrowTarget::Session => {
                        view.navigate_session(
                            super::session_navigation::Direction::Right,
                            window,
                            cx,
                        );
                    }
                    super::session_navigation::ArrowTarget::Arrangement => {
                        if let Some(clip) = view.selected_arrangement_midi_clip_data() {
                            view.nudge_midi_clip(clip, true, window.modifiers().platform);
                        } else if let Some((_, clip, ..)) = view.selected_clip_data() {
                            view.nudge_audio_clip(clip, true, window.modifiers().platform);
                        }
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &TrimLeft, window, cx| {
                view.cancel_duplicate_chain();
                if view.midi_focus.is_focused(window) {
                    if view.midi_selection.len() == 1 {
                        if let Some(note) = view.midi_selection.primary() {
                            if let Some(state) = view.state.as_ref() {
                                view.update_note_field(
                                    note,
                                    state.revision,
                                    view.project_generation,
                                    super::midi::NoteField::Duration(-1),
                                );
                            }
                        }
                    } else if view.midi_selection.len() > 1 {
                        view.adjust_selected_group_current(
                            super::midi_group_drag::GroupParameter::Duration(-1),
                        );
                    }
                } else if view.selected_midi_clip_data().is_some() {
                    view.error = Some("midi clip trimming is not supported yet".into());
                } else if let Some((_, clip, ..)) = view.selected_clip_data() {
                    view.enqueue(SessionRequest::Edit(Command::TrimClipEndBy {
                        clip,
                        frames: -(view.step_frames() as i64),
                    }));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &TrimRight, window, cx| {
                view.cancel_duplicate_chain();
                if view.midi_focus.is_focused(window) {
                    if view.midi_selection.len() == 1 {
                        if let Some(note) = view.midi_selection.primary() {
                            if let Some(state) = view.state.as_ref() {
                                view.update_note_field(
                                    note,
                                    state.revision,
                                    view.project_generation,
                                    super::midi::NoteField::Duration(1),
                                );
                            }
                        }
                    } else if view.midi_selection.len() > 1 {
                        view.adjust_selected_group_current(
                            super::midi_group_drag::GroupParameter::Duration(1),
                        );
                    }
                } else if view.selected_midi_clip_data().is_some() {
                    view.error = Some("midi clip trimming is not supported yet".into());
                } else if let Some((_, clip, ..)) = view.selected_clip_data() {
                    view.enqueue(SessionRequest::Edit(Command::TrimClipEndBy {
                        clip,
                        frames: view.step_frames() as i64,
                    }));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &super::ToggleSnapGrid, _, cx| {
                view.snap_grid = if view.snap_grid == super::musical_time::SnapGrid::Off {
                    super::musical_time::SnapGrid::Adaptive
                } else {
                    super::musical_time::SnapGrid::Off
                };
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &CopyTimeSelection, window, cx| {
                view.copy_time_selection(false, window);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &CutTimeSelection, window, cx| {
                view.copy_time_selection(true, window);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &PasteTimeSelection, window, cx| {
                view.paste_time_selection(window);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &InsertTimeSelection, window, cx| {
                view.insert_time_selection(window);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &DeleteClip, window, cx| {
                if view.delete_time_selection(window) {
                    cx.notify();
                    return;
                }
                let target = super::session_grid::resolve_delete_target(
                    view.state.as_ref().map(|state| state.project.as_ref()),
                    view.mode,
                    view.midi_focus.is_focused(window),
                    !view.midi_selection.is_empty(),
                    view.selected_cell,
                    view.selected_scene,
                    view.selected_clip,
                );
                match target {
                    Some(super::session_grid::DeleteTarget::MidiNotes) => {
                        view.remove_selected_note();
                    }
                    Some(super::session_grid::DeleteTarget::SessionCell { track, scene }) => {
                        if view.enqueue(SessionRequest::Edit(Command::ClearSessionCell {
                            track,
                            scene,
                        })) {
                            view.cancel_duplicate_chain();
                            view.cancel_midi_note_gesture();
                            view.selected_clip = None;
                        }
                    }
                    Some(super::session_grid::DeleteTarget::SessionScene(scene)) => {
                        if view.enqueue(SessionRequest::Edit(Command::RemoveScene(scene))) {
                            view.cancel_duplicate_chain();
                            view.cancel_midi_note_gesture();
                            view.selected_scene = None;
                            view.selected_cell = None;
                            view.selected_clip = None;
                            view.midi_selection.clear();
                        }
                    }
                    Some(super::session_grid::DeleteTarget::ArrangementClip(clip)) => {
                        let command = if view.selected_midi_clip_data().is_some() {
                            Command::RemoveMidiClip(clip)
                        } else {
                            Command::RemoveClip(clip)
                        };
                        if view.enqueue(SessionRequest::Edit(command)) {
                            view.cancel_duplicate_chain();
                            view.cancel_midi_note_gesture();
                            view.selected_clip = None;
                        }
                    }
                    None => {}
                }
                cx.notify();
            }))
            .on_action(
                cx.listener(|view, _: &super::LaunchSessionSelection, window, cx| {
                    if super::session_navigation::session_editor_has_focus(
                        view.mode,
                        view.midi_focus.is_focused(window),
                    ) {
                        view.launch_session_selection();
                    }
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|view, _: &DuplicateClip, window, cx| {
                if view.mode == super::EditorMode::Session
                    && !view.midi_focus.is_focused(window)
                    && view.selected_cell.is_none()
                    && view.selected_scene.is_some()
                {
                    view.duplicate_selected_scene();
                } else if let Some(clip) = view.selected_arrangement_midi_clip_data() {
                    view.duplicate_midi_clip(clip);
                } else if view.mode == super::EditorMode::Session {
                    view.error = Some("session cell duplication is not available yet".into());
                } else {
                    view.duplicate_selected();
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &super::MoveSceneUp, window, cx| {
                if super::session_navigation::session_editor_has_focus(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    view.move_selected_scene(super::session_navigation::Direction::Up);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &super::MoveSceneDown, window, cx| {
                if super::session_navigation::session_editor_has_focus(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    view.move_selected_scene(super::session_navigation::Direction::Down);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &SplitClip, _, cx| {
                view.split_at_edit_cursor();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &ExportProject, window, cx| {
                view.export_dialog(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &SetLoopFromClip, _, cx| {
                view.set_loop_from_clip();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &ToggleLoop, _, cx| {
                view.toggle_loop();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &ZoomIn, _, cx| {
                view.zoom_timeline(true);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &ZoomOut, _, cx| {
                view.zoom_timeline(false);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &FitTimeline, _, cx| {
                view.fit_timeline();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &PanTimelineLeft, _, cx| {
                view.pan_timeline(false);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &PanTimelineRight, _, cx| {
                view.pan_timeline(true);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &ToggleSolo, window, cx| {
                if window
                    .context_stack()
                    .iter()
                    .any(|context| context.contains("hex_text_editing"))
                {
                    return;
                }
                if let Some(track) = view.selected_track {
                    view.toggle_solo(track, true);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &AdditiveSolo, window, cx| {
                if window
                    .context_stack()
                    .iter()
                    .any(|context| context.contains("hex_text_editing"))
                {
                    return;
                }
                if let Some(track) = view.selected_track {
                    view.toggle_solo(track, false);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &PreviousTrack, window, cx| {
                match super::session_navigation::arrow_target(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    super::session_navigation::ArrowTarget::Piano => {
                        view.transpose_selected_note(true);
                    }
                    super::session_navigation::ArrowTarget::Session => {
                        view.navigate_session(super::session_navigation::Direction::Up, window, cx);
                    }
                    super::session_navigation::ArrowTarget::Arrangement => {
                        view.select_neighbor_track(false, window, cx);
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &NextTrack, window, cx| {
                match super::session_navigation::arrow_target(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    super::session_navigation::ArrowTarget::Piano => {
                        view.transpose_selected_note(false);
                    }
                    super::session_navigation::ArrowTarget::Session => {
                        view.navigate_session(
                            super::session_navigation::Direction::Down,
                            window,
                            cx,
                        );
                    }
                    super::session_navigation::ArrowTarget::Arrangement => {
                        view.select_neighbor_track(true, window, cx);
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &MidiOctaveUp, window, cx| {
                if view.midi_focus.is_focused(window) {
                    view.transpose_selected_note_by(12);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &MidiOctaveDown, window, cx| {
                if view.midi_focus.is_focused(window) {
                    view.transpose_selected_note_by(-12);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &AddTrack, _, cx| {
                view.add_track();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &AddMidiTrack, _, cx| {
                view.add_midi_track();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &AddMidiClip, _, cx| {
                view.add_midi_clip();
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &RemoveTrack, _, cx| {
                view.remove_track(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &Quit, window, cx| {
                view.request_quit(window, cx);
            }))
            .child(self.title_bar(cx))
            .child(self.transport(cx))
            .child(workspace)
            .child(self.status_bar(cx))
            .children(self.return_send_menu_overlay(window, cx))
            .children(self.floating_browser_audio_drag_ghost())
            .children(self.floating_clip_drag_ghost())
            .children(self.context_menu_overlay(window, cx))
            .children(self.controller_input_overlay(cx))
            .children(super::theme_settings::overlay(self, cx))
    }
}
