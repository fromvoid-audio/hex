use super::device_automation::RangeEdit;
use super::{
    AddMidiClip, AddMidiTrack, AddTrack, AdditiveSolo, CancelDragOrStop, CopyTimeSelection,
    CutTimeSelection, DeleteClip, DeleteSectionMarker, DuplicateClip, EditorView, ExportProject,
    FitTimeline, GroupSelection, ImportAudio, InsertTimeSelection, LoopSelection, MidiOctaveDown,
    MidiOctaveUp, MoveLeft, MoveRight, NewProject, NewStreamingProject, NextTrack,
    OpenAutomationRecovery, OpenProject, OpenRecentProject, OpenRelinkedProject,
    OpenStreamingProject, PanTimelineLeft, PanTimelineRight, PasteTimeSelection, PreviousTrack,
    Quit, RecoverKeptSession, RecoverPreviousSave, RecoverSavedProject, RecoverUntitledProject,
    Redo, RemoveTrack, SaveAsProject, SaveProject, SelectAllMidiNotes, SplitClip, Stop, ToggleLoop,
    ToggleMidiDrawMode, TogglePlay, ToggleSolo, TrimLeft, TrimRight, Undo, ZoomIn, ZoomOut,
};
use crate::diagnostics::ActionTraceContext as _;
use crate::theme;
use gpui::{Context, MouseButton, MouseUpEvent, Window, div, prelude::*, px, rgb};
use hex_project::Command;
use hex_session::service::Request as SessionRequest;
use std::sync::atomic::Ordering::Relaxed;

/// Status-bar disk-overload slot; fits the four-character `disk` label.
pub(super) const DISK_OVERLOAD_WIDTH: f32 = 32.0;

impl EditorView {
    pub(super) fn history_action(&mut self, request: SessionRequest, cx: &mut Context<Self>) {
        if !self.cancel_sampler_drafts(cx)
            && !self.cancel_gain_draft(cx)
            && super::device_panel::finalize_controls(self, cx)
        {
            self.cancel_duplicate_chain();
            self.cancel_queued_section();
            self.enqueue_device_barrier(request);
        }
        cx.notify();
    }

    fn duplicate_key(
        &mut self,
        kind: super::duplicate::DuplicateKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if kind == super::duplicate::DuplicateKind::Independent
            && self.computer_keyboard.accepts_key("d")
        {
            return;
        }
        if window
            .context_stack()
            .iter()
            .any(|context| context.contains("hex_text_editing"))
        {
            return;
        }
        if self.edit_automation_range(RangeEdit::Duplicate, window, cx) {
            return;
        }
        if self.track_clipboard_shortcut(window, super::track_clipboard::Shortcut::Duplicate) {
            cx.notify();
            return;
        }
        if self.duplicate_section(window) {
            cx.notify();
            return;
        }
        if self.midi_focus.is_focused(window) {
            if !self.duplicate_midi_time_selection() {
                self.duplicate_piano_notes();
            }
            cx.notify();
            return;
        }
        if self.duplicate_time_selection(window) {
            cx.notify();
            return;
        }
        if !self.focus.is_focused(window) && !self.midi_focus.is_focused(window) {
            return;
        }
        if self.mode == super::EditorMode::Session
            && !self.midi_focus.is_focused(window)
            && self.selected_cell.is_none()
            && self.selected_scene.is_some()
        {
            self.duplicate_selected_scene();
        } else if kind == super::duplicate::DuplicateKind::Independent {
            self.duplicate_selected();
        } else {
            self.duplicate_selected_with_kind(kind);
        }
        cx.notify();
    }

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
        if grid.is_off() {
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
        if grid.is_off() {
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

    fn project_title_label(&self) -> gpui::Stateful<gpui::Div> {
        let title = super::header::project_title(
            self.state()
                .and_then(|state| state.document_path.as_deref()),
        );
        let full_title = theme::text(&title);
        div()
            .id("project-title-label")
            .debug_selector(|| "project-title-label".into())
            .w(px(120.0))
            .min_w(px(32.0))
            .line_clamp(1)
            .text_ellipsis()
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: full_title.clone(),
                })
                .into()
            })
            .child(theme::text(title))
    }

    fn status_bar(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut bar = div()
            .id("editor-status-bar")
            .debug_selector(|| "editor-status-bar".into())
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_xs()
            .child(self.browser_toggle_button(cx))
            .child(self.project_title_label());
        let scan_status = self.plugin_scan_status();
        let plugin_status = self
            .pending_plugin_effect
            .as_ref()
            .map(|pending| pending.status.as_str())
            .or(scan_status.as_deref());
        if let Some(error) = &self.error
            && plugin_status.is_none()
        {
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
            let status = plugin_status.unwrap_or(&self.status);
            bar = bar.child(
                div()
                    .id("editor-status-message")
                    .debug_selector(|| "editor-status-message".into())
                    .flex_1()
                    .min_w(px(50.0))
                    .truncate()
                    .tooltip({
                        let message = status.to_owned();
                        move |_, cx| {
                            cx.new(|_| super::transport::MiniTooltip {
                                kind: crate::tooltips::Kind::Identification,
                                description: message.clone().into(),
                            })
                            .into()
                        }
                    })
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(status)),
            );
        }
        let offline = self
            .state
            .as_ref()
            .map_or(0, |state| state.project.offline_assets().count());
        if offline > 0 {
            bar = bar.child(
                div()
                    .id("missing-media-indicator")
                    .debug_selector(|| "missing-media-indicator".into())
                    .px_1()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .on_click(cx.listener(|view, _, _, cx| view.missing_media_show_offline(cx)))
                    .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                    .tooltip(|_, cx| {
                        cx.new(|_| super::transport::MiniTooltip {
                            kind: crate::tooltips::Kind::Identification,
                            description: theme::text(
                                "these files were missing when the project opened; \
                                 their clips play silence. click to relink them",
                            ),
                        })
                        .into()
                    })
                    .child(theme::text(format!("missing files: {offline}"))),
            );
        }
        if let Some(label) = self.update_label() {
            bar = bar.child(
                div()
                    .id("local-update-status")
                    .px_1()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(label)),
            );
        }
        if let Some(failure) = self.recovery_failure.as_ref().cloned() {
            let can_restart = failure.can_restart();
            bar = bar.child(
                div()
                    .id("automatic-recovery-unavailable")
                    .debug_selector(|| "automatic-recovery-unavailable".into())
                    .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                    .child(theme::text(if can_restart {
                        "recovery unavailable"
                    } else {
                        "recovery needs attention"
                    }))
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::display_error::RecoveryTooltip {
                            failure: std::sync::Arc::clone(&failure),
                        })
                        .into()
                    }),
            );
            if can_restart {
                bar = bar.child(
                    div()
                        .id("restart-automatic-recovery")
                        .debug_selector(|| "restart-automatic-recovery".into())
                        .px_1()
                        .cursor_pointer()
                        .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                        .child(theme::text("start fresh recovery"))
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.restart_recovery_dialog(window, cx)
                        })),
                );
            }
        }
        if let Some((label, actionable)) = self.automation_recovery_status() {
            bar = bar.child(
                div()
                    .id("automation-take-recovery")
                    .debug_selector(|| "automation-take-recovery".into())
                    .px_1()
                    .flex_shrink_0()
                    .text_color(rgb(theme::color(if actionable {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::Muted
                    })))
                    .when(actionable, |item| {
                        item.cursor_pointer().on_click(
                            cx.listener(|view, _, _, cx| view.start_automation_recovery(cx)),
                        )
                    })
                    .child(theme::text(label)),
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
                            kind: crate::tooltips::Kind::Identification,
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
                    kind: crate::tooltips::Kind::Identification,
                    description: crate::output_meter::description(meter_input).into(),
                })
                .into()
            }),
        );
        bar = bar.child(disk_overload_slot(&self.disk_overload));
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
                                kind: crate::tooltips::Kind::Identification,
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

/// Fixed-width disk-overload slot, so lighting it never shifts the bar. Its
/// label and tooltip are lowercase app text through `theme::text`.
fn disk_overload_slot(state: &super::disk_overload::DiskOverload) -> gpui::Stateful<gpui::Div> {
    let slot = div()
        .id("editor-disk-overload")
        .debug_selector(|| "editor-disk-overload".into())
        .w(px(DISK_OVERLOAD_WIDTH))
        .h_full()
        .flex_none()
        .truncate()
        .text_center();
    if !state.shown() {
        return slot;
    }
    let description = super::disk_overload::description(state.count());
    slot.tooltip(move |_, cx| {
        cx.new(|_| super::transport::MiniTooltip {
            kind: crate::tooltips::Kind::Identification,
            description: description.clone().into(),
        })
        .into()
    })
    .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
    .child(
        div()
            .debug_selector(|| "editor-disk-overload-lit".into())
            .child(theme::text("disk")),
    )
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
                    kind: crate::tooltips::Kind::Identification,
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
        self.wake_poll_if_busy();
        self.reconcile_update_ready(window, cx);
        #[cfg(all(test, feature = "ui-test-support"))]
        let _navigation_timer =
            super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::Editor);
        self.reconcile_zoom_fit();
        self.sync_hardware_step_context(window);
        self.restore_sampler_parameter_focus(window);
        self.reconcile_project_open(window, cx);
        self.reconcile_format_upgrade_prompt(window, cx);
        self.reconcile_leftover_recovery(window, cx);
        self.reconcile_truncated_import_offer(window, cx);
        self.reconcile_track_resize(window, cx);
        self.reconcile_track_reorder(window);
        super::browser_input::restyle(&self.browser.search, cx);
        if let Some(input) = &self.tuning.name {
            super::browser_input::restyle(input, cx);
        }
        self.reconcile_clip_name(window, cx);
        self.reconcile_sections(window, cx);
        self.reconcile_arrangement_focus();
        if let Some(state) = &self.state {
            self.track_hierarchy_indent.reconcile(
                self.project_generation,
                state.revision,
                &state.project,
            );
        }

        self.sync_tuning(cx);
        self.tick_tuning_midi_activity();
        self.reconcile_clip_selection();
        self.reconcile_detail_context();
        self.reconcile_track_name(window, cx);
        self.reconcile_fade_gesture();
        self.reconcile_clip_gain();
        if self.clip_view_open && self.gain_lane.track.is_some() && !self.automation_view.enabled {
            self.cancel_gain_draft(cx);
            self.gain_lane.track = None;
        }
        if !self.automation_view.enabled
            && let Some(state) = self.state.as_ref()
            && let Some(track) = self.selected_track
        {
            self.gain_lane.change_owner(track, &state.project);
        }
        self.clip_content_cache
            .reconcile(self.project_generation, self.state.as_ref(), cx);
        self.drag_layout_generation = self.drag_layout_generation.wrapping_add(1);
        let clip_view = self.clip_detail_view(window, cx);
        self.reconcile_device_action_menu(window, cx);
        let workspace = self.render_workspace(window, cx, clip_view);
        self.restore_clip_control_focus(window, cx);
        let modifier_entity = cx.entity();
        let console_drag_entity = cx.entity();
        let trim_pointer_entity = cx.entity();
        let midi_trim_pointer_entity = cx.entity();
        let key_context = if super::device_panel::picker_is_open(self) {
            "hex_device_picker_root"
        } else if self.changelog.is_open() {
            "hex_changelog"
        } else if self.track_name.is_editing() {
            "hex_track_name hex_text_editing"
        } else if self.clip_name_edit.is_some() || self.sections.naming() || self.locators.naming()
        {
            "hex_clip_name hex_text_editing"
        } else if self.tuning.open {
            "hex_tuning"
        } else if self.theme_settings.open {
            "hex_theme_settings"
        } else if self.controller_input_is_open() {
            "hex_controller_input"
        } else if self.context_menu.is_some() {
            "hex_context_menu"
        } else if self.project_tempo_editing(cx) {
            "hex_project_tempo_editor hex_text_editing"
        } else if self.gain_numeric_editing(cx) || self.clip_numeric_editing(cx) {
            "hex_gain_numeric_editor"
        } else if self.mode == super::EditorMode::Arrangement
            && self.automation_view.enabled
            && self.focus.is_focused(window)
        {
            "hex_editor hex_arrangement_automation"
        } else if self.mode == super::EditorMode::Arrangement
            && self.automation_view.enabled
            && self.track_header_focused(window)
        {
            "hex_editor hex_automation_track_header"
        } else {
            self.browser_key_context(window, cx)
        };
        let mut key_context = gpui::KeyContext::parse(key_context).unwrap_or_default();
        if self.has_arrangement_sections() {
            key_context.add("hex_arrangement_sections");
        }
        div()
            // Pointer presses leave the idle poll cadence before any handler runs.
            .capture_any_mouse_down(cx.listener(|view, _, _, _| view.wake_poll()))
            .when(crate::diagnostics::actions_enabled(cx), |root| {
                root.capture_any_mouse_down(|event, _, cx| {
                    crate::diagnostics::record_pointer(
                        cx,
                        "pointer_down",
                        event.button,
                        event.position,
                        event.modifiers,
                    )
                })
                .capture_any_mouse_up(|event, _, cx| {
                    crate::diagnostics::record_pointer(
                        cx,
                        "pointer_up",
                        event.button,
                        event.position,
                        event.modifiers,
                    )
                })
            })
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .font_family(theme::FONT_FAMILY)
            .child(self.fade_pointer_events(cx))
            .child(self.project_spectrum_pointer_events(cx))
            .child(self.clip_detail_pointer_events(cx))
            .track_focus(&self.focus)
            .key_context(key_context)
            // The canonical source handle may leave the viewport while its
            // resize preview remains active. Keep the same captured payload's
            // latest pointer on this stable root, not a virtualized child.
            .on_drag_move::<super::trim::TrimDrag>(move |event, _, cx| {
                if let Some(drag) = event.dragged_item().downcast_ref::<super::trim::TrimDrag>() {
                    trim_pointer_entity.update(cx, |view, cx| {
                        view.refresh_trim_pointer(drag, &event.event, cx);
                    });
                }
            })
            .on_drag_move::<super::trim::MidiTrimDrag>(move |event, _, cx| {
                if let Some(drag) = event
                    .dragged_item()
                    .downcast_ref::<super::trim::MidiTrimDrag>()
                {
                    midi_trim_pointer_entity.update(cx, |view, cx| {
                        view.refresh_midi_trim_pointer(drag, &event.event, cx);
                    });
                }
            })
            .on_drag_move::<super::drag::BrowserAudioDrag>(move |_, _, cx| {
                console_drag_entity.update(cx, |view, cx| {
                    if view.mode == super::EditorMode::Session
                        && let Some(drag) = view.active_browser_audio_drag.as_ref()
                        && matches!(
                            drag.destination.get(),
                            Some(super::drag::BrowserDropDestination::Console(_))
                        )
                    {
                        // Capture runs before child receivers: hover is rebuilt
                        // from this event's actual hit surfaces, never old selection.
                        drag.destination.set(None);
                        cx.notify();
                    }
                });
            })
            .capture_any_mouse_down(cx.listener(|view, event, window, cx| {
                super::device_panel::capture_navigation_pointer_down(view, event, window, cx);
            }))
            .capture_any_mouse_up(cx.listener(|view, event: &MouseUpEvent, window, cx| {
                if event.button == MouseButton::Left
                    && let Some(drag) = view.active_browser_audio_drag.clone()
                {
                    cx.defer_in(window, move |view, _, cx| {
                        // Child drop receivers run before this deferred cleanup.
                        // Retire only the captured Browser payload, never Trim.
                        view.clear_browser_audio_drag(&drag);
                        cx.notify();
                    });
                }
                if view.finish_trim_pointer_release(event, false, window, cx) {
                    cx.stop_active_drag(window);
                    cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                    cx.stop_propagation();
                }
            }))
            .on_modifiers_changed(move |event, window, cx| {
                modifier_entity.update(cx, |view, cx| {
                    // Section hover paint follows modifiers even without pointer motion.
                    cx.notify();
                    view.tuning.shift = event.modifiers.shift;
                    if view.option_held != event.modifiers.alt {
                        view.option_held = event.modifiers.alt;
                        cx.notify();
                    }
                    if view.active_native_device_drag.as_ref().is_some_and(|drag| {
                        matches!(
                            drag.source,
                            super::drag::NativeDeviceDragSource::Existing { .. }
                        )
                    }) {
                        cx.set_active_drag_cursor_style(
                            if event.modifiers.alt {
                                gpui::CursorStyle::DragCopy
                            } else {
                                gpui::CursorStyle::ClosedHand
                            },
                            window,
                        );
                    }
                    if view.sections.held() {
                        let position = window.mouse_position();
                        if view.update_section_pointer(
                            f32::from(position.x),
                            f32::from(position.y),
                            event.modifiers,
                        ) {
                            cx.notify();
                        }
                    }
                    if view.refresh_piano_note_modifiers(event.modifiers, window) {
                        cx.notify();
                    }
                    if view.refresh_loop_rail_modifiers(event.modifiers, window) {
                        cx.notify();
                    }
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
                    view.refresh_midi_trim_snap_modifier(
                        event.modifiers.platform,
                        event.modifiers.shift,
                        cx,
                    );
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
                    if view.retire_active_clip_drag() {
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
                if super::device_panel::picker_is_open(view) {
                    return;
                }
                if event.keystroke.key == "space"
                    && event.keystroke.modifiers == gpui::Modifiers::default()
                    && !window.context_stack().iter().any(|context| {
                        context.contains("hex_text_editing")
                            || context.contains("hex_text_input")
                            || context.contains("hex_browser_search")
                    })
                {
                    if !event.is_held {
                        let _scope = crate::diagnostics::action_scope(cx, "hex_editor::TogglePlay");
                        view.toggle_play();
                        cx.notify();
                    }
                    cx.stop_propagation();
                    return;
                }
                if view.section_loop_key_down(event, window) {
                    cx.stop_propagation();
                    return;
                }
                if !view.time_selection_key_down(event, window, cx)
                    && !view.tuning_audition_down(event, window, cx)
                    && !view.clip_gain_key_down(event, cx)
                    && !view.fade_key_down(event, window, cx)
                {
                    view.computer_keyboard_down(event, window, cx);
                }
            }))
            .capture_key_up(cx.listener(|view, event: &gpui::KeyUpEvent, window, cx| {
                if view.section_loop_key_up(event, window, cx) {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if view.drag_bounce_key_up(event) {
                    cx.notify();
                }
                if view.time_selection_key_up(event, cx) {
                    cx.stop_propagation();
                }
                super::device_panel::selection_key_up(view, event, window, cx);
                if view.tuning_audition_up(&event.keystroke.key) {
                    cx.stop_propagation();
                    cx.notify();
                }
                if event.keystroke.key == "f" && view.audio_event_editor.fade_key_held {
                    view.audio_event_editor.fade_key_held = false;
                    cx.notify();
                    cx.stop_propagation();
                }
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
                if super::device_panel::picker_is_open(view) {
                    return;
                }
                if view.workspace_tab(event, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                if view.arrangement_view_key(event, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                if view.reverse_clip_key(event, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                if view.activation_key(event, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape"
                    && (view.cancel_trim_gesture(cx)
                        | view.cancel_active_native_device_drag()
                        | view.device_panel.mapping_source.take().is_some())
                {
                    cx.stop_active_drag(window);
                    cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action(cx.trace(|view, _: &super::SearchBrowser, window, cx| {
                view.search_browser(window, cx);
            }))
            .capture_action(
                cx.trace(|view, _: &super::CreateFadesOrSearch, window, cx| {
                    if !view.create_selection_fades(window, cx) {
                        view.search_browser(window, cx);
                    }
                    cx.notify();
                }),
            )
            .on_action(
                cx.trace(|view, _: &super::SelectPreviousReturn, window, cx| {
                    view.select_adjacent_console_return(false, window, cx);
                }),
            )
            .on_action(cx.trace(|view, _: &super::SelectNextReturn, window, cx| {
                view.select_adjacent_console_return(true, window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::RenameTrack, window, cx| {
                view.begin_track_name(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::CommitTrackName, window, cx| {
                view.finish_track_name(true, window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::CancelTrackName, window, cx| {
                view.finish_track_name(false, window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::RenameClip, window, cx| {
                if !view.rename_selected_locator(window, cx)
                    && !view.section_name_begin(window, cx)
                    && !view.track_header_focused(window)
                {
                    view.begin_clip_name(window, cx);
                }
            }))
            .on_action(cx.trace(|view, _: &super::CommitClipName, window, cx| {
                if !view.finish_locator_name(true, window, cx)
                    && !view.section_name_finish(true, window, cx)
                {
                    view.finish_clip_name(true, window, cx);
                }
            }))
            .on_action(cx.trace(|view, _: &super::CancelClipName, window, cx| {
                if !view.finish_locator_name(false, window, cx)
                    && !view.section_name_finish(false, window, cx)
                {
                    view.finish_clip_name(false, window, cx);
                }
            }))
            .on_action(cx.trace(|view, _: &super::ToggleTrackPerformance, _, cx| {
                view.toggle_track_performance();
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::ToggleTrackLatency, _, cx| {
                view.show_track_latency = !view.show_track_latency;
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::ToggleBrowser, window, cx| {
                view.toggle_browser(window, cx);
            }))
            .on_action(
                cx.trace(|view, _: &super::changelog::OpenChangelog, window, cx| {
                    view.open_changelog(window, cx);
                }),
            )
            .on_action(
                cx.trace(|view, _: &super::changelog::CloseChangelog, window, cx| {
                    view.close_changelog(window, cx);
                }),
            )
            .on_action(cx.trace(|view, _: &super::OpenTuning, window, cx| {
                view.open_tuning(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::CloseTuning, window, cx| {
                view.close_tuning(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::CommitTuningName, window, cx| {
                view.commit_tuning_name(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::CancelTuningName, window, cx| {
                view.cancel_tuning_name(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::OpenSettings, window, cx| {
                view.open_theme_settings(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::BrowserPrevious, window, cx| {
                view.browser_previous(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::BrowserNext, window, cx| {
                view.browser_next(window, cx);
            }))
            .on_action(
                cx.trace(|view, _: &super::BrowserRandomSample, window, cx| {
                    view.browser_random_sample(window, cx);
                }),
            )
            .on_action(
                cx.trace(|view, _: &super::BrowserPreviewSelected, window, cx| {
                    view.browser_preview_selected(window, cx);
                }),
            )
            .on_action(
                cx.trace(|view, _: &super::BrowserCollapseFolder, window, cx| {
                    view.browser_collapse_folder(window, cx);
                }),
            )
            .on_action(cx.trace(|view, _: &super::BrowserActivate, window, cx| {
                view.browser_activate(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::BrowserEscape, window, cx| {
                if view.cancel_trim_gesture(cx)
                    | view.cancel_track_resize(cx)
                    | view.cancel_track_reorder()
                {
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if view.cancel_active_browser_audio_drag() {
                    cx.stop_propagation();
                    cx.notify();
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
                cx.trace(|view, action: &super::ThemeSettingsEscape, window, cx| {
                    super::theme_settings::handle_escape_action(view, action, window, cx);
                }),
            )
            .capture_action(
                cx.trace(|view, _: &hex_widgets::NumericEscape, window, cx| {
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
            .on_action(cx.trace(|view, _: &TogglePlay, _, cx| {
                view.toggle_play();
                cx.notify();
            }))
            .on_action(cx.trace(|view, stop: &Stop, window, cx| {
                cx.stop_active_drag(window);
                view.retire_active_clip_drag();
                view.cancel_active_browser_audio_drag();
                view.cancel_active_native_device_drag();
                view.cancel_arrangement_pan();
                view.cancel_track_reorder();
                view.device_panel.navigation.cancel();
                view.cancel_modulation_target_drag();
                if stop.return_to_beginning {
                    view.stop_at_beginning();
                } else {
                    view.stop();
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &CancelDragOrStop, window, cx| {
                if view.cancel_modulation_target_drag() {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if view.pending_plugin_effect.is_some() {
                    view.cancel_plugin_effect();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if view.cancel_eq_creation() {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if view.cancel_bounce() {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                view.cancel_duplicate_chain();
                if super::device_panel::cancel_sampler_boundary(view)
                    | view.cancel_audio_loop_gesture()
                    | view.cancel_loop_rail_gesture()
                    | view.locators.cancel_drag()
                    | view.project_spectrum_splitter_event(
                        hex_widgets::SplitterEvent::Cancel(hex_widgets::SplitterCancel::Escape),
                        window,
                        cx,
                    )
                    | view.clip_detail_splitter_event(
                        hex_widgets::SplitterEvent::Cancel(hex_widgets::SplitterCancel::Escape),
                        window,
                        cx,
                    )
                    | view.cancel_trim_gesture(cx)
                    | view.cancel_track_resize(cx)
                    | view.cancel_track_reorder()
                {
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if view.cancel_active_browser_audio_drag() {
                    cx.stop_propagation();
                    cx.notify();
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
                if view.midi_focus.is_focused(window) {
                    view.cancel_midi_audition();
                }
                let midi_canceled = view.cancel_midi_note_gesture();
                let time_selection_canceled = view.cancel_time_selection_gesture()
                    | view.cancel_clip_paint()
                    | view.cancel_section_pointer();
                let selection_cleared = if !midi_canceled && view.midi_focus.is_focused(window) {
                    view.midi_selection.clear() | view.clear_midi_time_selection()
                } else {
                    false
                };
                let drag_stopped = cx.stop_active_drag(window);
                let device_drag_stopped = view.cancel_active_native_device_drag();
                let pan_canceled =
                    view.cancel_arrangement_pan() | view.device_panel.navigation.cancel();
                if drag_stopped {
                    view.retire_active_clip_drag();
                }
                if !gain_canceled
                    && !midi_canceled
                    && !selection_cleared
                    && !time_selection_canceled
                    && !drag_stopped
                    && !device_drag_stopped
                    && !pan_canceled
                {
                    view.stop_from_escape();
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &SelectAllMidiNotes, _, cx| {
                view.select_all_midi_notes();
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &ToggleMidiDrawMode, _, cx| {
                view.midi_draw_mode = !view.midi_draw_mode;
                view.cancel_midi_note_gesture();
                view.midi_group_gesture = None;
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &ImportAudio, _, cx| view.import_dialog(cx)))
            .on_action(cx.trace(|view, _: &NewProject, window, cx| {
                view.new_project(hex_session::DocumentAudioMode::Resident, window, cx);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &NewStreamingProject, window, cx| {
                view.new_project(
                    hex_session::DocumentAudioMode::VerifiedStreaming,
                    window,
                    cx,
                );
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &OpenProject, window, cx| {
                view.open_dialog(hex_session::DocumentAudioMode::Resident, window, cx)
            }))
            .on_action(cx.trace(|view, _: &OpenStreamingProject, window, cx| {
                view.open_dialog(
                    hex_session::DocumentAudioMode::VerifiedStreaming,
                    window,
                    cx,
                )
            }))
            .on_action(cx.trace(|view, _: &OpenRelinkedProject, window, cx| {
                view.project_recovery_dialog(super::project_recovery::OpenKind::Relink, window, cx)
            }))
            .on_action(cx.trace(|view, _: &RecoverUntitledProject, window, cx| {
                view.project_recovery_dialog(
                    super::project_recovery::OpenKind::Untitled,
                    window,
                    cx,
                )
            }))
            .on_action(cx.trace(|view, _: &RecoverSavedProject, window, cx| {
                view.project_recovery_dialog(
                    super::project_recovery::OpenKind::SavedEdits,
                    window,
                    cx,
                )
            }))
            .on_action(cx.trace(|view, _: &OpenAutomationRecovery, _, cx| {
                view.open_automation_recovery_file(cx)
            }))
            .on_action(cx.trace(|view, _: &RecoverKeptSession, window, cx| {
                view.project_recovery_dialog(super::project_recovery::OpenKind::Kept, window, cx)
            }))
            .on_action(cx.trace(|view, _: &RecoverPreviousSave, window, cx| {
                view.project_recovery_dialog(
                    super::project_recovery::OpenKind::PreviousSave,
                    window,
                    cx,
                )
            }))
            .on_action(cx.trace(|view, action: &OpenRecentProject, _, cx| {
                if super::device_panel::finalize_controls(view, cx) {
                    view.open_recent_project(action);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &SaveProject, _, cx| {
                view.save_project(cx);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &SaveAsProject, _, cx| {
                view.save_as_dialog(cx);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &Undo, _, cx| {
                view.history_action(SessionRequest::Undo, cx);
            }))
            .on_action(cx.trace(|view, _: &Redo, _, cx| {
                view.history_action(SessionRequest::Redo, cx);
            }))
            .on_action(cx.trace(|view, _: &MoveLeft, window, cx| {
                view.cancel_duplicate_chain();
                match super::session_navigation::arrow_target(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    super::session_navigation::ArrowTarget::Piano => {
                        view.navigate_piano_cursor(false);
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
            .on_action(cx.trace(|view, _: &MoveRight, window, cx| {
                view.cancel_duplicate_chain();
                match super::session_navigation::arrow_target(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    super::session_navigation::ArrowTarget::Piano => {
                        view.navigate_piano_cursor(true);
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
            .on_action(cx.trace(|view, _: &TrimLeft, window, cx| {
                view.cancel_duplicate_chain();
                if view.midi_focus.is_focused(window) {
                    if view.midi_selection.len() == 1 {
                        if let Some(note) = view.midi_selection.primary()
                            && let Some(state) = view.state.as_ref()
                        {
                            view.update_note_field(
                                note,
                                state.revision,
                                view.project_generation,
                                super::midi::NoteField::Duration(-1),
                            );
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
            .on_action(cx.trace(|view, _: &TrimRight, window, cx| {
                view.cancel_duplicate_chain();
                if view.midi_focus.is_focused(window) {
                    if view.midi_selection.len() == 1 {
                        if let Some(note) = view.midi_selection.primary()
                            && let Some(state) = view.state.as_ref()
                        {
                            view.update_note_field(
                                note,
                                state.revision,
                                view.project_generation,
                                super::midi::NoteField::Duration(1),
                            );
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
            .on_action(cx.trace(|view, _: &super::CoarserGrid, _, cx| {
                view.step_adaptive_grid_density(true);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::FinerGrid, _, cx| {
                view.step_adaptive_grid_density(false);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::ToggleSnapGrid, _, cx| {
                view.toggle_snap_grid();
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &CopyTimeSelection, window, cx| {
                if !view.edit_automation_range(RangeEdit::Copy, window, cx)
                    && !view.copy_section(window)
                {
                    view.copy_time_selection(false, window);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &CutTimeSelection, window, cx| {
                if !view.edit_automation_range(RangeEdit::Cut, window, cx) {
                    view.copy_time_selection(true, window);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &PasteTimeSelection, window, cx| {
                if !view.edit_automation_range(RangeEdit::Paste, window, cx)
                    && !view.paste_section(window)
                {
                    view.paste_time_selection(window);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &InsertTimeSelection, window, cx| {
                if !view.edit_automation_range(RangeEdit::Insert, window, cx) {
                    view.insert_time_selection(window);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &DeleteSectionMarker, window, cx| {
                if !view.delete_section_kind(true, window) {
                    cx.propagate();
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &GroupSelection, window, cx| {
                if crate::diagnostics::actions_enabled(cx) {
                    crate::diagnostics::record_group_selection(
                        cx,
                        &view.group_selection_route(window),
                    );
                }
                if view.audio_event_focus.is_focused(window) {
                    view.event_group_selected();
                } else if view.section_has_focus(window) {
                    if !view.merge_sections(window) {
                        cx.propagate();
                    }
                } else if matches!(
                    view.focused_header_target(window),
                    Some(super::track_name::Target::Track(_))
                ) && !view.computer_keyboard.accepts_key("g")
                {
                    if let Some(tracks) = view.track_membership(None)
                        && let Some(revision) = view.state.as_ref().map(|state| state.revision)
                    {
                        view.perform_track_group(
                            view.project_generation,
                            revision,
                            Command::GroupTracks {
                                tracks,
                                name: "group".into(),
                            },
                            window,
                            cx,
                        );
                    }
                } else if view.mode == super::EditorMode::Arrangement
                    && view.focus.is_focused(window)
                {
                    view.group_selected_clips(window);
                } else {
                    cx.propagate();
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &DeleteClip, window, cx| {
                if view.selection_delete_blocked() {
                    return;
                }
                if view.delete_locator(window) {
                    cx.notify();
                    return;
                }
                if view.delete_automation_point(window, cx) {
                    return;
                }
                if view.delete_section(window) {
                    cx.notify();
                    return;
                }
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
                    // A focused track header owns Delete; a clip selection
                    // retained beside it is never its target.
                    Some(super::session_grid::DeleteTarget::ArrangementClip(_))
                        if view.track_header_focused(window) => {}
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
                cx.trace(|view, _: &super::LaunchSessionSelection, window, cx| {
                    if super::session_navigation::session_editor_has_focus(
                        view.mode,
                        view.midi_focus.is_focused(window),
                    ) {
                        view.launch_session_selection();
                    }
                    cx.notify();
                }),
            )
            .on_action(cx.trace(|view, _: &DuplicateClip, window, cx| {
                view.duplicate_key(super::duplicate::DuplicateKind::Independent, window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::DuplicateAliasClip, window, cx| {
                view.duplicate_key(super::duplicate::DuplicateKind::Alias, window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::AddLocator, window, cx| {
                view.add_locator(window);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::PreviousLocator, window, cx| {
                view.navigate_locator(false, window);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::NextLocator, window, cx| {
                view.navigate_locator(true, window);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::MakeClipUnique, window, cx| {
                if view.focus.is_focused(window)
                    && !view.track_header_focused(window)
                    && !view.selection_intent_epoch.is_some_and(|epoch| {
                        view.track_clipboard_context == Some((view.project_generation, epoch))
                    })
                    && !view.computer_keyboard.accepts_key("i")
                    && !window.context_stack().iter().any(|context| {
                        context.contains("hex_text_editing")
                            || context.contains("hex_audio_events")
                            || context.contains("hex_midi")
                            || context.contains("hex_device_selection")
                    })
                {
                    view.make_selected_clips_unique();
                    cx.notify();
                }
            }))
            .on_action(cx.trace(|view, _: &super::MoveSceneUp, window, cx| {
                if super::session_navigation::session_editor_has_focus(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    view.move_selected_scene(super::session_navigation::Direction::Up);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::MoveSceneDown, window, cx| {
                if super::session_navigation::session_editor_has_focus(
                    view.mode,
                    view.midi_focus.is_focused(window),
                ) {
                    view.move_selected_scene(super::session_navigation::Direction::Down);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &SplitClip, window, cx| {
                if !view.split_section_at_cursor(window) {
                    view.split_arrangement_clips(window, cx);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &ExportProject, window, cx| {
                view.export_dialog(window, cx);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::BounceToNewTracks, window, cx| {
                if !view.arm_drag_bounce() {
                    view.bounce_arrangement_selection(window, false, false);
                }
                cx.notify();
            }))
            .on_action(
                cx.trace(|view, _: &super::BounceAndMuteSources, window, cx| {
                    view.bounce_arrangement_selection(window, true, false);
                    cx.notify();
                }),
            )
            .on_action(cx.trace(|view, _: &super::BounceMixedRange, window, cx| {
                view.bounce_arrangement_selection(window, false, true);
                cx.notify();
            }))
            .on_action(
                cx.trace(|view, _: &super::BounceMixedRangeAndMute, window, cx| {
                    view.bounce_arrangement_selection(window, true, true);
                    cx.notify();
                }),
            )
            .on_action(cx.trace(|view, _: &LoopSelection, window, cx| {
                view.loop_selection_key(window, cx);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &ToggleLoop, window, cx| {
                view.toggle_active_loop_shortcut(window, cx);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &ZoomIn, _, cx| {
                view.zoom_timeline(true);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &ZoomOut, _, cx| {
                view.zoom_timeline(false);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::ZoomToFit, window, cx| {
                if view.zoom_fit_key(false, window, cx) {
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.trace(|view, _: &super::ZoomBack, window, cx| {
                if view.zoom_fit_key(true, window, cx) {
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.trace(|view, _: &FitTimeline, _, cx| {
                view.fit_timeline();
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &PanTimelineLeft, _, cx| {
                view.pan_timeline(false);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &PanTimelineRight, _, cx| {
                view.pan_timeline(true);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &ToggleSolo, window, cx| {
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
            .on_action(cx.trace(|view, _: &AdditiveSolo, window, cx| {
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
            .on_action(cx.trace(|view, _: &PreviousTrack, window, cx| {
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
            .on_action(cx.trace(|view, _: &NextTrack, window, cx| {
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
            .on_action(cx.trace(|view, _: &MidiOctaveUp, window, cx| {
                if view.midi_focus.is_focused(window) {
                    view.transpose_selected_note_by(12);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &MidiOctaveDown, window, cx| {
                if view.midi_focus.is_focused(window) {
                    view.transpose_selected_note_by(-12);
                }
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &AddTrack, _, cx| {
                view.add_track();
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &AddMidiTrack, _, cx| {
                view.add_midi_track();
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &AddMidiClip, _, cx| {
                view.add_midi_clip();
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &RemoveTrack, _, cx| {
                view.remove_track(cx);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &super::RemoveHeaderTracks, window, cx| {
                view.remove_header_tracks(window, cx);
                cx.notify();
            }))
            .on_action(cx.trace(|view, _: &Quit, window, cx| {
                view.request_quit(window, cx);
            }))
            .on_action(cx.trace(|view, _: &super::InstallUpdateOnQuit, _, cx| {
                view.select_update(crate::UpdateAction::InstallOnQuit, cx);
            }))
            .on_action(cx.trace(|view, _: &super::SaveAndRestartForUpdate, _, cx| {
                view.select_update(crate::UpdateAction::SaveAndRestart, cx);
            }))
            .on_action(cx.trace(|view, _: &super::CancelUpdate, _, cx| {
                view.cancel_requested_update(cx);
            }))
            .child(self.transport(window, cx))
            .child(workspace)
            .child(self.status_bar(cx))
            .children(self.return_send_menu_overlay(window, cx))
            .children(self.floating_browser_audio_drag_ghost(window.scale_factor()))
            .children(self.floating_clip_drag_ghost(window.scale_factor()))
            .children(self.context_menu_overlay(window, cx))
            .children(self.controller_input_overlay(cx))
            .children(super::theme_settings::overlay(self, cx))
            .children(self.tuning_overlay(cx))
            .children(self.changelog_overlay(cx))
            .children(self.missing_media_overlay(cx))
            .children(super::device_panel::picker_overlay(self, window, cx))
            .when(self.update_preparing(), |root| {
                root.child(
                    div()
                        .id("update-preparing-overlay")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgb(theme::color(theme::PaletteRole::Background)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(theme::text("preparing update"))
                                .child(
                                    div()
                                        .id("cancel-update-preparation")
                                        .cursor_pointer()
                                        .child(theme::text("cancel"))
                                        .on_click(cx.listener(|view, _, _, cx| {
                                            view.cancel_requested_update(cx)
                                        })),
                                ),
                        ),
                )
            })
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};

    #[gpui::test]
    fn redo_cancels_gain_draft_without_submitting_history(cx: &mut TestAppContext) {
        let (handle, entity, _) = crate::editor::gain_lane::tests::native::window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-point-value-value").unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-shift-z");
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            assert!(view.gain_lane.draft.is_none());
            assert!(!view.gain_numeric_editing(cx));
            assert_eq!(view.worker_requests_pending, 0);
        });
    }
}
