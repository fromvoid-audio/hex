//! Arrangement time selection, range movement and worker clipboard routing.
use super::edit_cursor::EditCursor;
use super::{EditorMode, EditorView};
use crate::theme;
use gpui::{CursorStyle, Div, Stateful, Window, div, prelude::*, px, relative, rgb};
use hex_project::{Command, TrackId};
use hex_session::service::Request;
use std::time::Instant;

pub(super) const SELECTION_HANDLE_FRACTION: f32 = 1.0 / 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TimeSelection {
    pub(super) anchor_track: TrackId,
    pub(super) end_track: TrackId,
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

#[derive(Clone, Debug)]
pub(super) struct PendingTimeRangeMove {
    pub source: hex_project::TimeRange,
    pub range: hex_project::TimeRange,
    pub project_scope: bool,
    pub destination_first_index: usize,
    pub destination_last_index: usize,
    pub source_revision: u64,
    pub selection_intent_epoch: u64,
    pub copy_modifier: bool,
}

impl EditorView {
    /// Actual selection choices supersede pending ACK side effects, even when
    /// the user returns to identical coordinates. Exhaustion stays invalid for
    /// this view's lifetime; never reuse an epoch or cancel the canonical edit.
    pub(super) fn advance_selection_intent(&mut self) {
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

    pub(super) fn begin_main_time_selection_gesture(&mut self, x: f32, y: f32, free: bool) -> bool {
        self.suppress_cursor_click = false;
        if self.mode != EditorMode::Arrangement
            || self.clip_name_edit.is_some()
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
        let Some(first_track) = state.project.tracks().first().map(|track| track.id()) else {
            // An empty project has no valid whole-project time range.
            return false;
        };
        let Some(last_track) = state.project.tracks().last().map(|track| track.id()) else {
            return false;
        };
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
        let generation = self.project_generation;
        let revision = state.revision;
        let selection = TimeSelection {
            anchor_track: first_track,
            end_track: last_track,
            anchor_frame: frame,
            end_frame: frame,
        };
        self.advance_selection_intent();
        self.time_selection_project_scope = true;
        self.edit_cursor = Some(EditCursor {
            track: first_track,
            frame,
            generation,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: Instant::now(),
        });
        self.time_selection = Some(selection);
        self.time_selection_gesture = Some(TimeSelectionGesture {
            selection,
            generation,
            revision,
            moved: false,
            pointer_x: x,
            pointer_y: y,
            free_modifier: free,
            last_tick: Instant::now(),
        });
        self.retain_clip_detail_for_time_selection();
        self.selected_clip = None;
        true
    }

    pub(super) fn begin_main_time_range_move_gesture(&mut self, x: f32, y: f32) -> bool {
        self.suppress_cursor_click = false;
        if !self.time_selection_project_scope
            || self.mode != EditorMode::Arrangement
            || self.edit_pending()
            || self.clip_name_edit.is_some()
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
        let Some(lane) = self.main_track_lane_bounds.filter(|lane| {
            self.main_track_lane_epoch == self.drag_layout_generation && lane_contains(x, y, *lane)
        }) else {
            return false;
        };
        if y >= lane.top + lane.height * SELECTION_HANDLE_FRACTION {
            return false;
        }
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

    pub(super) fn begin_time_selection_gesture(&mut self, x: f32, y: f32, free: bool) -> bool {
        self.suppress_cursor_click = false;
        if self.mode != EditorMode::Arrangement
            || self.clip_name_edit.is_some()
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
        if !inside_selection && self.clip_handle_at_pointer(track, x, y, viewport) {
            return false;
        }
        let generation = self.project_generation;
        let revision = state.revision;
        let selection = TimeSelection {
            anchor_track: track,
            end_track: track,
            anchor_frame: frame,
            end_frame: frame,
        };
        self.advance_selection_intent();
        self.edit_cursor = Some(EditCursor {
            track,
            frame,
            generation,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: Instant::now(),
        });
        self.time_selection = Some(selection);
        self.time_selection_project_scope = false;
        self.time_selection_gesture = Some(TimeSelectionGesture {
            selection,
            generation,
            revision,
            moved: false,
            pointer_x: x,
            pointer_y: y,
            free_modifier: free,
            last_tick: Instant::now(),
        });
        self.retain_clip_detail_for_time_selection();
        self.selected_track = Some(track);
        self.selected_clip = None;
        self.cancel_midi_note_gesture();
        true
    }

    pub(super) fn begin_time_range_move_gesture(&mut self, x: f32, y: f32) -> bool {
        if self.mode != EditorMode::Arrangement
            || self.edit_pending()
            || self.clip_name_edit.is_some()
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
        // Only the upper third is a move handle; the rest always reselects.
        if y >= lane.top + lane.height * SELECTION_HANDLE_FRACTION {
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
        let Some((pointer_track, lane)) = self.time_gesture_lane_near_y(y) else {
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
        let anchor = tracks
            .iter()
            .position(|track| track.id() == selection.anchor_track)?;
        let end = tracks
            .iter()
            .position(|track| track.id() == selection.end_track)?;
        Some(if anchor <= end {
            (anchor, end)
        } else {
            (end, anchor)
        })
    }

    /// Duplicate the bounded Arrangement rectangle through the existing atomic
    /// copy edit. Normal overwrite semantics match Option-drag range copying.
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
        if self.edit_pending()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.clip_name_edit.is_some()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.save_dialog_open
            || self.export_dialog_open
        {
            self.error = Some("finish the current edit before duplicating time".into());
            return true;
        }
        let Some(end) = range.end().checked_add(range.duration()) else {
            self.error = Some("duplicated time range would overflow".into());
            return true;
        };
        let Some((first, last)) = self.time_selection_track_indices() else {
            return true;
        };
        let Ok(destination) =
            hex_project::TimeRange::new(range.tracks().to_vec(), range.end(), end)
        else {
            return true;
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
            return true;
        };
        if self.enqueue(Request::Edit(command)) {
            self.pending_time_range_move = Some(PendingTimeRangeMove {
                selection_intent_epoch,
                source: range,
                range: destination,
                project_scope: self.time_selection_project_scope,
                destination_first_index: first,
                destination_last_index: last,
                source_revision: revision,
                copy_modifier: true,
            });
        }
        true
    }

    pub(super) fn copy_time_selection(&mut self, cut: bool, window: &Window) {
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
        self.enqueue(if insert {
            Request::InsertTimeRange { track, frame }
        } else {
            Request::PasteTimeRange { track, frame }
        });
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
            .or_else(|| self.edit_cursor.map(|cursor| (cursor.track, cursor.frame)))
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

    pub(super) fn move_time_selection_complete(&mut self, event: &hex_session::service::Event) {
        let Some(pending) = self.pending_time_range_move.take() else {
            return;
        };
        if !event.request_complete {
            self.pending_time_range_move = Some(pending);
            return;
        }
        // The copy/move is admitted with no other pending edit. Its advanced
        // canonical revision witnesses the commit; a later recovery checkpoint
        // can still attach an error to that same completion. Keep that diagnostic,
        // but settle the committed destination. Refused edits do not advance it.
        if event.state.revision <= pending.source_revision {
            return;
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
            return;
        }
        let (first_track, last_track) = if pending.project_scope {
            let Some(first) = event.state.project.tracks().first().map(|track| track.id()) else {
                self.time_selection = None;
                self.time_selection_project_scope = false;
                self.edit_cursor = None;
                return;
            };
            let Some(last) = event.state.project.tracks().last().map(|track| track.id()) else {
                self.time_selection = None;
                self.time_selection_project_scope = false;
                self.edit_cursor = None;
                return;
            };
            (first, last)
        } else {
            let Some((&first, rest)) = pending.range.tracks().split_first() else {
                return;
            };
            (first, rest.last().copied().unwrap_or(first))
        };
        let start_frame = pending.range.start();
        self.time_selection = Some(TimeSelection {
            anchor_track: first_track,
            end_track: last_track,
            anchor_frame: start_frame,
            end_frame: pending.range.end(),
        });
        if let Some(cursor) = &mut self.edit_cursor {
            cursor.track = last_track;
            cursor.frame = start_frame;
        }
        self.time_selection_project_scope = pending.project_scope;
        if !self.time_selection_project_scope {
            self.selected_track = Some(last_track);
        }
        self.selected_clip = None;
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
            end_track: if self.time_selection_project_scope {
                state
                    .project
                    .tracks()
                    .last()
                    .map_or(track, |item| item.id())
            } else {
                track
            },
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
        if !self.time_selection_project_scope {
            self.selected_track = Some(track);
        }
        self.selected_clip = None;
        true
    }

    pub(super) fn finish_time_selection_gesture(&mut self) -> bool {
        let Some(gesture) = self.time_selection_gesture.take() else {
            return false;
        };
        self.suppress_cursor_click = gesture.moved;
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
        if let Some(cursor) = self.edit_cursor {
            self.time_selection = Some(TimeSelection {
                anchor_track: cursor.track,
                end_track: cursor.track,
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
                self.drag_lane_epochs.get(track) == Some(&self.drag_layout_generation)
            })
            .find(|(_, lane)| lane_contains(x, y, **lane))?;
        state.project.track(*track)?;
        Some((*track, *lane))
    }

    /// Reserve clip headers and visible edge trims; bodies share empty-lane input.
    pub(super) fn clip_handle_at_pointer(
        &self,
        track: TrackId,
        x: f32,
        y: f32,
        viewport: super::viewport::Viewport,
    ) -> bool {
        let (ordinary, fade) = self.clip_pointer_handles(track, x, y, viewport);
        ordinary || fade
    }
    pub(super) fn pointer_hits_fade_control(&self, x: f32, y: f32) -> bool {
        let Some((track, _)) = self.visible_lane_at(x, y) else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        self.clip_pointer_handles(track, x, y, self.visible_viewport(state.timeline_frames))
            .1
    }
    fn clip_pointer_handles(
        &self,
        track: TrackId,
        x: f32,
        y: f32,
        viewport: super::viewport::Viewport,
    ) -> (bool, bool) {
        let Some(state) = self.state.as_ref() else {
            return (true, false);
        };
        let Some(track) = state.project.track(track) else {
            return (true, false);
        };
        let Some(lane) = self.drag_lanes.get(&track.id()) else {
            return (true, false);
        };
        let Some(viewport_end) = viewport.end().filter(|_| viewport.length > 0) else {
            return (true, false);
        };
        let handle_hit = |start: u64, end: u64, minimum: f32| {
            let visible_start = start.max(viewport.start);
            let visible_end = end.min(viewport_end);
            if visible_start >= visible_end {
                return None;
            }
            let left = lane.left
                + lane.width
                    * ((visible_start - viewport.start) as f64 / viewport.length as f64) as f32;
            let fraction = (visible_end - visible_start) as f64 / viewport.length as f64;
            let width = super::trim::rendered_clip_width(lane.width, fraction, minimum);
            if x < left || x >= left + width {
                return None;
            }
            let edge = super::trim::handle_width(width);
            Some(
                y < lane.top + lane.height * SELECTION_HANDLE_FRACTION
                    || (visible_start == start && x < left + edge)
                    || (visible_end == end && x >= left + width - edge),
            )
        };
        // MIDI is drawn after audio; later clips cover earlier clips of each kind.
        // A visible body must win over any obscured header or resize edge.
        track
            .midi_clips()
            .iter()
            .rev()
            .find_map(|clip| {
                let preview = state.midi_preview.get(clip.id())?;
                handle_hit(preview.frame_start, preview.frame_end, 4.0)
                    .map(|ordinary| (ordinary, false))
            })
            .or_else(|| {
                track.clips().iter().rev().find_map(|clip| {
                    let end = clip.start().checked_add(clip.duration())?;
                    handle_hit(clip.start(), end, 3.0).map(|ordinary| {
                        (
                            ordinary,
                            self.fade_handle_hit(state, track.id(), clip, x, y, viewport),
                        )
                    })
                })
            })
            .unwrap_or((false, false))
    }

    pub(super) fn time_selection_overlay(
        &self,
        track: TrackId,
        viewport: super::viewport::Viewport,
        scale_factor: f32,
    ) -> Option<Stateful<Div>> {
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
                .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                .opacity(theme::TIME_SELECTION_OPACITY)
                .child(
                    div()
                        .id(("time-selection-handle", track.value()))
                        .debug_selector(|| format!("time-selection-handle-{}", track.value()))
                        .absolute()
                        .top_0()
                        .w_full()
                        .h(relative(SELECTION_HANDLE_FRACTION))
                        .cursor(CursorStyle::OpenHand)
                        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Text)))),
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
                .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
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
                        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Text)))),
                ),
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
                .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                .opacity(0.12)
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
