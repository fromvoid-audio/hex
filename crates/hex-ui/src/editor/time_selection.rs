//! Arrangement time selection, range movement and worker clipboard routing.
use super::{
    EditorMode, EditorView,
    duplicate::MAX_QUEUED_REPEATS,
    edit_cursor::EditCursor,
    pending_edit::{Disposition, Receipt},
};
use crate::theme;
use gpui::{
    Context, CursorStyle, Div, Stateful, Window, canvas, div, prelude::*, px, relative, rgb,
};
use hex_project::{ClipId, Command, TrackId};
use hex_session::service::{Event, Request};
use hex_widgets::{RangeSelectionTarget, RangeSelectionZones};
use std::time::Instant;

pub(super) const SELECTION_HANDLE_FRACTION: f32 = hex_widgets::RANGE_HANDLE_FRACTION;

/// Other modifier chords retain their existing section/loop editing meanings.
pub(super) fn is_extent_selection_click(modifiers: gpui::Modifiers) -> bool {
    modifiers.shift && !modifiers.platform && !modifiers.alt && !modifiers.control
}

/// A compact row keeps the complete title usable; larger rows retain their
/// existing proportional move rail. Rendered affordances and hit tests agree.
pub(super) fn header_hit_height(row_height: f32) -> f32 {
    RangeSelectionZones::new(row_height, super::clip_name::CLIP_NAME_HEIGHT)
        .map_or(0.0, RangeSelectionZones::handle_height)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TimeSelection {
    // An empty project's Main range has no ordinary track anchors.
    pub(super) anchor_track: Option<TrackId>,
    pub(super) end_track: Option<TrackId>,
    pub(super) anchor_frame: u64,
    pub(super) end_frame: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TimeSelectionGesture {
    pub selection: TimeSelection,
    pub generation: u64,
    pub revision: u64,
    pub moved: bool,
    pub(super) pointer_x: f32,
    pub(super) pointer_y: f32,
    pub(super) free_modifier: bool,
    pub(super) last_tick: Instant,
}

#[derive(Clone, Debug)]
pub(super) struct TimeRangeMoveGesture {
    range: hex_project::TimeRange,
    pub(super) generation: u64,
    pub(super) revision: u64,
    grab_frame_offset: u64,
    grab_track_offset: usize,
    pub(super) copy_modifier: bool,
    pub(super) pointer_x: f32,
    pub(super) pointer_y: f32,
    pub(super) free_modifier: bool,
    pub(super) last_tick: Instant,
    preview: Option<TimeRangeMovePreview>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TimeRangeMovePreview {
    first_track: TrackId,
    last_track: TrackId,
    first_index: usize,
    last_index: usize,
    start_frame: u64,
    end_frame: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TimeRangeMoveDisplay<'a> {
    pub source: &'a hex_project::TimeRange,
    pub destination_first_index: usize,
    pub destination_last_index: usize,
    pub destination_start: u64,
    pub destination_end: u64,
    pub copy_modifier: bool,
}

#[derive(Debug)]
pub(super) struct PendingTimeRangeMove {
    pub source: hex_project::TimeRange,
    pub range: hex_project::TimeRange,
    pub project_scope: bool,
    pub destination_first_index: usize,
    pub destination_last_index: usize,
    pub source_revision: u64,
    pub selection_intent_epoch: u64,
    pub copy_modifier: bool,
    pub(super) keyboard: Option<KeyboardTimeCopy>,
}

#[derive(Debug)]
pub(super) struct KeyboardTimeCopy {
    receipt: Receipt,
    /// Cancellation drops future repeats while retaining the released copy.
    repeats: Option<u8>,
}

impl EditorView {
    /// Actual selection choices supersede pending ACK side effects, even when
    /// the user returns to identical coordinates. Exhaustion stays invalid for
    /// this view's lifetime; never reuse an epoch or cancel the canonical edit.
    pub(super) fn advance_selection_intent(&mut self) {
        self.play_start_at_beginning = false;
        self.escape_stop_armed = false;
        if self
            .edit_cursor
            .is_some_and(|cursor| cursor.track.is_none())
        {
            self.edit_cursor = None;
        }
        self.selection_intent_epoch = self
            .selection_intent_epoch
            .and_then(|epoch| epoch.checked_add(1));
    }

    pub(super) fn normalized_time_selection(&self) -> Option<hex_project::TimeRange> {
        let selection = self.time_selection?;
        if selection.anchor_frame == selection.end_frame {
            return None;
        }
        let state = self.state.as_ref()?;
        let tracks = if self.time_selection_project_scope {
            state
                .project
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect()
        } else {
            let (first, last) = self.time_selection_track_indices()?;
            state.project.tracks()[first..=last]
                .iter()
                .map(|track| track.id())
                .collect()
        };
        hex_project::TimeRange::new(
            tracks,
            selection.anchor_frame.min(selection.end_frame),
            selection.anchor_frame.max(selection.end_frame),
        )
        .ok()
    }

    /// A Shift-click can extend only a live cursor in the current document.
    fn time_selection_cursor_anchor(&self, extend: bool) -> Option<EditCursor> {
        if !extend {
            return None;
        }
        self.edit_cursor.filter(|cursor| {
            cursor.generation == self.project_generation
                && self.state.as_ref().is_some_and(|state| {
                    cursor
                        .track
                        .is_some_and(|track| state.project.track(track).is_some())
                })
        })
    }

    fn range_input_available(&self) -> bool {
        self.mode == EditorMode::Arrangement
            && (!self.automation_view.enabled || self.time_selection_key_held)
            && self.clip_name_edit.is_none()
            && !self.sections.naming()
            && !self.track_name.is_editing()
            && self.context_menu.is_none()
            && !self.opening_project
            && !self.close_prompt_open
            && !self.save_dialog_open
            && !self.export_dialog_open
            && self.active_clip_drag.is_none()
            && self.active_browser_audio_drag.is_none()
            && self.active_native_device_drag.is_none()
            && self.active_trim_drag.is_none()
            && self.active_fade_drag.is_none()
            && self.active_midi_trim_drag.is_none()
    }

    /// Select an exact captured extent without snapping, seeking or an edit.
    /// This is the same project-scope selection/cursor owner as Main painting;
    /// normal tracks come from canonical order, never the focused display list.
    pub(super) fn select_project_time_range(
        &mut self,
        start: u64,
        end: u64,
        generation: u64,
        revision: u64,
    ) -> bool {
        if start >= end
            || generation != self.project_generation
            || !self.range_input_available()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.sections.held()
            || self.loop_rail_gesture.is_some()
            || self.arrangement_pan.is_some()
        {
            return false;
        }
        let Some(state) = self
            .state
            .as_ref()
            .filter(|state| state.revision == revision)
        else {
            return false;
        };
        let (Some(first), Some(last)) = (
            state.project.tracks().first(),
            state.project.tracks().last(),
        ) else {
            return false;
        };
        let selection = TimeSelection {
            anchor_track: Some(first.id()),
            end_track: Some(last.id()),
            anchor_frame: start,
            end_frame: end,
        };
        self.advance_selection_intent();
        self.retire_section_pointer_context_for_range();
        self.time_selection_project_scope = true;
        self.time_selection = Some(selection);
        self.edit_cursor = Some(EditCursor {
            track: selection.anchor_track,
            frame: start,
            generation,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: Instant::now(),
        });
        self.selected_clip = None;
        self.suppress_cursor_click = true;
        true
    }

    pub(super) fn begin_main_time_selection_gesture(
        &mut self,
        x: f32,
        y: f32,
        free: bool,
        extend: bool,
    ) -> bool {
        self.suppress_cursor_click = false;
        if !self.range_input_available() {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let first_track = state.project.tracks().first().map(|track| track.id());
        let last_track = state.project.tracks().last().map(|track| track.id());
        let Some(lane) = self.main_track_lane_bounds.filter(|lane| {
            self.main_track_lane_epoch == self.drag_layout_generation && lane_contains(x, y, *lane)
        }) else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(frame) = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport) else {
            return false;
        };
        let Some(frame) = self.snap_grid.with_platform_modifier(free).snap_frame(
            &state.project,
            frame,
            viewport,
            lane.width,
        ) else {
            return false;
        };
        let anchor = self.time_selection_cursor_anchor(extend).or_else(|| {
            self.edit_cursor.filter(|cursor| {
                extend
                    && self.time_selection_project_scope
                    && cursor.track.is_none()
                    && cursor.generation == self.project_generation
            })
        });
        let generation = self.project_generation;
        let revision = state.revision;
        self.clear_automation_range();
        let selection = TimeSelection {
            anchor_track: first_track,
            end_track: last_track,
            anchor_frame: anchor.map_or(frame, |cursor| cursor.frame),
            end_frame: frame,
        };
        self.advance_selection_intent();
        self.time_selection_project_scope = true;
        self.edit_cursor = Some(anchor.unwrap_or(EditCursor {
            track: first_track,
            frame,
            generation,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: Instant::now(),
        }));
        self.time_selection = Some(selection);
        self.time_selection_gesture = Some(TimeSelectionGesture {
            selection,
            generation,
            revision,
            moved: anchor.is_some(),
            pointer_x: x,
            pointer_y: y,
            free_modifier: free,
            last_tick: Instant::now(),
        });
        self.selected_clip = None;
        true
    }

    /// Existing Main range handle has priority over an empty-lane point click.
    /// Capture and section-lane arbitration share this exact geometry query.
    pub(super) fn main_time_range_grab(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(hex_project::TimeRange, u64)> {
        if !self.time_selection_project_scope {
            return None;
        }
        let range = self.normalized_time_selection()?;
        let state = self.state.as_ref()?;
        let lane = self.main_track_lane_bounds.filter(|lane| {
            self.main_track_lane_epoch == self.drag_layout_generation && lane_contains(x, y, *lane)
        })?;
        if RangeSelectionZones::new(lane.height, super::clip_name::CLIP_NAME_HEIGHT)?.target(
            y - lane.top,
            true,
            false,
        ) != Some(RangeSelectionTarget::Move)
        {
            return None;
        }
        let frame = super::seek::pointer_to_frame(
            x,
            lane.left,
            lane.width,
            self.visible_viewport(state.timeline_frames),
        )?;
        if frame < range.start() || frame >= range.end() {
            return None;
        }
        let offset = frame.checked_sub(range.start())?;
        Some((range, offset))
    }

    pub(super) fn begin_main_time_range_move_gesture(&mut self, x: f32, y: f32) -> bool {
        self.suppress_cursor_click = false;
        if !self.time_selection_project_scope
            || self.mode != EditorMode::Arrangement
            || self.edit_pending()
            || self.clip_name_edit.is_some()
            || self.sections.naming()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.active_midi_trim_drag.is_some()
        {
            return false;
        }
        let Some((range, grab_frame_offset)) = self.main_time_range_grab(x, y) else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        self.time_range_move_gesture = Some(TimeRangeMoveGesture {
            range,
            generation: self.project_generation,
            revision: state.revision,
            grab_frame_offset,
            grab_track_offset: 0,
            copy_modifier: false,
            pointer_x: x,
            pointer_y: y,
            free_modifier: false,
            last_tick: Instant::now(),
            preview: None,
        });
        true
    }

    pub(super) fn begin_time_selection_gesture(
        &mut self,
        x: f32,
        y: f32,
        free: bool,
        extend: bool,
    ) -> bool {
        self.suppress_cursor_click = false;
        if self.mode != EditorMode::Arrangement
            || self.clip_name_edit.is_some()
            || self.sections.naming()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.active_midi_trim_drag.is_some()
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some((track, lane)) = self.visible_lane_at(x, y) else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(frame) = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport) else {
            return false;
        };
        let inside_selection = self.normalized_time_selection().is_some_and(|range| {
            range.tracks().contains(&track) && frame >= range.start() && frame < range.end()
        });
        let Some(frame) = self.snap_grid.with_platform_modifier(free).snap_frame(
            &state.project,
            frame,
            viewport,
            lane.width,
        ) else {
            return false;
        };
        let (ordinary_handle, priority_control, _, _) =
            self.clip_pointer_handles(track, x, y, viewport, self.option_held, true);
        if !self.time_selection_key_held
            && (priority_control || ((extend || !inside_selection) && ordinary_handle))
        {
            return false;
        }
        let anchor = self.time_selection_cursor_anchor(extend);
        let generation = self.project_generation;
        let revision = state.revision;
        let anchor_track = anchor.and_then(|cursor| cursor.track).unwrap_or(track);
        self.clear_automation_range();
        let selection = TimeSelection {
            anchor_track: Some(anchor_track),
            end_track: Some(track),
            anchor_frame: anchor.map_or(frame, |cursor| cursor.frame),
            end_frame: frame,
        };
        self.advance_selection_intent();
        self.edit_cursor = Some(anchor.unwrap_or(EditCursor {
            track: Some(track),
            frame,
            generation,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: Instant::now(),
        }));
        self.time_selection = Some(selection);
        self.time_selection_project_scope = false;
        self.time_selection_gesture = Some(TimeSelectionGesture {
            selection,
            generation,
            revision,
            moved: anchor.is_some(),
            pointer_x: x,
            pointer_y: y,
            free_modifier: free,
            last_tick: Instant::now(),
        });
        self.select_lane_span(anchor_track, track);
        // The pressed clip stays the clip editor subject, not an object selection.
        let pressed = self.arrangement_clip_at(track, frame, self.detail_clip_id());
        if anchor.is_none() {
            self.set_detail_context(pressed);
        }
        self.selected_clip = None;
        self.cancel_midi_note_gesture();
        true
    }

    pub(super) fn begin_time_range_move_gesture(&mut self, x: f32, y: f32) -> bool {
        if self.mode != EditorMode::Arrangement
            || self.edit_pending()
            || self.clip_name_edit.is_some()
            || self.sections.naming()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.active_midi_trim_drag.is_some()
        {
            return false;
        }
        let Some(range) = self.normalized_time_selection() else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some((track, lane)) = self.visible_lane_at(x, y) else {
            return false;
        };
        // The visible title/move rail is the handle; the rest reselects.
        if RangeSelectionZones::new(lane.height, super::clip_name::CLIP_NAME_HEIGHT)
            .and_then(|zones| zones.target(y - lane.top, true, false))
            != Some(RangeSelectionTarget::Move)
        {
            return false;
        }
        let Some(track_offset) = range.tracks().iter().position(|item| *item == track) else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(frame) = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport) else {
            return false;
        };
        if frame < range.start() || frame >= range.end() {
            return false;
        }
        let Some(grab_frame_offset) = frame.checked_sub(range.start()) else {
            return false;
        };
        self.time_range_move_gesture = Some(TimeRangeMoveGesture {
            range,
            generation: self.project_generation,
            revision: state.revision,
            grab_frame_offset,
            grab_track_offset: track_offset,
            copy_modifier: false,
            pointer_x: x,
            pointer_y: y,
            free_modifier: false,
            last_tick: Instant::now(),
            preview: None,
        });
        self.suppress_cursor_click = false;
        true
    }

    pub(super) fn update_time_range_move_gesture(&mut self, x: f32, y: f32, free: bool) -> bool {
        let Some(mut gesture) = self.time_range_move_gesture.take() else {
            return false;
        };
        // A dragged range must not also become a fresh lane click on release.
        self.suppress_cursor_click = true;
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || gesture.generation != self.project_generation
            || gesture.revision != state.revision
            || self.clip_name_edit.is_some()
            || self.sections.naming()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.close_prompt_open
            || self.opening_project
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return true;
        }
        gesture.pointer_x = x;
        gesture.pointer_y = y;
        gesture.free_modifier = free;
        let Some((Some(pointer_track), lane)) = self.time_gesture_lane_near_y(y) else {
            self.time_range_move_gesture = Some(gesture);
            return true;
        };
        let Some(pointer_index) = state
            .project
            .tracks()
            .iter()
            .position(|track| track.id() == pointer_track)
        else {
            return true;
        };
        let row_count = gesture.range.tracks().len();
        let max_first = state.project.tracks().len().saturating_sub(row_count);
        let first_index = pointer_index
            .saturating_sub(gesture.grab_track_offset)
            .min(max_first);
        let Some(last_index) = first_index.checked_add(gesture.range.tracks().len() - 1) else {
            gesture.preview = None;
            self.time_range_move_gesture = Some(gesture);
            return true;
        };
        let Some(rows) = state.project.tracks().get(first_index..=last_index) else {
            self.time_range_move_gesture = Some(gesture);
            return true;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(pointer_frame) = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport)
        else {
            self.time_range_move_gesture = Some(gesture);
            return true;
        };
        let candidate_frame = pointer_frame.saturating_sub(gesture.grab_frame_offset);
        let Some(candidate_frame) = self.snap_grid.with_platform_modifier(free).snap_frame(
            &state.project,
            candidate_frame,
            viewport,
            lane.width,
        ) else {
            self.time_range_move_gesture = Some(gesture);
            return true;
        };
        let Some(end_frame) = candidate_frame.checked_add(gesture.range.duration()) else {
            self.time_range_move_gesture = Some(gesture);
            return true;
        };
        let (Some(first_track), Some(last_track)) = (rows.first(), rows.last()) else {
            self.time_range_move_gesture = Some(gesture);
            return true;
        };
        gesture.preview = Some(TimeRangeMovePreview {
            first_track: first_track.id(),
            last_track: last_track.id(),
            first_index,
            last_index,
            start_frame: candidate_frame,
            end_frame,
        });
        self.time_range_move_gesture = Some(gesture);
        true
    }

    pub(super) fn finish_time_range_move_gesture(&mut self) -> bool {
        let Some(gesture) = self.time_range_move_gesture.take() else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || gesture.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != gesture.revision)
            || self.clip_name_edit.is_some()
            || self.sections.naming()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return true;
        }
        let Some(preview) = gesture.preview else {
            return true;
        };
        if preview.first_track == gesture.range.tracks()[0]
            && gesture.range.tracks().last().copied() == Some(preview.last_track)
            && preview.start_frame == gesture.range.start()
        {
            return true;
        }
        if self.edit_pending() {
            return true;
        }
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        let Some(first) = state
            .project
            .tracks()
            .iter()
            .position(|track| track.id() == preview.first_track)
        else {
            return true;
        };
        let Some(last) = state
            .project
            .tracks()
            .iter()
            .position(|track| track.id() == preview.last_track)
        else {
            return true;
        };
        let Some(rows) = state.project.tracks().get(first..=last) else {
            return true;
        };
        let Ok(destination) = hex_project::TimeRange::new(
            rows.iter().map(|track| track.id()).collect(),
            preview.start_frame,
            preview.end_frame,
        ) else {
            return true;
        };
        let target = preview.first_track;
        let source_revision = gesture.revision;
        let source = gesture.range.clone();
        let project_scope = self.time_selection_project_scope;
        let command = if gesture.copy_modifier {
            Command::CopyTimeRangeTo {
                range: gesture.range,
                track: target,
                frame: destination.start(),
            }
        } else {
            Command::MoveTimeRange {
                range: gesture.range,
                track: target,
                frame: destination.start(),
            }
        };
        let Some(selection_intent_epoch) = self.selection_intent_epoch else {
            self.error = Some(
                "selection intent exhausted · reopen the editor before moving or copying time"
                    .into(),
            );
            return true;
        };
        if self.enqueue(Request::Edit(command)) {
            self.pending_time_range_move = Some(PendingTimeRangeMove {
                selection_intent_epoch,
                source,
                range: destination,
                project_scope,
                destination_first_index: first,
                destination_last_index: last,
                source_revision,
                copy_modifier: gesture.copy_modifier,
                keyboard: None,
            });
        }
        true
    }

    pub(super) fn time_selection_track_indices(&self) -> Option<(usize, usize)> {
        let selection = self.time_selection?;
        let tracks = self.state.as_ref()?.project.tracks();
        if self.time_selection_project_scope {
            return Some((0, tracks.len().checked_sub(1)?));
        }
        let anchor_track = selection.anchor_track?;
        let end_track = selection.end_track?;
        let anchor = tracks.iter().position(|track| track.id() == anchor_track)?;
        let end = tracks.iter().position(|track| track.id() == end_track)?;
        Some(if anchor <= end {
            (anchor, end)
        } else {
            (end, anchor)
        })
    }

    /// Duplicate the bounded Arrangement rectangle through the existing atomic
    /// copy edit. Each repeat starts from a committed destination and receives
    /// its own undo step.
    pub(super) fn duplicate_time_selection(&mut self, window: &Window) -> bool {
        if self.mode != EditorMode::Arrangement
            || self.midi_focus.is_focused(window)
            || !self.focus.is_focused(window)
        {
            return false;
        }
        if self
            .time_selection
            .is_none_or(|selection| selection.anchor_frame == selection.end_frame)
        {
            return false;
        }
        let Some(range) = self.normalized_time_selection() else {
            self.error = Some("time selection no longer belongs to current tracks".into());
            return true;
        };
        if self.time_duplicate_input_blocked() {
            self.error = Some("finish the current edit before duplicating time".into());
            return true;
        }
        let revision = self.state.as_ref().map_or(0, |state| state.revision);
        if let Some(pending) = &mut self.pending_time_range_move
            && let Some(keyboard) = &mut pending.keyboard
        {
            if pending.source != range
                || pending.source_revision != revision
                || self.selection_intent_epoch != Some(pending.selection_intent_epoch)
                || self.time_selection_project_scope != pending.project_scope
            {
                self.error = Some("time duplicate is still completing".into());
                return true;
            }
            let keyboard = self
                .ahead_time_range_move
                .as_mut()
                .and_then(|ahead| ahead.keyboard.as_mut())
                .unwrap_or(keyboard);
            match &mut keyboard.repeats {
                Some(repeats) if *repeats < MAX_QUEUED_REPEATS => *repeats += 1,
                Some(_) => {
                    self.error = Some("duplicate queue full · maximum 32 queued repeats".into());
                }
                None => self.error = Some("time duplicate is still completing".into()),
            }
            self.start_ahead_time_duplicate();
            return true;
        }
        if self.edit_pending() {
            self.error = Some("finish the current edit before duplicating time".into());
            return true;
        }
        self.start_time_duplicate(range, 0);
        true
    }

    fn time_duplicate_input_blocked(&self) -> bool {
        self.opening_project
            || self.close_prompt_open
            || self.automation_close_preflight.frozen()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.clip_name_edit.is_some()
            || self.sections.naming()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.save_dialog_open
            || self.export_dialog_open
    }

    pub(super) fn start_time_duplicate(&mut self, range: hex_project::TimeRange, repeats: u8) {
        let Some(end) = range.end().checked_add(range.duration()) else {
            self.error = Some("duplicated time range would overflow".into());
            return;
        };
        let Some((first, last)) = self.time_selection_track_indices() else {
            return;
        };
        let Ok(destination) =
            hex_project::TimeRange::new(range.tracks().to_vec(), range.end(), end)
        else {
            return;
        };
        let revision = self.state.as_ref().map_or(0, |state| state.revision);
        let command = Command::CopyTimeRangeTo {
            range: range.clone(),
            track: range.tracks()[0],
            frame: range.end(),
        };
        let Some(selection_intent_epoch) = self.selection_intent_epoch else {
            self.error = Some(
                "selection intent exhausted · reopen the editor before moving or copying time"
                    .into(),
            );
            return;
        };
        self.pending_time_range_move = Some(PendingTimeRangeMove {
            selection_intent_epoch,
            source: range,
            range: destination,
            project_scope: self.time_selection_project_scope,
            destination_first_index: first,
            destination_last_index: last,
            source_revision: revision,
            copy_modifier: true,
            keyboard: Some(KeyboardTimeCopy {
                receipt: Receipt::new(self.project_generation),
                repeats: Some(repeats),
            }),
        });
        if !self.enqueue(Request::Edit(command)) {
            self.pending_time_range_move = None;
        }
    }

    pub(super) fn continue_time_duplicate(&mut self, repeats: u8) {
        if let Some(remaining) = repeats.checked_sub(1)
            && self.mode == EditorMode::Arrangement
            && !self.edit_pending()
            && !self.time_duplicate_input_blocked()
            && let Some(range) = self.normalized_time_selection()
        {
            self.start_time_duplicate(range, remaining);
        }
    }

    pub(super) fn cancel_time_duplicate_repeats(&mut self) {
        if let Some(keyboard) = self
            .ahead_time_range_move
            .as_mut()
            .and_then(|pending| pending.keyboard.as_mut())
        {
            keyboard.repeats = None;
        }
        if let Some(keyboard) = self
            .pending_time_range_move
            .as_mut()
            .and_then(|pending| pending.keyboard.as_mut())
        {
            keyboard.repeats = None;
        }
    }

    pub(super) fn pending_time_duplicate_matches(&self, request: &Request) -> bool {
        self.pending_time_range_move
            .iter()
            .chain(self.ahead_time_range_move.iter())
            .any(|pending| {
                pending
                    .keyboard
                    .as_ref()
                    .is_some_and(|keyboard| keyboard.receipt.is_queued())
                    && matches!(
                        request,
                        Request::Edit(Command::CopyTimeRangeTo { range, track, frame })
                            if *range == pending.source
                                && pending.range.tracks().first() == Some(track)
                                && pending.range.start() == *frame
                    )
            })
    }

    pub(super) fn time_duplicate_submitted(&mut self) {
        for pending in self
            .pending_time_range_move
            .iter_mut()
            .chain(self.ahead_time_range_move.iter_mut())
        {
            if let Some(keyboard) = &mut pending.keyboard
                && keyboard.receipt.is_queued()
            {
                keyboard.receipt.submitted(self.worker_requests_pending);
                break;
            }
        }
    }

    pub(super) fn copy_time_selection(&mut self, cut: bool, window: &Window) {
        if self.track_clipboard_shortcut(
            window,
            if cut {
                super::track_clipboard::Shortcut::Cut
            } else {
                super::track_clipboard::Shortcut::Copy
            },
        ) {
            return;
        }
        if self.mode != EditorMode::Arrangement
            || self.midi_focus.is_focused(window)
            || !self.focus.is_focused(window)
        {
            return;
        }
        let Some(range) = self.normalized_time_selection() else {
            if !cut {
                let clips = self.selected_clip_ids();
                if !clips.is_empty() {
                    self.enqueue(Request::CopyClips(clips));
                }
            }
            return;
        };
        self.enqueue(if cut {
            Request::CutTimeRange(range)
        } else {
            Request::CopyTimeRange(range)
        });
    }

    pub(super) fn paste_time_selection(&mut self, window: &Window) {
        if self.track_clipboard_shortcut(window, super::track_clipboard::Shortcut::Paste) {
            return;
        }
        self.paste_or_insert_time_selection(false, window);
    }

    pub(super) fn insert_time_selection(&mut self, window: &Window) {
        self.paste_or_insert_time_selection(true, window);
    }

    fn paste_or_insert_time_selection(&mut self, insert: bool, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || self.midi_focus.is_focused(window)
            || !self.focus.is_focused(window)
        {
            return;
        }
        let Some((track, frame)) = self.time_selection_paste_target() else {
            return;
        };
        let request = if insert {
            Request::InsertTimeRange { track, frame }
        } else {
            Request::PasteTimeRange { track, frame }
        };
        // A clip-clipboard paste selects its acknowledged births.
        let intent = super::group_copy::Pending::capture_paste(self, &request);
        let before_pending = self.worker_requests_pending;
        if self.enqueue(request)
            && let Some(mut intent) = intent
        {
            if self.worker_requests_pending > before_pending {
                intent.submitted(self.worker_requests_pending);
            }
            self.pending_group_copy = Some(intent);
        }
    }

    pub(super) fn time_selection_paste_target(&self) -> Option<(TrackId, u64)> {
        self.normalized_time_selection()
            .and_then(|range| {
                range
                    .tracks()
                    .first()
                    .copied()
                    .map(|track| (track, range.start()))
            })
            .or_else(|| {
                self.edit_cursor
                    .and_then(|cursor| Some((cursor.track?, cursor.frame)))
            })
    }

    pub(super) fn delete_time_selection(&mut self, window: &Window) -> bool {
        if self.mode != EditorMode::Arrangement
            || self.midi_focus.is_focused(window)
            || !self.focus.is_focused(window)
        {
            return false;
        }
        let Some(range) = self.normalized_time_selection() else {
            return false;
        };
        if self.enqueue(Request::Edit(Command::DeleteTimeRange { range })) {
            self.cancel_duplicate_chain();
            self.selected_clip = None;
        }
        true
    }

    /// A committed view paints exact copied content; the pending receipt still
    /// owns selection, history and the terminal outcome.
    pub(super) fn observe_time_range_copy_view(
        &mut self,
        observation: hex_session::service::ArrangementEditViewObservation,
    ) -> bool {
        use hex_session::service::ArrangementEditViewKind;
        let Some(pending) = self.pending_time_range_move.as_ref() else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || self.opening_project
            || self.close_prompt_open
            || !matches!(observation.kind, ArrangementEditViewKind::TimeRangeCopy)
            || observation.project_generation != self.project_generation
            || observation.first_previous_revision != observation.previous_revision
        {
            return false;
        }
        let matching = std::iter::once(pending)
            .chain(self.ahead_time_range_move.iter())
            .find(|candidate| {
                candidate.copy_modifier
                    && candidate
                        .keyboard
                        .as_ref()
                        .is_none_or(|keyboard| !keyboard.receipt.is_queued())
                    && candidate.source_revision == observation.previous_revision
            });
        let Some(matching) = matching else {
            return false;
        };
        let predecessor_valid = self
            .state
            .as_ref()
            .is_some_and(|state| state.revision == matching.source_revision)
            || (self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == pending.source_revision)
                && self
                    .committed_time_range_copy_view
                    .as_ref()
                    .is_some_and(|view| view.revision == matching.source_revision));
        if !predecessor_valid {
            return false;
        }
        let Some(state) = observation.state.upgrade() else {
            return false;
        };
        if matching.source_revision.checked_add(1) != Some(state.revision)
            || self
                .committed_time_range_copy_view
                .as_ref()
                .is_some_and(|view| view.revision > state.revision)
        {
            return false;
        }
        self.committed_time_range_copy_view = Some(state);
        self.start_ahead_time_duplicate();
        self.time_range_copy_view().is_some()
    }

    /// Admit at most one successor after the source copy has committed.
    pub(super) fn start_ahead_time_duplicate(&mut self) {
        if self.ahead_time_range_move.is_some()
            || self.time_duplicate_input_blocked()
            || self.worker_requests_pending != 1
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
        {
            return;
        }
        let Some(pending) = self.pending_time_range_move.as_ref() else {
            return;
        };
        let Some(keyboard) = pending.keyboard.as_ref() else {
            return;
        };
        let Some(remaining) = keyboard.repeats.and_then(|repeats| repeats.checked_sub(1)) else {
            return;
        };
        let Some(view) = self.committed_time_range_copy_view.as_ref() else {
            return;
        };
        if pending.source_revision.checked_add(1) != Some(view.revision)
            || self.selection_intent_epoch != Some(pending.selection_intent_epoch)
            || self.time_selection_project_scope != pending.project_scope
            || self.normalized_time_selection().as_ref() != Some(&pending.source)
            || self.mode != EditorMode::Arrangement
        {
            return;
        }
        let source = pending.range.clone();
        let Some(end) = source.end().checked_add(source.duration()) else {
            self.error = Some("duplicated time range would overflow".into());
            return;
        };
        let Ok(range) = hex_project::TimeRange::new(source.tracks().to_vec(), source.end(), end)
        else {
            return;
        };
        let command = Command::CopyTimeRangeTo {
            range: source.clone(),
            track: source.tracks()[0],
            frame: source.end(),
        };
        self.ahead_time_range_move = Some(PendingTimeRangeMove {
            source,
            range,
            project_scope: pending.project_scope,
            destination_first_index: pending.destination_first_index,
            destination_last_index: pending.destination_last_index,
            source_revision: view.revision,
            selection_intent_epoch: pending.selection_intent_epoch,
            copy_modifier: true,
            keyboard: Some(KeyboardTimeCopy {
                receipt: Receipt::new(self.project_generation),
                repeats: Some(remaining),
            }),
        });
        if let Some(keyboard) = self
            .pending_time_range_move
            .as_mut()
            .and_then(|pending| pending.keyboard.as_mut())
        {
            keyboard.repeats = None;
        }
        if !self.enqueue(Request::Edit(command)) {
            self.ahead_time_range_move = None;
        }
    }

    pub(super) fn time_range_copy_view(
        &self,
    ) -> Option<&std::sync::Arc<hex_session::service::ViewState>> {
        let pending = self.pending_time_range_move.as_ref()?;
        let state = self.state.as_ref()?;
        if self.mode != EditorMode::Arrangement
            || self.opening_project
            || self.close_prompt_open
            || state.revision != pending.source_revision
        {
            return None;
        }
        self.committed_time_range_copy_view.as_ref().filter(|view| {
            pending.source_revision.checked_add(1) == Some(view.revision)
                || self.ahead_time_range_move.as_ref().is_some_and(|ahead| {
                    pending.source_revision.checked_add(1) == Some(ahead.source_revision)
                        && ahead.source_revision.checked_add(1) == Some(view.revision)
                })
        })
    }

    pub(super) fn complete_time_duplicate_pair(&mut self, event: &Event) -> u8 {
        if let Some(keyboard) = self
            .ahead_time_range_move
            .as_mut()
            .and_then(|pending| pending.keyboard.as_mut())
        {
            let disposition = keyboard.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete && !event.plugin_parameter_observation,
            );
            if disposition == Disposition::Invalidated {
                self.ahead_time_range_move = None;
            }
        }
        let repeats = self.move_time_selection_complete(event);
        if self.pending_time_range_move.is_none()
            && let Some(mut ahead) = self.ahead_time_range_move.take()
        {
            if (event.error.is_some()
                || self.selection_intent_epoch != Some(ahead.selection_intent_epoch))
                && let Some(keyboard) = &mut ahead.keyboard
            {
                keyboard.repeats = None;
            }
            self.pending_time_range_move = Some(ahead);
        }
        repeats
    }

    pub(super) fn refuse_time_duplicate(&mut self, request: &Request) -> bool {
        let matches_ahead = self.ahead_time_range_move.as_ref().is_some_and(|pending| {
            matches!(request, Request::Edit(Command::CopyTimeRangeTo { range, track, frame })
                if *range == pending.source && pending.range.tracks().first() == Some(track)
                    && pending.range.start() == *frame)
        });
        if matches_ahead {
            self.ahead_time_range_move.take().is_some()
        } else {
            self.pending_time_range_move.take().is_some()
        }
    }

    pub(super) fn move_time_selection_complete(&mut self, event: &Event) -> u8 {
        let Some(mut pending) = self.pending_time_range_move.take() else {
            return 0;
        };
        let mut repeats = 0;
        if let Some(keyboard) = &mut pending.keyboard {
            match keyboard.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete && !event.plugin_parameter_observation,
            ) {
                Disposition::Waiting => {
                    self.pending_time_range_move = Some(pending);
                    return 0;
                }
                Disposition::Invalidated => return 0,
                Disposition::Terminal => {}
            }
            if pending.source_revision.checked_add(1) != Some(event.state.revision) {
                return 0;
            }
            if event.error.is_none() {
                repeats = keyboard.repeats.unwrap_or(0);
            }
        } else if !event.request_complete || event.gesture_request || event.tuning_gesture_request {
            self.pending_time_range_move = Some(pending);
            return 0;
        }
        // The copy/move is admitted with no other pending edit. Its advanced
        // canonical revision witnesses the commit; a later recovery checkpoint
        // can still attach an error to that same completion. Keep that diagnostic,
        // but settle the committed destination. Refused edits do not advance it.
        if event.state.revision <= pending.source_revision {
            return 0;
        }
        // The accepted copy still commits, but its selection intent does not
        // own a later pointer choice or a range the user has cancelled.
        if self.selection_intent_epoch != Some(pending.selection_intent_epoch)
            || self
                .edit_cursor
                .is_none_or(|cursor| cursor.generation != self.project_generation)
            || self.time_selection_project_scope != pending.project_scope
            || self.normalized_time_selection().as_ref() != Some(&pending.source)
        {
            return 0;
        }
        let (first_track, last_track) = if pending.project_scope {
            let Some(first) = event.state.project.tracks().first().map(|track| track.id()) else {
                self.time_selection = None;
                self.time_selection_project_scope = false;
                self.edit_cursor = None;
                return 0;
            };
            let Some(last) = event.state.project.tracks().last().map(|track| track.id()) else {
                self.time_selection = None;
                self.time_selection_project_scope = false;
                self.edit_cursor = None;
                return 0;
            };
            (first, last)
        } else {
            let Some((&first, rest)) = pending.range.tracks().split_first() else {
                return 0;
            };
            (first, rest.last().copied().unwrap_or(first))
        };
        let start_frame = pending.range.start();
        self.time_selection = Some(TimeSelection {
            anchor_track: Some(first_track),
            end_track: Some(last_track),
            anchor_frame: start_frame,
            end_frame: pending.range.end(),
        });
        if let Some(cursor) = &mut self.edit_cursor {
            cursor.track = Some(last_track);
            cursor.frame = start_frame;
        }
        self.time_selection_project_scope = pending.project_scope;
        if !self.time_selection_project_scope {
            self.selected_track = Some(last_track);
        }
        self.selected_clip = None;
        repeats
    }

    pub(super) fn update_time_selection_gesture(&mut self, x: f32, y: f32, free: bool) -> bool {
        let Some(mut gesture) = self.time_selection_gesture else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            self.time_selection_gesture = None;
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || gesture.generation != self.project_generation
            || gesture.revision != state.revision
            || self.clip_name_edit.is_some()
            || self.sections.naming()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.close_prompt_open
            || self.opening_project
            || self.save_dialog_open
            || self.export_dialog_open
        {
            self.time_selection_gesture = None;
            return false;
        }
        gesture.pointer_x = x;
        gesture.pointer_y = y;
        gesture.free_modifier = free;
        let Some((track, lane)) = self.time_gesture_lane_near_y(y) else {
            self.time_selection_gesture = Some(gesture);
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(frame) = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport) else {
            return false;
        };
        let Some(frame) = self.snap_grid.with_platform_modifier(free).snap_frame(
            &state.project,
            frame,
            viewport,
            lane.width,
        ) else {
            return false;
        };
        let next = TimeSelection {
            end_track: track,
            end_frame: frame,
            ..gesture.selection
        };
        if next == gesture.selection {
            self.time_selection_gesture = Some(gesture);
            return false;
        }
        gesture.selection = next;
        gesture.moved = true;
        self.time_selection = Some(next);
        self.time_selection_gesture = Some(gesture);
        if !self.time_selection_project_scope
            && let (Some(anchor), Some(track)) = (next.anchor_track, track)
        {
            self.select_lane_span(anchor, track);
        }
        self.selected_clip = None;
        true
    }

    pub(super) fn finish_time_selection_gesture(&mut self) -> bool {
        let Some(gesture) = self.time_selection_gesture.take() else {
            return false;
        };
        self.suppress_cursor_click = gesture.moved;
        if gesture.selection.anchor_track.is_none()
            && gesture.selection.anchor_frame == gesture.selection.end_frame
        {
            self.time_selection = None;
            self.time_selection_project_scope = false;
            self.edit_cursor = None;
            return true;
        }
        if gesture.moved {
            self.fit_clip_editor_to_time_selection();
        } else {
            self.update_locked_play_start_from_point();
        }
        true
    }

    pub(super) fn cancel_time_range_move_gesture(&mut self) -> bool {
        self.time_range_move_gesture.take().is_some()
    }

    pub(super) fn update_time_range_copy_modifier(&mut self, held: bool) -> bool {
        let Some(gesture) = self.time_range_move_gesture.as_mut() else {
            return false;
        };
        if gesture.copy_modifier == held {
            return false;
        }
        gesture.copy_modifier = held;
        true
    }

    pub(super) fn cancel_time_selection_gesture(&mut self) -> bool {
        let had_move = self.cancel_time_range_move_gesture();
        let had_gesture = self.time_selection_gesture.take().is_some();
        if had_move {
            self.advance_selection_intent();
            self.suppress_cursor_click = false;
            return true;
        }
        let previous = self.time_selection.take();
        let had_range = previous.is_some_and(|selection| {
            selection.anchor_frame != selection.end_frame
                || selection.anchor_track != selection.end_track
        });
        if had_gesture || had_range {
            self.advance_selection_intent();
        }
        if let Some(cursor) = self.edit_cursor
            && let Some(track) = cursor.track
        {
            self.time_selection = Some(TimeSelection {
                anchor_track: Some(track),
                end_track: Some(track),
                anchor_frame: cursor.frame,
                end_frame: cursor.frame,
            });
        } else {
            self.time_selection_project_scope = false;
        }
        self.suppress_cursor_click = false;
        had_move || had_gesture || had_range
    }

    pub(super) fn visible_lane_at(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(TrackId, super::drag::LaneBounds)> {
        let state = self.state.as_ref()?;
        let (track, lane) = self
            .drag_lanes
            .iter()
            .filter(|(track, _)| {
                self.arrangement_track_visible(**track)
                    && self.drag_lane_epochs.get(track) == Some(&self.drag_layout_generation)
            })
            .find(|(_, lane)| lane_contains(x, y, **lane))?;
        state.project.track(*track)?;
        Some((*track, *lane))
    }

    /// Explicit controls win; ordinary headers yield only to double-press painting.
    pub(super) fn clip_handle_at_pointer(
        &self,
        track: TrackId,
        x: f32,
        y: f32,
        viewport: super::viewport::Viewport,
        painting: bool,
    ) -> bool {
        let (ordinary, priority, _, _) =
            self.clip_pointer_handles(track, x, y, viewport, self.option_held, !painting);
        ordinary || priority
    }
    /// Hit the topmost painted clip body using the same bounds as its controls.
    pub(super) fn visible_clip_at_pointer(&self, track: TrackId, x: f32, y: f32) -> Option<ClipId> {
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        self.clip_pointer_handles(track, x, y, viewport, false, true)
            .3
    }

    pub(super) fn pointer_hits_priority_clip_control(&self, x: f32, y: f32, option: bool) -> bool {
        let Some((track, _)) = self.visible_lane_at(x, y) else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        self.clip_pointer_handles(
            track,
            x,
            y,
            self.visible_viewport(state.timeline_frames),
            option,
            true,
        )
        .1
    }
    fn pointer_hits_option_repeat_edge(&self, x: f32, y: f32) -> bool {
        if self.time_selection_key_held {
            return false;
        }
        let Some((track, _)) = self.visible_lane_at(x, y) else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        self.clip_pointer_handles(
            track,
            x,
            y,
            self.visible_viewport(state.timeline_frames),
            true,
            true,
        )
        .2
    }
    /// Ordinary headers/trim edges may yield to a selected time range; visible
    /// fade and loop controls and an eligible Option-repeat edge take priority.
    /// Returns ordinary, priority and Option-repeat hits plus the topmost clip.
    fn clip_pointer_handles(
        &self,
        track: TrackId,
        x: f32,
        y: f32,
        viewport: super::viewport::Viewport,
        option: bool,
        include_header: bool,
    ) -> (bool, bool, bool, Option<ClipId>) {
        let Some(state) = self.state.as_ref() else {
            return (true, false, false, None);
        };
        let Some(track) = state.project.track(track) else {
            return (true, false, false, None);
        };
        let Some(lane) = self.drag_lanes.get(&track.id()) else {
            return (true, false, false, None);
        };
        let Some(viewport_end) = viewport.end().filter(|_| viewport.length > 0) else {
            return (true, false, false, None);
        };
        let handle_hit = |start: u64, end: u64| {
            let visible_start = start.max(viewport.start);
            let visible_end = end.min(viewport_end);
            if visible_start >= visible_end {
                return None;
            }
            let left = lane.left
                + lane.width
                    * ((visible_start - viewport.start) as f64 / viewport.length as f64) as f32;
            let fraction = (visible_end - visible_start) as f64 / viewport.length as f64;
            let width = super::trim::rendered_clip_width(lane.width, fraction);
            if x < left || x >= left + width {
                return None;
            }
            // Edge trim strips only cover the header rail (the whole row when
            // it is header-only); below that the edge is ordinary body.
            let edge = super::trim::handle_width(width);
            let on_edge = (visible_start == start && x < left + edge)
                || (visible_end == end && x >= left + width - edge);
            let in_edge_rail = y < lane.top + super::trim::edge_trim_height(lane.height);
            let right_edge = visible_end == end && x >= left + width - edge && in_edge_rail;
            Some((
                (include_header && y < lane.top + header_hit_height(lane.height))
                    || (on_edge && in_edge_rail),
                right_edge,
            ))
        };
        // MIDI is drawn after audio; later clips cover earlier clips of each kind.
        // A visible body must win over any obscured header or resize edge.
        track
            .midi_clips()
            .iter()
            .rev()
            .find_map(|clip| {
                if !self.arrangement_clip_visible(clip.id()) {
                    return None;
                }
                let preview = state.midi_preview.get(clip.id())?;
                handle_hit(preview.frame_start, preview.frame_end).map(|(ordinary, right_edge)| {
                    let repeat = option
                        && right_edge
                        && super::trim::loop_handles::midi_repeat_available(clip);
                    (
                        ordinary,
                        repeat
                            || super::trim::loop_handles::midi_controls(
                                clip,
                                &state.project,
                                *lane,
                                viewport,
                            )
                            .is_some_and(|c| c.hit(x, y)),
                        repeat,
                        Some(clip.id()),
                    )
                })
            })
            .or_else(|| {
                track.clips().iter().rev().find_map(|clip| {
                    if !self.arrangement_clip_visible(clip.id()) {
                        return None;
                    }
                    let end = clip.start().checked_add(clip.duration())?;
                    handle_hit(clip.start(), end).map(|(ordinary, right_edge)| {
                        let repeat = option
                            && right_edge
                            && super::trim::loop_handles::audio_repeat_available(
                                clip,
                                &state.project,
                            );
                        (
                            ordinary,
                            repeat
                                || self.fade_handle_hit(track.id(), clip, x, y, viewport)
                                || super::trim::loop_handles::audio_controls(
                                    clip,
                                    &state.project,
                                    *lane,
                                    viewport,
                                )
                                .is_some_and(|c| c.hit(x, y)),
                            repeat,
                            Some(clip.id()),
                        )
                    })
                })
            })
            .unwrap_or((false, false, false, None))
    }

    /// Paint one opaque marker at the true dragged endpoint, above the range's
    /// translucent fill. Reuse the loop rail's checked musical-duration law.
    fn time_selection_binary_marker(
        &self,
        selection: TimeSelection,
        viewport: super::viewport::Viewport,
    ) -> Option<Div> {
        if selection.end_frame < viewport.start || selection.end_frame > viewport.end()? {
            return None;
        }
        let region = hex_project::LoopRegion::new(
            selection.anchor_frame.min(selection.end_frame),
            selection.anchor_frame.max(selection.end_frame),
            true,
        )
        .ok()?;
        if !super::loop_rail::binary_duration(&self.state.as_ref()?.project, region) {
            return None;
        }
        let marker = div()
            .debug_selector(|| "time-selection-binary-length-marker".into())
            .absolute()
            .top(px(2.0))
            .size(px(5.0))
            .rounded_full()
            .bg(rgb(theme::color(theme::PaletteRole::Background)));
        Some(if selection.end_frame > selection.anchor_frame {
            marker.right_0()
        } else {
            marker.left_0()
        })
    }

    pub(super) fn time_selection_overlay(
        &self,
        track: TrackId,
        viewport: super::viewport::Viewport,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let cursor_owner = cx.entity();
        let raw_selection = self.time_selection?;
        let state = self.state.as_ref()?;
        state.project.track(track)?;
        let (anchor_index, end_index) = self.time_selection_track_indices()?;
        let track_index = state
            .project
            .tracks()
            .iter()
            .position(|item| item.id() == track)?;
        let (first_index, last_index) = if anchor_index <= end_index {
            (anchor_index, end_index)
        } else {
            (end_index, anchor_index)
        };
        if track_index < first_index || track_index > last_index {
            return None;
        }
        let viewport_end = viewport.end()?;
        let stroke = 1.0 / scale_factor.max(1.0);
        if raw_selection.anchor_frame == raw_selection.end_frame {
            if self.play_start_at_beginning
                || self.displayed_clip_move_guide_frame(viewport).is_some()
            {
                return None;
            }
            let frame = raw_selection.anchor_frame;
            if frame < viewport.start || frame > viewport_end {
                return None;
            }
            let fraction = viewport.fraction(frame)?;
            let color = self
                .edit_cursor
                .map_or(theme::color(theme::PaletteRole::Text), |cursor| {
                    cursor.color()
                });
            return Some(
                div()
                    .id(("edit-cursor-line", track.value()))
                    .debug_selector(|| format!("edit-cursor-line-{}", track.value()))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(fraction))
                    .w(px(stroke))
                    .bg(rgb(color)),
            );
        }
        let visible_start = raw_selection
            .anchor_frame
            .min(raw_selection.end_frame)
            .max(viewport.start);
        let visible_end = raw_selection
            .anchor_frame
            .max(raw_selection.end_frame)
            .min(viewport_end);
        if visible_start >= visible_end {
            return None;
        }
        let left =
            (visible_start.saturating_sub(viewport.start)) as f64 / viewport.length.max(1) as f64;
        let width =
            visible_end.saturating_sub(visible_start) as f64 / viewport.length.max(1) as f64;
        Some(
            div()
                .id(("edit-time-selection", track.value()))
                .debug_selector(|| format!("edit-time-selection-{}", track.value()))
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(left as f32))
                .w(relative(width as f32))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(rgb(theme::time_selection_fill()))
                        .opacity(theme::TIME_SELECTION_OPACITY)
                        .child(
                            div()
                                .id(("time-selection-handle", track.value()))
                                .debug_selector(|| {
                                    format!("time-selection-handle-{}", track.value())
                                })
                                .absolute()
                                .top_0()
                                .w_full()
                                .h(px(header_hit_height(self.track_height(track))))
                                .when(!self.option_held, |handle| {
                                    handle.cursor(CursorStyle::OpenHand)
                                })
                                .children(self.option_held.then(|| {
                                    canvas(
                                        |bounds, window, _| {
                                            window
                                                .insert_hitbox(bounds, gpui::HitboxBehavior::Normal)
                                        },
                                        move |_, hitbox, window, cx| {
                                            if hitbox.is_hovered(window) {
                                                let position = window.mouse_position();
                                                let repeat = cursor_owner
                                                    .read(cx)
                                                    .pointer_hits_option_repeat_edge(
                                                        f32::from(position.x),
                                                        f32::from(position.y),
                                                    );
                                                window.set_cursor_style(
                                                    if repeat {
                                                        CursorStyle::Loop
                                                    } else {
                                                        CursorStyle::OpenHand
                                                    },
                                                    &hitbox,
                                                );
                                            }
                                        },
                                    )
                                    .absolute()
                                    .inset_0()
                                }))
                                .hover(|style| {
                                    style.bg(rgb(theme::color(theme::PaletteRole::Text)))
                                }),
                        ),
                )
                .children(
                    if track_index == first_index && !self.time_selection_project_scope {
                        self.time_selection_binary_marker(raw_selection, viewport)
                    } else {
                        None
                    },
                ),
        )
    }

    pub(super) fn main_time_selection_overlay(
        &self,
        viewport: super::viewport::Viewport,
    ) -> Option<Stateful<Div>> {
        if !self.time_selection_project_scope {
            return None;
        }
        let selection = self.time_selection?;
        let viewport_end = viewport.end()?;
        if selection.anchor_frame == selection.end_frame {
            if self.play_start_at_beginning
                || self.displayed_clip_move_guide_frame(viewport).is_some()
            {
                return None;
            }
            let frame = selection.anchor_frame;
            if frame < viewport.start || frame > viewport_end {
                return None;
            }
            let fraction = viewport.fraction(frame)?;
            let color = self
                .edit_cursor
                .map_or(theme::color(theme::PaletteRole::Text), |cursor| {
                    cursor.color()
                });
            return Some(
                div()
                    .id("main-project-edit-cursor")
                    .debug_selector(|| "main-project-edit-cursor".to_owned())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(fraction))
                    .w(px(1.0))
                    .bg(rgb(color)),
            );
        }
        let visible_start = selection
            .anchor_frame
            .min(selection.end_frame)
            .max(viewport.start);
        let visible_end = selection
            .anchor_frame
            .max(selection.end_frame)
            .min(viewport_end);
        if visible_start >= visible_end {
            return None;
        }
        let left =
            (visible_start.saturating_sub(viewport.start)) as f64 / viewport.length.max(1) as f64;
        let width =
            visible_end.saturating_sub(visible_start) as f64 / viewport.length.max(1) as f64;
        Some(
            div()
                .id("main-project-time-selection")
                .debug_selector(|| "main-project-time-selection".to_owned())
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(left as f32))
                .w(relative(width as f32))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(rgb(theme::time_selection_fill()))
                        .opacity(theme::TIME_SELECTION_OPACITY)
                        .child(
                            div()
                                .id("main-project-time-selection-handle")
                                .debug_selector(|| "main-project-time-selection-handle".to_owned())
                                .absolute()
                                .top_0()
                                .w_full()
                                .h(relative(SELECTION_HANDLE_FRACTION))
                                .cursor(CursorStyle::OpenHand)
                                .hover(|style| {
                                    style.bg(rgb(theme::color(theme::PaletteRole::Text)))
                                }),
                        ),
                )
                .children(self.time_selection_binary_marker(selection, viewport)),
        )
    }

    pub(super) fn time_range_move_preview_overlay(
        &self,
        track: TrackId,
        track_index: usize,
        viewport: super::viewport::Viewport,
    ) -> Option<Stateful<Div>> {
        let display = self.time_range_move_display()?;
        let preview = display;
        if track_index < preview.destination_first_index
            || track_index > preview.destination_last_index
        {
            return None;
        }
        let viewport_end = viewport.end()?;
        let visible_start = preview.destination_start.max(viewport.start);
        let visible_end = preview.destination_end.min(viewport_end);
        if visible_start >= visible_end {
            return None;
        }
        let left =
            visible_start.saturating_sub(viewport.start) as f64 / viewport.length.max(1) as f64;
        let width =
            visible_end.saturating_sub(visible_start) as f64 / viewport.length.max(1) as f64;
        Some(
            div()
                .id(("time-selection-move-preview", track.value()))
                .debug_selector(|| format!("time-selection-move-preview-{}", track.value()))
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(left as f32))
                .w(relative(width as f32))
                .bg(rgb(theme::time_selection_fill()))
                .opacity(theme::TIME_SELECTION_PREVIEW_OPACITY)
                .cursor(if preview.copy_modifier {
                    CursorStyle::DragCopy
                } else {
                    CursorStyle::Arrow
                }),
        )
    }

    pub(super) fn time_range_move_display(&self) -> Option<TimeRangeMoveDisplay<'_>> {
        let state = self.state.as_ref()?;
        let (source, destination_first_index, destination_last_index, start, end, copy_modifier) =
            if let Some(pending) = &self.pending_time_range_move {
                (
                    &pending.source,
                    pending.destination_first_index,
                    pending.destination_last_index,
                    pending.range.start(),
                    pending.range.end(),
                    pending.copy_modifier,
                )
            } else {
                let gesture = self.time_range_move_gesture.as_ref()?;
                if gesture.generation != self.project_generation
                    || gesture.revision != state.revision
                {
                    return None;
                }
                let preview = gesture.preview.as_ref()?;
                if gesture.range.tracks().first() == Some(&preview.first_track)
                    && gesture.range.tracks().last() == Some(&preview.last_track)
                    && gesture.range.start() == preview.start_frame
                {
                    return None;
                }
                (
                    &gesture.range,
                    preview.first_index,
                    preview.last_index,
                    preview.start_frame,
                    preview.end_frame,
                    gesture.copy_modifier,
                )
            };
        let same_rows = state
            .project
            .tracks()
            .get(destination_first_index..=destination_last_index)
            .is_some_and(|rows| {
                rows.iter()
                    .map(|row| row.id())
                    .eq(source.tracks().iter().copied())
            });
        if same_rows && source.start() == start {
            return None;
        }
        Some(TimeRangeMoveDisplay {
            source,
            destination_first_index,
            destination_last_index,
            destination_start: start,
            destination_end: end,
            copy_modifier,
        })
    }
}

fn lane_contains(x: f32, y: f32, lane: super::drag::LaneBounds) -> bool {
    x.is_finite()
        && y.is_finite()
        && lane.left.is_finite()
        && lane.top.is_finite()
        && lane.width.is_finite()
        && lane.height.is_finite()
        && lane.width > 0.0
        && lane.height > 0.0
        && x >= lane.left
        && x < lane.left + lane.width
        && y >= lane.top
        && y < lane.top + lane.height - 1.0
}

#[cfg(all(test, feature = "ui-test-support"))]
mod duplicate_tests;

#[cfg(all(test, feature = "ui-test-support"))]
mod retained_duplicate_tests;

#[cfg(all(test, feature = "ui-test-support"))]
mod shift_click_tests;
