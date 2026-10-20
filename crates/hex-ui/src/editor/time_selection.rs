//! Arrangement time selection, range movement and worker clipboard routing.
use super::edit_cursor::EditCursor;
use super::{EditorMode, EditorView};
use crate::theme;
use gpui::{CursorStyle, Div, Stateful, Window, div, prelude::*, px, relative, rgb};
use hex_project::{Command, TrackId};
use hex_session::service::Request;
use hex_session::snapshot_exports::{SnapshotProjectRead, SnapshotTransportError};
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
    pub copy_modifier: bool,
}

fn selection_indices(
    project: SnapshotProjectRead<'_>,
    selection: TimeSelection,
    project_scope: bool,
) -> Option<(usize, usize)> {
    if project_scope {
        return Some((0, project.tracks().len().checked_sub(1)?));
    }
    let anchor = project
        .tracks()
        .position(|track| track.id() == selection.anchor_track)?;
    let end = project
        .tracks()
        .position(|track| track.id() == selection.end_track)?;
    Some((anchor.min(end), anchor.max(end)))
}
fn selection_range(
    project: SnapshotProjectRead<'_>,
    selection: TimeSelection,
    project_scope: bool,
) -> Option<hex_project::TimeRange> {
    if selection.anchor_frame == selection.end_frame {
        return None;
    }
    let (first, last) = selection_indices(project, selection, project_scope)?;
    let tracks = project
        .tracks()
        .skip(first)
        .take(last - first + 1)
        .map(|track| track.id())
        .collect();
    hex_project::TimeRange::new(
        tracks,
        selection.anchor_frame.min(selection.end_frame),
        selection.anchor_frame.max(selection.end_frame),
    )
    .ok()
}

impl EditorView {
    pub(super) fn normalized_time_selection(&self) -> Option<hex_project::TimeRange> {
        self.normalized_time_selection_checked().ok().flatten()
    }

    pub(super) fn normalized_time_selection_checked(
        &self,
    ) -> Result<Option<hex_project::TimeRange>, SnapshotTransportError> {
        let (Some(_), Some(state)) = (self.time_selection, self.state.as_ref()) else {
            return Ok(None);
        };
        state.read_with(|snapshot| self.normalized_time_selection_in(snapshot.model()))
    }

    pub(super) fn normalized_time_selection_in(
        &self,
        project: SnapshotProjectRead<'_>,
    ) -> Option<hex_project::TimeRange> {
        selection_range(
            project,
            self.time_selection?,
            self.time_selection_project_scope,
        )
    }

    pub(super) fn begin_main_time_selection_gesture(&mut self, x: f32, y: f32, free: bool) -> bool {
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
        let Some(lane) = self.main_track_lane_bounds.filter(|lane| {
            self.main_track_lane_epoch == self.drag_layout_generation && lane_contains(x, y, *lane)
        }) else {
            return false;
        };
        let Ok(Some((first_track, last_track, frame, revision))) = state.read_with(|snapshot| {
            let project = snapshot.model();
            let first = project.tracks().next()?.id();
            let last = project.tracks().last()?.id();
            let viewport = self.visible_viewport(snapshot, snapshot.timeline_frames());
            let frame = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport)?;
            let frame = self
                .snap_grid
                .with_platform_modifier(free)
                .snap_frame(&project, frame, viewport, lane.width)?;
            Some((first, last, frame, snapshot.revision()))
        }) else {
            return false;
        };
        let generation = self.project_generation;
        let selection = TimeSelection {
            anchor_track: first_track,
            end_track: last_track,
            anchor_frame: frame,
            end_frame: frame,
        };
        self.time_selection_project_scope = true;
        self.suppress_cursor_click = false;
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
        self.selected_clip = None;
        true
    }

    pub(super) fn begin_main_time_range_move_gesture(&mut self, x: f32, y: f32) -> bool {
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
        let Ok(Some(range)) = self.normalized_time_selection_checked() else {
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
        let Ok((viewport, revision)) = state.read_with(|snapshot| {
            (
                self.visible_viewport(snapshot, snapshot.timeline_frames()),
                snapshot.revision(),
            )
        }) else {
            return false;
        };
        let Some(frame) = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport) else {
            return false;
        };
        if frame < range.start() || frame >= range.end() {
            return false;
        }
        let Some(grab_frame_offset) = frame.checked_sub(range.start()) else {
            return false;
        };
        self.suppress_cursor_click = false;
        self.time_range_move_gesture = Some(TimeRangeMoveGesture {
            range,
            generation: self.project_generation,
            revision,
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
        let Ok((viewport, revision)) = state.read_with(|snapshot| {
            (
                self.visible_viewport(snapshot, snapshot.timeline_frames()),
                snapshot.revision(),
            )
        }) else {
            return false;
        };
        let Some(frame) = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport) else {
            return false;
        };
        let Ok(range) = self.normalized_time_selection_checked() else {
            return false;
        };
        let inside_selection = range.is_some_and(|range| {
            range.tracks().contains(&track) && frame >= range.start() && frame < range.end()
        });
        let Ok(Some(frame)) = state.read_with(|snapshot| {
            self.snap_grid.with_platform_modifier(free).snap_frame(
                &snapshot.model(),
                frame,
                viewport,
                lane.width,
            )
        }) else {
            return false;
        };
        if !inside_selection && self.clip_handle_at_pointer(track, x, y, viewport) {
            return false;
        }
        let generation = self.project_generation;
        let selection = TimeSelection {
            anchor_track: track,
            end_track: track,
            anchor_frame: frame,
            end_frame: frame,
        };
        self.suppress_cursor_click = false;
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
        self.selected_track = Some(track);
        self.selected_clip = None;
        self.cancel_midi_note_gesture();
        self.device_panel_active = true;
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
        let Ok(Some(range)) = self.normalized_time_selection_checked() else {
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
        let Ok((viewport, revision)) = state.read_with(|snapshot| {
            (
                self.visible_viewport(snapshot, snapshot.timeline_frames()),
                snapshot.revision(),
            )
        }) else {
            return false;
        };
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
            revision,
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
        let Some(mut gesture) = self.time_range_move_gesture.clone() else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            self.time_range_move_gesture = None;
            return false;
        };
        let Ok(revision) = state.read_with(|snapshot| snapshot.revision()) else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || gesture.generation != self.project_generation
            || gesture.revision != revision
            || self.clip_name_edit.is_some()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.close_prompt_open
            || self.opening_project
            || self.save_dialog_open
            || self.export_dialog_open
        {
            self.time_range_move_gesture = None;
            self.suppress_cursor_click = true;
            return true;
        }
        let lane = self.time_gesture_lane_near_y(y);
        let Ok(preview) = state.read_with(|snapshot| {
            let Some((pointer_track, lane)) = lane else {
                return None;
            };
            let project = snapshot.model();
            let pointer_index = project
                .tracks()
                .position(|track| track.id() == pointer_track)?;
            let row_count = gesture.range.tracks().len();
            let max_first = project.tracks().len().saturating_sub(row_count);
            let first_index = pointer_index
                .saturating_sub(gesture.grab_track_offset)
                .min(max_first);
            let last_index = first_index.checked_add(row_count.checked_sub(1)?)?;
            let first_track = project.tracks().nth(first_index)?.id();
            let last_track = project.tracks().nth(last_index)?.id();
            let viewport = self.visible_viewport(snapshot, snapshot.timeline_frames());
            let pointer_frame = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport)?;
            let candidate_frame = self.snap_grid.with_platform_modifier(free).snap_frame(
                &project,
                pointer_frame.saturating_sub(gesture.grab_frame_offset),
                viewport,
                lane.width,
            )?;
            let end_frame = candidate_frame.checked_add(gesture.range.duration())?;
            Some(TimeRangeMovePreview {
                first_track,
                last_track,
                first_index,
                last_index,
                start_frame: candidate_frame,
                end_frame,
            })
        }) else {
            return false;
        };
        self.suppress_cursor_click = true;
        gesture.pointer_x = x;
        gesture.pointer_y = y;
        gesture.free_modifier = free;
        // An invalid projection keeps the previous preview, as it did before this reader port.
        if let Some(preview) = preview {
            gesture.preview = Some(preview);
        }
        self.time_range_move_gesture = Some(gesture);
        true
    }

    pub(super) fn finish_time_range_move_gesture(&mut self) -> bool {
        let Some(gesture) = self.time_range_move_gesture.as_ref() else {
            return false;
        };
        let facts = match self.state.as_ref() {
            Some(state) => state.read_with(|snapshot| {
                let project = snapshot.model();
                let destination = gesture.preview.as_ref().and_then(|preview| {
                    let first = project
                        .tracks()
                        .position(|track| track.id() == preview.first_track)?;
                    let last = project
                        .tracks()
                        .position(|track| track.id() == preview.last_track)?;
                    let count = last.checked_sub(first)?.checked_add(1)?;
                    let rows = project
                        .tracks()
                        .skip(first)
                        .take(count)
                        .map(|track| track.id())
                        .collect();
                    let range =
                        hex_project::TimeRange::new(rows, preview.start_frame, preview.end_frame)
                            .ok()?;
                    Some((first, last, range))
                });
                (snapshot.revision() == gesture.revision, destination)
            }),
            None => Ok((false, None)),
        };
        let Ok((current, destination)) = facts else {
            return false;
        };
        let Some(gesture) = self.time_range_move_gesture.take() else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || gesture.generation != self.project_generation
            || !current
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
        let Some((first, last, destination)) = destination else {
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
        if self.enqueue(Request::Edit(command)) {
            self.pending_time_range_move = Some(PendingTimeRangeMove {
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
        self.state
            .as_ref()?
            .read_with(|snapshot| {
                selection_indices(
                    snapshot.model(),
                    selection,
                    self.time_selection_project_scope,
                )
            })
            .ok()
            .flatten()
    }

    pub(super) fn copy_time_selection(&mut self, cut: bool, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || self.midi_focus.is_focused(window)
            || !self.focus.is_focused(window)
        {
            return;
        }
        let range = match self.normalized_time_selection_checked() {
            Ok(range) => range,
            Err(_) => return,
        };
        let Some(range) = range else {
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
        self.normalized_time_selection_checked()
            .ok()?
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
        let Ok(Some(range)) = self.normalized_time_selection_checked() else {
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
        if event.error.is_some() || event.state.revision <= pending.source_revision {
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
        let Ok(revision) = state.read_with(|snapshot| snapshot.revision()) else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || gesture.generation != self.project_generation
            || gesture.revision != revision
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
        let Ok(next) = state.read_with(|snapshot| {
            let project = snapshot.model();
            let viewport = self.visible_viewport(snapshot, snapshot.timeline_frames());
            let frame = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport)?;
            let frame = self
                .snap_grid
                .with_platform_modifier(free)
                .snap_frame(&project, frame, viewport, lane.width)?;
            Some(TimeSelection {
                end_track: if self.time_selection_project_scope {
                    project.tracks().last().map_or(track, |item| item.id())
                } else {
                    track
                },
                end_frame: frame,
                ..gesture.selection
            })
        }) else {
            return false;
        };
        let Some(next) = next else {
            return false;
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
            self.suppress_cursor_click = false;
            return true;
        }
        let previous = self.time_selection.take();
        let had_range = previous.is_some_and(|selection| {
            selection.anchor_frame != selection.end_frame
                || selection.anchor_track != selection.end_track
        });
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
        self.state
            .as_ref()?
            .read_with(|snapshot| self.visible_lane_at_in(snapshot.model(), x, y))
            .ok()
            .flatten()
    }
    pub(super) fn visible_lane_at_in(
        &self,
        project: SnapshotProjectRead<'_>,
        x: f32,
        y: f32,
    ) -> Option<(TrackId, super::drag::LaneBounds)> {
        let (track, lane) = self
            .drag_lanes
            .iter()
            .filter(|(track, _)| {
                self.drag_lane_epochs.get(track) == Some(&self.drag_layout_generation)
            })
            .find(|(_, lane)| lane_contains(x, y, **lane))?;
        project.track(*track)?;
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
        let Ok(viewport) =
            state.read_with(|snapshot| self.visible_viewport(snapshot, snapshot.timeline_frames()))
        else {
            return false;
        };
        self.clip_pointer_handles(track, x, y, viewport).1
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
        let (track_index, first_index, last_index) = state
            .read_with(|snapshot| {
                let project = snapshot.model();
                let (first, last) =
                    selection_indices(project, raw_selection, self.time_selection_project_scope)?;
                let index = project.tracks().position(|item| item.id() == track)?;
                Some((index, first, last))
            })
            .ok()
            .flatten()?;
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
                .opacity(0.13)
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
                .opacity(0.13)
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
        let revision = state.read_with(|snapshot| snapshot.revision()).ok()?;
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
                if gesture.generation != self.project_generation || gesture.revision != revision {
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
            .read_with(|snapshot| {
                let project = snapshot.model();
                let count = destination_last_index
                    .checked_sub(destination_first_index)?
                    .checked_add(1)?;
                (destination_last_index < project.tracks().len()).then(|| {
                    project
                        .tracks()
                        .skip(destination_first_index)
                        .take(count)
                        .map(|track| track.id())
                        .eq(source.tracks().iter().copied())
                })
            })
            .ok()
            .flatten()?;
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
