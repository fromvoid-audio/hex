//! View-only Arrangement insertion position and scoped canonical split dispatch.
//! Selection normalization stays with time_selection; project/session own cuts and history.
//! Empty Arrangement space places a project-level point. Track, clip and Main
//! selections stay intact, and the previous time range is cleared. The point
//! has no track edit target and supplies Play's start until the next selection.
use super::{EditorMode, EditorView, time_selection::TimeSelection, viewport::Viewport};
use crate::theme;
use gpui::{Bounds, Context, Div, Modifiers, Pixels, Point, Window, canvas, div, prelude::*, px};
use hex_project::{ClipId, Command, TrackId};
use hex_session::service::Request;
use std::time::{Duration, Instant};

const PULSE_PERIOD: Duration = Duration::from_millis(1_200);
// Quantize both cursor pulses to 100 ms steps (10 Hz) matching `IDLE_INTERVAL`
// so idle pulsing advances smoothly without 60 Hz wakeups or full-window renders.
const PULSE_STEP_MS: u128 = 100;
const LOCK_PULSE_PERIOD_MS: u128 = 2_400;
const LOCK_PULSE_STEP_MS: u128 = 100;
pub(super) const POINT_CLICK_SLOP_PX: f64 = 2.0;
const LOCKED_DRAG_THRESHOLD_PX: f32 = 3.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct EditCursorTick {
    pub(super) structural_changed: bool,
    pub(super) pulse_changed: bool,
}

#[derive(Clone, Copy)]
pub(super) struct EditCursor {
    pub(super) track: Option<TrackId>,
    pub(super) frame: u64,
    pub(super) generation: u64,
    pub(super) pulse_color: u32,
    pub(super) pulse_started: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockedPlayStartDragKind {
    Marker { allow_unlock_on_click: bool },
    RulerPlace { started_at_beginning: bool },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LockedPlayStartDrag {
    generation: u64,
    viewport: Viewport,
    timeline_left: f32,
    timeline_width: f32,
    pointer_down: Point<Pixels>,
    last_pointer: Point<Pixels>,
    preview_frame: u64,
    pulse_started: Instant,
    moved: bool,
    click_disqualified: bool,
    kind: LockedPlayStartDragKind,
}

pub(super) struct DropCursor {
    pub(super) track: TrackId,
    pub(super) frame: u64,
    pub(super) generation: u64,
    pub(super) epoch: u64,
}

impl EditCursor {
    fn new(track: Option<TrackId>, frame: u64, generation: u64) -> Self {
        Self {
            track,
            frame,
            generation,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: Instant::now(),
        }
    }

    fn is_current(self, generation: u64, project: &hex_project::Project) -> bool {
        self.generation == generation
            && self
                .track
                .is_none_or(|track| project.track(track).is_some())
    }

    fn pulse(&mut self, now: Instant) -> bool {
        let elapsed = now
            .saturating_duration_since(self.pulse_started)
            .as_millis();
        let step = (elapsed % PULSE_PERIOD.as_millis()) / PULSE_STEP_MS;
        let phase = (step * PULSE_STEP_MS) as f64 / PULSE_PERIOD.as_millis() as f64;
        let blend = (1.0 - (phase * std::f64::consts::TAU).cos()) * 0.5;
        let mut color = 0;
        for shift in [16, 8, 0] {
            let white = f64::from((theme::color(theme::PaletteRole::Text) >> shift) & 0xff);
            let cyan = f64::from((theme::color(theme::PaletteRole::Secondary) >> shift) & 0xff);
            let channel = (white + (cyan - white) * blend).round() as u32;
            color |= channel << shift;
        }
        if color == self.pulse_color {
            return false;
        }
        self.pulse_color = color;
        true
    }

    pub(super) fn color(&self) -> u32 {
        self.pulse_color
    }

    fn pulse_locked_marker(&mut self, now: Instant) -> bool {
        let elapsed = now
            .saturating_duration_since(self.pulse_started)
            .as_millis();
        let step = (elapsed % LOCK_PULSE_PERIOD_MS) / LOCK_PULSE_STEP_MS;
        let phase = (step * LOCK_PULSE_STEP_MS) as f32 / LOCK_PULSE_PERIOD_MS as f32;
        let blend = (1.0 - (phase * std::f32::consts::TAU).cos()) * 0.5;
        let color = theme::locked_time_position_marker_color(blend);
        let changed = self.pulse_color != color;
        self.pulse_color = color;
        changed
    }
}

impl EditorView {
    pub(super) fn valid_locked_play_start(&self) -> Option<EditCursor> {
        if let Some(drag) = self.interaction.locked_play_start_drag
            && matches!(drag.kind, LockedPlayStartDragKind::RulerPlace { .. })
            && self.locked_play_start_drag_valid(drag)
        {
            let mut cursor = self
                .interaction
                .locked_play_start
                .filter(|cursor| self.cursor_is_current(*cursor))
                .unwrap_or_else(|| EditCursor {
                    track: None,
                    frame: drag.preview_frame,
                    generation: drag.generation,
                    pulse_color: theme::locked_time_position_marker_color(0.0),
                    pulse_started: drag.pulse_started,
                });
            cursor.track = None;
            cursor.frame = drag.preview_frame;
            return Some(cursor);
        }
        self.interaction
            .locked_play_start
            .filter(|cursor| self.cursor_is_current(*cursor))
            .map(|mut cursor| {
                if let Some(drag) = self.interaction.locked_play_start_drag
                    && drag.moved
                    && matches!(drag.kind, LockedPlayStartDragKind::Marker { .. })
                    && self.locked_play_start_drag_valid(drag)
                {
                    cursor.frame = drag.preview_frame;
                }
                cursor
            })
    }

    pub(super) fn locked_play_start_drag_active(&self) -> bool {
        self.interaction.locked_play_start_drag.is_some()
    }

    fn ruler_locked_play_start_drag_active(&self) -> bool {
        self.interaction.locked_play_start_drag.is_some_and(|drag| {
            matches!(drag.kind, LockedPlayStartDragKind::RulerPlace { .. })
                && self.locked_play_start_drag_valid(drag)
        })
    }

    pub(super) fn cancel_locked_play_start_drag(&mut self) -> bool {
        self.interaction.locked_play_start_drag.take().is_some()
    }

    pub(super) fn cancel_locked_play_start_drag_on_ruler_change(
        &mut self,
        bounds: Bounds<Pixels>,
    ) -> bool {
        if self.interaction.locked_play_start_drag.is_some_and(|drag| {
            drag.timeline_left != f32::from(bounds.origin.x)
                || drag.timeline_width != f32::from(bounds.size.width)
        }) {
            self.interaction.locked_play_start_drag = None;
            return true;
        }
        false
    }

    fn locked_play_start_drag_modifiers(modifiers: Modifiers) -> bool {
        !modifiers.shift && !modifiers.alt && !modifiers.control && !modifiers.function
    }

    fn ruler_locked_play_start_modifiers(modifiers: Modifiers) -> bool {
        modifiers.alt && !modifiers.shift && !modifiers.control && !modifiers.function
    }

    fn play_start_interaction_available(&self, cx: &Context<Self>) -> bool {
        self.mode == EditorMode::Arrangement
            && self.context_menu.is_none()
            && !super::device_panel::picker_is_open(self)
            && !super::device_panel::source_naming(self)
            && !(self.audio_detail.clip_view_open
                && self.device_panel.active
                && self.device_panel.add_menu_is_open(self.state.as_deref()))
            && self.session_grid.send_menu_track.is_none()
            && !self.track_name.is_editing()
            && self.clip_name_edit.is_none()
            && !self.sections.naming()
            && !self.sections.held()
            && !self.locators.naming()
            && !self.settings.changelog.is_open()
            && !self.project_tempo_editing(cx)
            && !self.gain_numeric_editing(cx)
            && !self.clip_numeric_editing(cx)
            && !self.device_panel.sampler_parameter.numeric_draft_active()
            && !self.track_gain_controls.has_pending_draft(cx)
            && super::device_panel::pending_send(self, cx).is_none()
            && !self.persistence.opening_project
            && !self.persistence.project_open.busy()
            && self.persistence.pending_open_path.is_none()
            && !self.persistence.close_prompt_open
            && !self.persistence.save_dialog_open
            && !self.persistence.export_dialog_open
            && !self.device_panel.preset_dialog_open
            && !self.settings.theme.open
            && !self.settings.tuning.open
            && !self.controller_input_is_open()
            && self.settings.missing_media.is_none()
            && !self.update_preparing()
            && self.interaction.pointer_down.is_none()
            && !self.gain_lane.pointer_is_active()
            && !self.automation_view.devices.pointer_active()
            && !self.device_panel.project_spectrum.resize_is_active()
            && !self.audio_detail.detail_resize.is_active()
            && !self.device_panel.navigation.is_active()
            && self.interaction.time_selection_gesture.is_none()
            && self.interaction.time_range_move_gesture.is_none()
            && self.interaction.arrangement_pan.is_none()
            && self.interaction.active_clip_drag.is_none()
            && self.browser.active_audio_drag.is_none()
            && self.device_panel.active_native_drag.is_none()
            && self.interaction.active_trim_drag.is_none()
            && self.midi_editor.active_trim_drag.is_none()
            && self.interaction.active_fade_drag.is_none()
            && self.interaction.clip_paint_gesture.is_none()
            && self.interaction.loop_rail_gesture.is_none()
            && !self.locked_play_start_drag_active()
            && !self.scrub_active()
            && !self.track_resize_active()
            && !self.track_reorder_active()
            && !cx.has_active_drag()
    }

    fn play_start_marker_available(&self, cx: &Context<Self>) -> bool {
        !self.interaction.play_start_at_beginning && self.play_start_interaction_available(cx)
    }

    fn locked_play_start_drag_valid(&self, drag: LockedPlayStartDrag) -> bool {
        let kind_valid = match drag.kind {
            LockedPlayStartDragKind::Marker { .. } => {
                !self.interaction.play_start_at_beginning
                    && self
                        .interaction
                        .locked_play_start
                        .is_some_and(|cursor| self.cursor_is_current(cursor))
            }
            LockedPlayStartDragKind::RulerPlace {
                started_at_beginning,
            } => self.interaction.play_start_at_beginning == started_at_beginning,
        };
        kind_valid
            && self.mode == EditorMode::Arrangement
            && self.context_menu.is_none()
            && !super::device_panel::picker_is_open(self)
            && !super::device_panel::source_naming(self)
            && !(self.audio_detail.clip_view_open
                && self.device_panel.active
                && self.device_panel.add_menu_is_open(self.state.as_deref()))
            && self.session_grid.send_menu_track.is_none()
            && !self.locators.naming()
            && !self.sections.held()
            && !self.persistence.opening_project
            && !self.persistence.project_open.busy()
            && self.persistence.pending_open_path.is_none()
            && !self.persistence.close_prompt_open
            && !self.persistence.save_dialog_open
            && !self.persistence.export_dialog_open
            && !self.device_panel.preset_dialog_open
            && !self.settings.theme.open
            && !self.settings.tuning.open
            && !self.settings.changelog.is_open()
            && !self.controller_input_is_open()
            && self.settings.missing_media.is_none()
            && !self.update_preparing()
            && drag.generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| self.visible_viewport(state.timeline_frames) == drag.viewport)
    }

    fn locked_play_start_drag_frame(
        &self,
        pointer_x: f32,
        timeline_left: f32,
        timeline_width: f32,
        viewport: Viewport,
        free: bool,
    ) -> Option<u64> {
        let state = self.state.as_ref()?;
        let viewport_end = viewport.end()?;
        let candidate =
            super::seek::pointer_to_frame(pointer_x, timeline_left, timeline_width, viewport)?;
        let frame = self.snap_grid.with_platform_modifier(free).snap_frame(
            &state.project,
            candidate,
            viewport,
            timeline_width,
        )?;
        Some(frame.clamp(viewport.start, viewport_end))
    }

    pub(super) fn begin_ruler_locked_play_start_drag(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        viewport: Viewport,
        bounds: Bounds<Pixels>,
        cx: &Context<Self>,
    ) -> bool {
        if !self.play_start_interaction_available(cx)
            || !Self::ruler_locked_play_start_modifiers(modifiers)
            || !f32::from(position.x).is_finite()
            || !f32::from(position.y).is_finite()
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.visible_viewport(state.timeline_frames) != viewport
            || self.timeline_ruler_bounds() != Some(bounds)
        {
            return false;
        }
        let timeline_left = f32::from(bounds.origin.x);
        let timeline_width = f32::from(bounds.size.width);
        if !timeline_left.is_finite() || !timeline_width.is_finite() || timeline_width <= 0.0 {
            return false;
        }
        let Some(preview_frame) = self.locked_play_start_drag_frame(
            f32::from(position.x),
            timeline_left,
            timeline_width,
            viewport,
            modifiers.platform,
        ) else {
            return false;
        };
        self.interaction.locked_play_start_drag = Some(LockedPlayStartDrag {
            generation: self.project_generation,
            viewport,
            timeline_left,
            timeline_width,
            pointer_down: position,
            last_pointer: position,
            preview_frame,
            pulse_started: Instant::now(),
            moved: true,
            click_disqualified: true,
            kind: LockedPlayStartDragKind::RulerPlace {
                started_at_beginning: self.interaction.play_start_at_beginning,
            },
        });
        true
    }

    fn begin_locked_play_start_drag(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &Context<Self>,
    ) -> bool {
        if !self.play_start_marker_available(cx)
            || !Self::locked_play_start_drag_modifiers(modifiers)
            || !f32::from(position.x).is_finite()
            || !f32::from(position.y).is_finite()
        {
            return false;
        }
        let Some(locked) = self
            .interaction
            .locked_play_start
            .filter(|cursor| self.cursor_is_current(*cursor))
        else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(bounds) = self.timeline_ruler_bounds() else {
            return false;
        };
        let timeline_left = f32::from(bounds.origin.x);
        let timeline_width = f32::from(bounds.size.width);
        if !timeline_left.is_finite() || !timeline_width.is_finite() || timeline_width <= 0.0 {
            return false;
        }
        let viewport = self.visible_viewport(state.timeline_frames);
        self.interaction.locked_play_start_drag = Some(LockedPlayStartDrag {
            generation: self.project_generation,
            viewport,
            timeline_left,
            timeline_width,
            pointer_down: position,
            last_pointer: position,
            preview_frame: locked.frame,
            pulse_started: locked.pulse_started,
            moved: false,
            click_disqualified: false,
            kind: LockedPlayStartDragKind::Marker {
                allow_unlock_on_click: modifiers == Modifiers::default(),
            },
        });
        true
    }

    pub(super) fn update_locked_play_start_drag(
        &mut self,
        position: Point<Pixels>,
        free: bool,
    ) -> bool {
        let Some(mut drag) = self.interaction.locked_play_start_drag else {
            return false;
        };
        if !self.locked_play_start_drag_valid(drag)
            || !f32::from(position.x).is_finite()
            || !f32::from(position.y).is_finite()
        {
            self.interaction.locked_play_start_drag = None;
            return true;
        }
        let delta_x = f32::from(position.x - drag.pointer_down.x);
        let delta_y = f32::from(position.y - drag.pointer_down.y);
        let was_moved = drag.moved;
        let previous_preview = drag.preview_frame;
        drag.last_pointer = position;
        drag.moved |= delta_x.abs() >= LOCKED_DRAG_THRESHOLD_PX;
        drag.click_disqualified |= drag.moved || delta_y.abs() >= LOCKED_DRAG_THRESHOLD_PX;
        if drag.moved
            && let Some(frame) = self.locked_play_start_drag_frame(
                f32::from(position.x),
                drag.timeline_left,
                drag.timeline_width,
                drag.viewport,
                free,
            )
        {
            drag.preview_frame = frame;
        }
        self.interaction.locked_play_start_drag = Some(drag);
        drag.moved != was_moved || drag.preview_frame != previous_preview
    }

    pub(super) fn refresh_locked_play_start_drag_modifiers(
        &mut self,
        modifiers: Modifiers,
    ) -> bool {
        let Some(drag) = self.interaction.locked_play_start_drag else {
            return false;
        };
        if !drag.moved {
            return false;
        }
        self.update_locked_play_start_drag(drag.last_pointer, modifiers.platform)
    }

    pub(super) fn finish_locked_play_start_drag(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> bool {
        if self.interaction.locked_play_start_drag.is_none() {
            return false;
        }
        let _ = self.update_locked_play_start_drag(position, modifiers.platform);
        let Some(drag) = self.interaction.locked_play_start_drag.take() else {
            return true;
        };
        if !self.locked_play_start_drag_valid(drag) {
            return true;
        }
        match drag.kind {
            LockedPlayStartDragKind::RulerPlace { .. } => {
                self.interaction.play_start_at_beginning = false;
                self.interaction.escape_stop_armed = false;
                if let Some(locked) = self
                    .interaction
                    .locked_play_start
                    .filter(|cursor| self.cursor_is_current(*cursor))
                {
                    self.interaction.locked_play_start = Some(EditCursor {
                        track: None,
                        frame: drag.preview_frame,
                        ..locked
                    });
                } else {
                    self.interaction.locked_play_start = Some(EditCursor {
                        track: None,
                        frame: drag.preview_frame,
                        generation: self.project_generation,
                        pulse_color: theme::locked_time_position_marker_color(0.0),
                        pulse_started: drag.pulse_started,
                    });
                }
                true
            }
            LockedPlayStartDragKind::Marker {
                allow_unlock_on_click,
            } => {
                if drag.moved {
                    if let Some(locked) = self.interaction.locked_play_start.as_mut() {
                        locked.track = None;
                        locked.frame = drag.preview_frame;
                        self.interaction.escape_stop_armed = false;
                        return true;
                    }
                    return false;
                }
                if !drag.click_disqualified
                    && allow_unlock_on_click
                    && modifiers == Modifiers::default()
                {
                    self.interaction.locked_play_start = None;
                    return true;
                }
                false
            }
        }
    }

    pub(super) fn pointer_hits_bottom_play_start_marker(
        &self,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> bool {
        if self.mode != EditorMode::Arrangement || self.interaction.play_start_at_beginning {
            return false;
        }
        let modifiers_ok = if self.valid_locked_play_start().is_some() {
            Self::locked_play_start_drag_modifiers(modifiers)
        } else {
            modifiers == Modifiers::default()
        };
        modifiers_ok
            && self
                .interaction
                .bottom_play_start_marker_bounds
                .get()
                .is_some_and(|bounds| bounds.contains(&position))
    }

    fn cursor_is_current(&self, cursor: EditCursor) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| cursor.is_current(self.project_generation, &state.project))
    }

    pub(super) fn project_point_cursor(&self) -> Option<EditCursor> {
        self.interaction
            .edit_cursor
            .filter(|cursor| cursor.track.is_none() && self.cursor_is_current(*cursor))
    }

    fn lock_play_start(&mut self, clip_editor_start: Option<u64>, cx: &mut Context<Self>) -> bool {
        if !self.play_start_marker_available(cx) {
            return false;
        }
        if clip_editor_start.is_some() || self.normalized_time_selection().is_none() {
            // Pin the active transport frame without editing the clip.
            let clip_start = clip_editor_start.or_else(|| {
                self.selected_clip
                    .and_then(|_| self.selected_arrangement_play_start())
                    .map(|_| self.playback_start_frame())
            });
            let point = clip_start
                .map(|frame| EditCursor::new(None, frame, self.project_generation))
                .or_else(|| {
                    self.interaction
                        .edit_cursor
                        .filter(|cursor| self.cursor_is_current(*cursor))
                });
            self.interaction.locked_play_start = point.map(|point| EditCursor {
                pulse_color: theme::locked_time_position_marker_color(0.0),
                pulse_started: Instant::now(),
                ..point
            });
        }
        cx.notify();
        true
    }

    pub(super) fn place_edit_cursor(
        &mut self,
        track: TrackId,
        pointer_x: f32,
        free: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.cursor_click_available() {
            return;
        }
        let Some(lane) = self.timeline_caches.drag_lanes.get(&track).copied() else {
            return;
        };
        if self
            .state
            .as_ref()
            .is_none_or(|state| state.project.track(track).is_none())
        {
            return;
        }
        let Some(frame) = self.cursor_frame_at(pointer_x, lane, free) else {
            return;
        };
        self.clear_automation_range();
        self.set_point_cursor(Some(track), frame);
        self.interaction.time_selection = Some(TimeSelection {
            anchor_track: Some(track),
            end_track: Some(track),
            anchor_frame: frame,
            end_frame: frame,
        });
        self.interaction.time_selection_project_scope = false;
        let under_pointer = self.arrangement_clip_at(track, frame, self.selected_clip);
        let selected_clip_under_pointer =
            under_pointer.is_some() && under_pointer == self.selected_clip;
        self.select_lane_track(track);
        self.interaction.edit_cursor_in_selected_clip =
            self.selected_clip.filter(|_| selected_clip_under_pointer);
        if under_pointer.is_none() {
            self.focus_track_details(window);
        } else if !selected_clip_under_pointer {
            // A cursor inside a clip keeps it in the clip editor (see detail_context).
            self.set_detail_context(under_pointer);
            self.focus_detail_context(window, cx);
        }
        self.focus.focus(window);
        cx.notify();
    }

    fn cursor_click_available(&mut self) -> bool {
        if std::mem::take(&mut self.interaction.suppress_cursor_click) {
            return false;
        }
        self.context_menu.is_none()
            && !self.persistence.opening_project
            && !self.persistence.close_prompt_open
            && self.mode == EditorMode::Arrangement
    }

    pub(super) fn cursor_frame_at(
        &self,
        pointer_x: f32,
        lane: super::drag::LaneBounds,
        free: bool,
    ) -> Option<u64> {
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        let frame = super::seek::pointer_to_frame(pointer_x, lane.left, lane.width, viewport)?;
        self.snap_grid.with_platform_modifier(free).snap_frame(
            &state.project,
            frame,
            viewport,
            lane.width,
        )
    }

    fn set_point_cursor(&mut self, track: Option<TrackId>, frame: u64) {
        self.advance_selection_intent();
        self.interaction.edit_cursor = Some(EditCursor::new(track, frame, self.project_generation));
        self.interaction.loop_start_over_clip = None;
    }

    /// Make a locator the next stopped play start using ordinary point-cursor precedence.
    pub(super) fn place_locator_play_start(&mut self, frame: u64) {
        self.set_point_cursor(None, frame);
        self.interaction.edit_cursor_in_selected_clip = None;
        self.interaction.time_selection = None;
        self.interaction.time_selection_project_scope = false;
    }

    /// A held or released drag only paints a point. Commit its placement in
    /// the same receive as the canonical destination, so refusal or Escape
    /// naturally restores the original cursor. A later choice or lock wins.
    pub(super) fn install_drop_cursor(&mut self, cursor: DropCursor, moved_clip: Option<ClipId>) {
        if self.mode != EditorMode::Arrangement
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.project_generation != cursor.generation
            || self.interaction.selection_intent_epoch != Some(cursor.epoch)
            || self
                .state
                .as_ref()
                .is_none_or(|s| s.project.track(cursor.track).is_none())
        {
            return;
        }
        self.advance_selection_intent();
        if let Some(clip) = moved_clip
            && !self.clip_is_selected(clip)
        {
            self.select_arrangement_clip(cursor.track, clip);
        }
        self.interaction.edit_cursor = Some(EditCursor::new(
            Some(cursor.track),
            cursor.frame,
            cursor.generation,
        ));
        self.interaction.time_selection = Some(TimeSelection {
            anchor_track: Some(cursor.track),
            end_track: Some(cursor.track),
            anchor_frame: cursor.frame,
            end_frame: cursor.frame,
        });
        self.interaction.time_selection_project_scope = false;
        // The grabbed clip's point also supersedes whole-rectangle selection.
        self.interaction.edit_cursor_in_selected_clip = self.selected_clip;
        self.interaction.clip_drop_cursor_epoch = self.interaction.selection_intent_epoch;
        self.interaction.loop_start_over_clip = None;
    }

    pub(super) fn place_empty_arrangement_cursor(
        &mut self,
        event: &gpui::MouseClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = event.down.position;
        let end = event.up.position;
        let Some(lane) = self.painted_arrangement_tail(f32::from(end.x), f32::from(end.y)) else {
            return;
        };
        let plain = |mut modifiers: gpui::Modifiers| {
            modifiers.platform = false;
            modifiers == gpui::Modifiers::default()
        };
        if event.down.click_count != 1
            || !plain(event.down.modifiers)
            || !plain(event.up.modifiers)
            || (end - start).magnitude() > POINT_CLICK_SLOP_PX
            || self
                .painted_arrangement_tail(f32::from(start.x), f32::from(start.y))
                .is_none()
            || !self.cursor_click_available()
            || self.interaction.time_selection_key_held
            || self.interaction.time_selection_gesture.is_some()
            || self.interaction.time_range_move_gesture.is_some()
            || self.interaction.arrangement_pan.is_some()
            || self.sections.held()
            || self.interaction.loop_rail_gesture.is_some()
            || self.locked_play_start_drag_active()
            || self.scrub_active()
            || self.track_resize_active()
            || self.track_reorder_active()
            || self.interaction.active_clip_drag.is_some()
            || self.browser.active_audio_drag.is_some()
            || self.device_panel.active_native_drag.is_some()
            || self.interaction.active_trim_drag.is_some()
            || self.midi_editor.active_trim_drag.is_some()
            || self.interaction.active_fade_drag.is_some()
            || self.interaction.clip_paint_gesture.is_some()
            || cx.has_active_drag()
            || super::device_panel::picker_is_open(self)
        {
            return;
        }
        let Some(frame) = self.cursor_frame_at(f32::from(end.x), lane, event.up.modifiers.platform)
        else {
            return;
        };
        self.clear_automation_range();
        self.set_point_cursor(None, frame);
        self.interaction.edit_cursor_in_selected_clip = None;
        self.interaction.time_selection = None;
        self.interaction.time_selection_project_scope = false;
        self.focus.focus(window);
        cx.notify();
    }

    /// An acknowledged loop enable hands the next Play start to the loop start
    /// by retiring a placed point cursor; a new click overrides it again. Ranges
    /// keep precedence, and disabling the loop never restores the cursor.
    pub(super) fn retire_point_cursor_on_loop_enable(&mut self, loop_was_enabled: bool) {
        let loop_enabled = self
            .state
            .as_ref()
            .and_then(|state| state.project.loop_region())
            .is_some_and(|region| region.enabled());
        if loop_was_enabled
            || !loop_enabled
            || self.pending_selection_loop_has_newer_intent()
            || self.interaction.edit_cursor.is_none()
            || self.normalized_time_selection().is_some()
            || self.interaction.time_selection_gesture.is_some()
            || self.interaction.time_range_move_gesture.is_some()
        {
            return;
        }
        self.interaction.loop_start_over_clip =
            self.interaction.edit_cursor_in_selected_clip.take();
        self.interaction.edit_cursor = None;
        self.interaction.time_selection = None;
        self.interaction.time_selection_project_scope = false;
        self.interaction.suppress_cursor_click = false;
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn tick_edit_cursor(&mut self, now: Instant) -> bool {
        let tick = self.tick_edit_cursor_detailed(now);
        tick.structural_changed || tick.pulse_changed
    }

    pub(super) fn tick_edit_cursor_detailed(&mut self, now: Instant) -> EditCursorTick {
        let mut structural_changed = false;
        if self
            .interaction
            .locked_play_start
            .is_some_and(|cursor| !self.cursor_is_current(cursor))
        {
            self.interaction.locked_play_start = None;
            structural_changed |= self.cancel_locked_play_start_drag();
        }
        if self
            .interaction
            .locked_play_start_drag
            .is_some_and(|drag| !self.locked_play_start_drag_valid(drag))
        {
            structural_changed |= self.cancel_locked_play_start_drag();
        }
        if self.context_menu.is_some()
            || self.persistence.close_prompt_open
            || (self.mode != EditorMode::Arrangement
                && (self.interaction.time_selection_gesture.is_some()
                    || self.interaction.time_range_move_gesture.is_some()))
            || self.persistence.save_dialog_open
            || self.persistence.export_dialog_open
        {
            return EditCursorTick {
                structural_changed: self.cancel_time_selection_gesture() || structural_changed,
                pulse_changed: false,
            };
        }
        // The locked triangles can outlive the point cursor or coexist with
        // range endpoints. Update them together from one elapsed-time phase.
        let lock_pulse_changed = self.mode == EditorMode::Arrangement
            && !self.interaction.play_start_at_beginning
            && self
                .interaction
                .locked_play_start
                .as_mut()
                .is_some_and(|locked| locked.pulse_locked_marker(now));
        if self.interaction.time_selection_project_scope
            && let Some(state) = &self.state
        {
            let tracks = state.project.tracks();
            let first = tracks.first().map(|track| track.id());
            if let Some(cursor) = self.interaction.edit_cursor.as_mut() {
                cursor.track = first;
            }
            if let Some(selection) = self.interaction.time_selection.as_mut() {
                selection.anchor_track = first;
                selection.end_track = tracks.last().map(|track| track.id());
            }
        }
        let Some(cursor) = &mut self.interaction.edit_cursor else {
            return EditCursorTick {
                structural_changed: self.cancel_time_selection_gesture() || structural_changed,
                pulse_changed: lock_pulse_changed,
            };
        };
        if self
            .state
            .as_ref()
            .is_none_or(|state| !cursor.is_current(self.project_generation, &state.project))
        {
            self.interaction.edit_cursor = None;
            self.interaction.time_selection = None;
            self.interaction.time_selection_project_scope = false;
            self.interaction.time_selection_gesture = None;
            self.interaction.suppress_cursor_click = false;
            return EditCursorTick {
                structural_changed: true,
                pulse_changed: lock_pulse_changed,
            };
        }
        if !self.interaction.time_selection_project_scope
            && self.interaction.time_selection.is_some_and(|selection| {
                self.state.as_ref().is_none_or(|state| {
                    [selection.anchor_track, selection.end_track]
                        .into_iter()
                        .any(|track| track.is_none_or(|track| state.project.track(track).is_none()))
                })
            })
        {
            self.interaction.time_selection = None;
            self.interaction.time_selection_project_scope = false;
            self.interaction.time_selection_gesture = None;
            self.interaction.suppress_cursor_click = false;
            return EditCursorTick {
                structural_changed: true,
                pulse_changed: lock_pulse_changed,
            };
        }
        if self
            .interaction
            .time_selection
            .is_some_and(|selection| selection.anchor_frame != selection.end_frame)
        {
            return EditCursorTick {
                structural_changed,
                pulse_changed: lock_pulse_changed,
            };
        }
        // A completed range belongs to Arrangement even while Console is shown.
        // Its stale-source checks above still run; hidden point cursors need no pulse.
        // A point cursor hidden behind a rectangle play start needs no pulse either.
        let point_pulse_changed = self.mode == EditorMode::Arrangement
            && !self.interaction.play_start_at_beginning
            && self.selected_arrangement_play_start().is_none()
            && self
                .interaction
                .edit_cursor
                .as_mut()
                .is_some_and(|cursor| cursor.pulse(now));
        EditCursorTick {
            structural_changed,
            pulse_changed: lock_pulse_changed || point_pulse_changed,
        }
    }

    /// Paint the same live range bounds above the loop rail and at the bottom
    /// of Arrangement. No canonical state or pointer-capture ownership changes.
    pub(super) fn edit_cursor_markers(
        &self,
        viewport: super::viewport::Viewport,
        edge: hex_widgets::TimeMarkerEdge,
        guide: Option<u64>,
        clip_editor_start: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        use hex_widgets::{TimeMarkerEndpoint as Endpoint, time_position_marker};
        let locked = self.valid_locked_play_start();
        let cursor = self
            .interaction
            .edit_cursor
            .filter(|cursor| cursor.generation == self.project_generation);
        let cursor = cursor.or(locked);
        let reset = self.interaction.play_start_at_beginning
            && guide.is_none()
            && !self.ruler_locked_play_start_drag_active();
        let clip_editor_start =
            clip_editor_start.filter(|_| !reset && guide.is_none() && locked.is_none());
        let selection_start =
            if reset || guide.is_some() || (cursor.is_none() && self.selected_clip.is_none()) {
                None
            } else {
                self.selected_arrangement_play_start()
            };
        let clip_start = selection_start.filter(|_| self.selected_clip.is_some());
        let cursor_frame = cursor
            .map(|cursor| cursor.frame)
            .or(clip_editor_start)
            .or(guide)
            .or(reset.then_some(0))
            .or(clip_start)?;
        let (start, end) =
            self.interaction
                .time_selection
                .map_or((cursor_frame, cursor_frame), |selection| {
                    (
                        selection.anchor_frame.min(selection.end_frame),
                        selection.anchor_frame.max(selection.end_frame),
                    )
                });
        let edge_name = match edge {
            hex_widgets::TimeMarkerEdge::Top => "top",
            hex_widgets::TimeMarkerEdge::Bottom => "bottom",
        };
        if locked.is_none()
            && clip_editor_start.is_none()
            && start == end
            && selection_start.is_some()
            && clip_start.is_none()
        {
            return None;
        }
        let mut markers = Vec::new();
        if guide.is_none() && start != end {
            for (frame, endpoint, name) in [
                (start, Endpoint::Start, "start"),
                (end, Endpoint::End, "end"),
            ] {
                if let Some(marker) = viewport.fraction(frame).and_then(|position| {
                    time_position_marker(
                        format!("time-marker-{edge_name}-{name}").into(),
                        position,
                        endpoint,
                        edge,
                        theme::time_position_marker_color(),
                    )
                }) {
                    markers.push(marker.into_any_element());
                }
            }
        }
        if let Some(frame) = guide
            && let Some(marker) = viewport.fraction(frame).and_then(|position| {
                time_position_marker(
                    format!("clip-move-guide-{edge_name}").into(),
                    position,
                    Endpoint::Point,
                    edge,
                    theme::time_position_marker_color(),
                )
            })
        {
            markers.push(marker.into_any_element());
        }
        let point_frame = if let Some(locked) = locked {
            Some(locked.frame)
        } else if clip_editor_start.is_some() {
            clip_editor_start
        } else if guide.is_some() || start != end {
            None
        } else if selection_start.is_some() {
            clip_start.map(|_| self.playback_start_frame())
        } else {
            cursor.map(|cursor| cursor.frame)
        };
        if reset
            && let Some(marker) = viewport.fraction(0).and_then(|position| {
                time_position_marker(
                    format!("transport-start-{edge_name}").into(),
                    position,
                    Endpoint::Point,
                    edge,
                    theme::time_position_marker_color(),
                )
            })
        {
            markers.push(marker.into_any_element());
        }
        if !reset
            && let Some(frame) = point_frame
            && let Some(position) = viewport.fraction(frame)
            && let Some(mut marker) = if locked.is_some() && self.uses_sibling_playback_overlay() {
                if position.is_finite() && (0.0..=1.0).contains(&position) {
                    let slots = std::rc::Rc::clone(&self.playback_subviews.slots);
                    let bottom_entity =
                        (edge == hex_widgets::TimeMarkerEdge::Bottom).then(|| cx.entity());
                    Some(
                        div()
                            .absolute()
                            .left(gpui::relative(position))
                            .ml(px(-5.0))
                            .w(px(10.0))
                            .h(px(hex_widgets::TIME_POSITION_MARKER_HEIGHT))
                            .when(edge == hex_widgets::TimeMarkerEdge::Top, |m| m.top_0())
                            .when(edge == hex_widgets::TimeMarkerEdge::Bottom, |m| {
                                m.bottom_0()
                            })
                            .child(
                                canvas(
                                    move |bounds, window, cx| {
                                        if let Ok(mut slots) = slots.try_borrow_mut() {
                                            slots.locked_markers.push(
                                                super::playback_subviews::LockedMarkerSlot {
                                                    edge,
                                                    bounds,
                                                    clip_mask: window.content_mask().bounds,
                                                },
                                            );
                                        }
                                        if let Some(entity) = &bottom_entity {
                                            entity
                                                .read(cx)
                                                .interaction
                                                .bottom_play_start_marker_bounds
                                                .set(Some(bounds));
                                        }
                                    },
                                    |_, _, _, _| (),
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    )
                } else {
                    None
                }
            } else {
                time_position_marker(
                    if locked.is_some() {
                        format!("locked-play-start-{edge_name}").into()
                    } else if clip_editor_start.is_some() {
                        format!("clip-editor-play-start-{edge_name}").into()
                    } else {
                        format!("time-marker-{edge_name}-start").into()
                    },
                    position,
                    Endpoint::Point,
                    edge,
                    locked.map_or_else(theme::time_position_marker_color, |point| point.color()),
                )
            }
        {
            if edge == hex_widgets::TimeMarkerEdge::Bottom
                && !(locked.is_some() && self.uses_sibling_playback_overlay())
            {
                let bounds_entity = cx.entity();
                marker = marker.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, _, cx| {
                            bounds_entity
                                .read(cx)
                                .interaction
                                .bottom_play_start_marker_bounds
                                .set(Some(bounds));
                        },
                    )
                    .absolute()
                    .inset_0(),
                );
            }
            markers.push(
                marker
                    .id(if edge == hex_widgets::TimeMarkerEdge::Top {
                        "play-start-lock-top"
                    } else {
                        "play-start-lock-bottom"
                    })
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                            let is_locked = view.valid_locked_play_start().is_some();
                            let modifiers_ok = if is_locked {
                                Self::locked_play_start_drag_modifiers(event.modifiers)
                            } else {
                                event.modifiers == gpui::Modifiers::default()
                            };
                            if modifiers_ok {
                                let point =
                                    (f32::from(event.position.x), f32::from(event.position.y));
                                // Arrangement capture saw this press before the marker.
                                // Discard its pending tail drag so the marker can claim it.
                                if view
                                    .interaction
                                    .time_selection_gesture
                                    .as_ref()
                                    .is_some_and(|gesture| {
                                        !gesture.moved && gesture.tail_press == Some(point)
                                    })
                                {
                                    view.interaction.time_selection_gesture = None;
                                }
                                if is_locked {
                                    if view.begin_locked_play_start_drag(
                                        event.position,
                                        event.modifiers,
                                        cx,
                                    ) {
                                        view.focus.focus(window);
                                        cx.notify();
                                    }
                                } else {
                                    let current = clip_editor_start.filter(|frame| {
                                        view.focused_clip_editor_play_start(window) == Some(*frame)
                                    });
                                    if (clip_editor_start.is_none() || current.is_some())
                                        && view.lock_play_start(current, cx)
                                    {
                                        view.focus.focus(window);
                                    }
                                }
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .into_any_element(),
            );
        }
        // Point edits can leave a cursor at a different frame from the lock.
        // Only the locked marker toggles that lock.
        if !reset
            && guide.is_none()
            && start == end
            && let (Some(locked), Some(cursor)) = (locked, cursor)
            && cursor.frame != locked.frame
            && selection_start.is_none()
            && let Some(marker) = viewport.fraction(cursor.frame).and_then(|position| {
                time_position_marker(
                    format!("time-marker-{edge_name}-start").into(),
                    position,
                    Endpoint::Point,
                    edge,
                    theme::time_position_marker_color(),
                )
            })
        {
            markers.push(marker.into_any_element());
        }
        Some(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(hex_widgets::TIME_POSITION_MARKER_HEIGHT))
                .children(markers),
        )
    }

    pub(super) fn edit_cursor_layer(
        &self,
        viewport: super::viewport::Viewport,
        guide: Option<u64>,
        clip_editor_start: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        self.interaction.bottom_play_start_marker_bounds.set(None);
        let markers = self.edit_cursor_markers(
            viewport,
            hex_widgets::TimeMarkerEdge::Bottom,
            guide,
            clip_editor_start,
            cx,
        );
        let line = if guide.is_some() {
            None
        } else if self.interaction.play_start_at_beginning
            && !self.ruler_locked_play_start_drag_active()
        {
            Some((
                0,
                theme::time_position_marker_color(),
                "transport-start-line",
                false,
            ))
        } else {
            self.project_point_cursor().map(|cursor| {
                (
                    cursor.frame,
                    cursor.color(),
                    "project-edit-cursor-line",
                    true,
                )
            })
        };
        let line = line.and_then(|(frame, color, selector, pulsing)| {
            let position = viewport.fraction(frame)?;
            if pulsing && self.uses_sibling_playback_overlay() {
                let slots = std::rc::Rc::clone(&self.playback_subviews.slots);
                return Some(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(gpui::relative(position))
                        .w(px(1.0))
                        .child(
                            canvas(
                                move |bounds, window, _| {
                                    if let Ok(mut slots) = slots.try_borrow_mut() {
                                        slots.edit_cursor_lines.push(
                                            super::playback_subviews::EditCursorLineSlot {
                                                selector: selector.into(),
                                                bounds,
                                                clip_mask: window.content_mask().bounds,
                                            },
                                        );
                                    }
                                },
                                |_, _, _, _| (),
                            )
                            .absolute()
                            .inset_0(),
                        ),
                );
            }
            Some(
                div()
                    .debug_selector(move || selector.into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(position))
                    .w(px(1.0))
                    .bg(gpui::rgb(color)),
            )
        });
        if markers.is_none() && line.is_none() {
            return None;
        }
        Some(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right(px(self.track_header_width()))
                .debug_selector(|| "edit-cursor-layer".into())
                .children(line)
                .children(markers),
        )
    }

    pub(super) fn split_arrangement_clips(&mut self, window: &Window, cx: &Context<Self>) {
        if self.mode != EditorMode::Arrangement {
            self.error = Some("split is available in arrangement".into());
            return;
        }
        // Ordinary track headers belong to the same arranger editing scope.
        let arranger_focused = self.focus.is_focused(window) || self.track_header_focused(window);
        if (!arranger_focused && !self.midi_editor.focus.is_focused(window))
            || !self.arrangement_view_change_available(cx)
            || self.device_panel.has_pending_draft(cx)
            || (arranger_focused && self.automation_view.enabled)
        {
            return;
        }
        if arranger_focused
            && self
                .interaction
                .time_selection
                .is_some_and(|s| s.anchor_frame != s.end_frame)
        {
            let Some(range) = self.normalized_time_selection() else {
                self.error = Some("time selection is no longer available".into());
                return;
            };
            self.cancel_duplicate_chain();
            self.enqueue(Request::Edit(Command::SplitTimeRange { range }));
            return;
        }
        let Some(cursor) = self
            .interaction
            .edit_cursor
            .filter(|cursor| cursor.generation == self.project_generation)
        else {
            self.error = Some("click a track to place the edit cursor".into());
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        if self.interaction.time_selection_project_scope {
            let tracks: Vec<_> = state
                .project
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect();
            let beat = state.project.tempo_map().frame_to_beat(cursor.frame).ok();
            let has_crossing_clip = state.project.tracks().iter().any(|track| {
                track.clips().iter().any(|clip| {
                    cursor.frame > clip.start()
                        && clip
                            .start()
                            .checked_add(clip.duration())
                            .is_some_and(|end| cursor.frame < end)
                }) || beat.is_some_and(|beat| {
                    track.midi_clips().iter().any(|clip| {
                        beat > clip.start()
                            && clip
                                .start()
                                .raw()
                                .checked_add(clip.length().raw())
                                .is_some_and(|end| beat.raw() < end)
                    })
                })
            });
            if !has_crossing_clip {
                self.error = Some("edit cursor must be inside a clip to split".into());
                return;
            }
            self.cancel_duplicate_chain();
            if self.enqueue(Request::Edit(Command::SplitTracksAtFrame {
                tracks,
                frame: cursor.frame,
            })) {
                self.selected_clip = None;
            }
            return;
        }
        let Some(cursor_track) = cursor.track else {
            self.error = Some("click a track to place the edit cursor for splitting".into());
            return;
        };
        let Some(track) = state.project.track(cursor_track) else {
            self.error = Some("edit cursor track no longer exists".into());
            return;
        };
        let audio = track.clips().iter().filter(|clip| {
            cursor.frame > clip.start()
                && clip
                    .start()
                    .checked_add(clip.duration())
                    .is_some_and(|end| cursor.frame < end)
        });
        let beat = state.project.tempo_map().frame_to_beat(cursor.frame).ok();
        let midi = track.midi_clips().iter().filter(|clip| {
            beat.is_some_and(|beat| {
                beat > clip.start()
                    && clip
                        .start()
                        .raw()
                        .checked_add(clip.length().raw())
                        .is_some_and(|end| beat.raw() < end)
            })
        });
        let mut commands: Vec<(ClipId, Command)> = audio
            .map(|clip| {
                (
                    clip.id(),
                    Command::SplitClip {
                        clip: clip.id(),
                        frame: cursor.frame,
                    },
                )
            })
            .chain(midi.filter_map(|clip| {
                beat.map(|beat| {
                    (
                        clip.id(),
                        Command::SplitMidiClip {
                            clip: clip.id(),
                            beat,
                        },
                    )
                })
            }))
            .collect();
        let selected = self
            .selected_clip
            .and_then(|id| commands.iter().position(|(clip, _)| *clip == id))
            .unwrap_or(0);
        if commands.is_empty() {
            self.error = Some("edit cursor must be inside a clip to split".into());
            return;
        }
        let (clip, command) = commands.swap_remove(selected);
        self.cancel_duplicate_chain();
        if self.enqueue(Request::Edit(command)) {
            self.selected_clip = Some(clip);
            self.selected_track = Some(cursor_track);
        }
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    #[test]
    fn cursor_pulse_interpolates_smoothly_and_uses_elapsed_time_without_drift() {
        let now = Instant::now();
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).expect("project"), 4)
                .expect("editor");
        let hex_project::Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .expect("track")
        else {
            panic!("track");
        };
        let mut cursor = EditCursor {
            track: Some(track),
            frame: 0,
            generation: 0,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: now,
        };
        assert!(!cursor.pulse(now));
        assert_eq!(cursor.color(), theme::color(theme::PaletteRole::Text));
        assert!(cursor.pulse(now + Duration::from_millis(300)));
        let midpoint = cursor.color();
        assert_ne!(midpoint, theme::color(theme::PaletteRole::Text));
        assert_ne!(midpoint, theme::color(theme::PaletteRole::Secondary));
        assert!(cursor.pulse(now + Duration::from_millis(600)));
        assert_eq!(cursor.color(), theme::color(theme::PaletteRole::Secondary));
        assert!(cursor.pulse(now + Duration::from_millis(900)));
        assert_eq!(cursor.color(), midpoint);
        assert!(cursor.pulse(now + Duration::from_millis(1_200)));
        assert_eq!(cursor.color(), theme::color(theme::PaletteRole::Text));
        // A skipped timer tick resumes the same phase, rather than toggling once.
        assert!(cursor.pulse(now + Duration::from_millis(3_000)));
        assert_eq!(cursor.color(), theme::color(theme::PaletteRole::Secondary));
        assert!(!cursor.pulse(now + Duration::from_millis(3_000)));
    }
}
