//! Arrangement time selection, range movement and worker clipboard routing.
use super::edit_cursor::EditCursor;
use super::{EditorMode, EditorView};
use crate::theme::Palette;
use gpui::{CursorStyle, Div, Stateful, Window, div, prelude::*, px, relative, rgb};
use hex_project::{Command, TrackId};
use hex_session::service::Request;
use std::time::Instant;

const SELECTION_HANDLE_FRACTION: f32 = 1.0 / 3.0;

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
    pub destination_first_index: usize,
    pub destination_last_index: usize,
    pub source_revision: u64,
    pub copy_modifier: bool,
}

impl EditorView {
    pub(super) fn normalized_time_selection(&self) -> Option<hex_project::TimeRange> {
        let selection = self.time_selection?;
        if selection.anchor_frame == selection.end_frame {
            return None;
        }
        let state = self.state.as_ref()?;
        let (first, last) = self.time_selection_track_indices()?;
        hex_project::TimeRange::new(
            state.project.tracks()[first..=last]
                .iter()
                .map(|track| track.id())
                .collect(),
            selection.anchor_frame.min(selection.end_frame),
            selection.anchor_frame.max(selection.end_frame),
        )
        .ok()
    }

    pub(super) fn begin_time_selection_gesture(&mut self, x: f32, y: f32, free: bool) -> bool {
        self.suppress_cursor_click = false;
        if self.mode != EditorMode::Arrangement
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_trim_drag.is_some()
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
        if !inside_selection && self.track_has_clip_at_frame(track, frame, viewport) {
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
        self.edit_cursor = Some(EditCursor {
            track,
            frame,
            generation,
            pulse_color: Palette::TEXT,
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
        self.selected_track = Some(track);
        self.selected_clip = None;
        true
    }

    pub(super) fn begin_time_range_move_gesture(&mut self, x: f32, y: f32) -> bool {
        if self.mode != EditorMode::Arrangement
            || self.edit_pending()
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_trim_drag.is_some()
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
        let Some((pointer_track, lane)) = self.visible_lane_near_y(y) else {
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
                destination_first_index: first,
                destination_last_index: last,
                source_revision,
                copy_modifier: gesture.copy_modifier,
            });
        }
        true
    }

    fn time_selection_track_indices(&self) -> Option<(usize, usize)> {
        let selection = self.time_selection?;
        let tracks = self.state.as_ref()?.project.tracks();
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

    pub(super) fn copy_time_selection(&mut self, cut: bool, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || self.midi_focus.is_focused(window)
            || !self.focus.is_focused(window)
        {
            return;
        }
        let Some(range) = self.normalized_time_selection() else {
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
        if event.error.is_some() || event.state.revision <= pending.source_revision {
            return;
        }
        let Some((&first_track, rest)) = pending.range.tracks().split_first() else {
            return;
        };
        let Some(&last_track) = rest.last().or(Some(&first_track)) else {
            return;
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
        self.selected_track = Some(last_track);
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
        let Some((track, lane)) = self.visible_lane_near_y(y) else {
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
        self.selected_track = Some(track);
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
        }
        self.suppress_cursor_click = false;
        had_move || had_gesture || had_range
    }

    fn visible_lane_at(&self, x: f32, y: f32) -> Option<(TrackId, super::drag::LaneBounds)> {
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

    fn track_has_clip_at_frame(
        &self,
        track: TrackId,
        frame: u64,
        viewport: super::viewport::Viewport,
    ) -> bool {
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        let Some(track) = state.project.track(track) else {
            return true;
        };
        let Some(lane) = self.drag_lanes.get(&track.id()) else {
            return true;
        };
        let frame_tolerance = if lane.width > 0.0 {
            ((viewport.length as f64 * 2.0 / f64::from(lane.width)).ceil() as u64).max(1)
        } else {
            0
        };
        if track.clips().iter().any(|clip| {
            clip.start().saturating_sub(frame_tolerance) <= frame
                && clip
                    .start()
                    .checked_add(clip.duration())
                    .is_some_and(|end| frame <= end.saturating_add(frame_tolerance))
        }) {
            return true;
        }
        track.midi_clips().iter().any(|clip| {
            let start = state.project.tempo_map().beat_to_frame(clip.start()).ok();
            let end = clip
                .start()
                .raw()
                .checked_add(clip.length().raw())
                .map(hex_project::BeatTime::from_raw)
                .and_then(|end| state.project.tempo_map().beat_to_frame(end).ok());
            start.is_some_and(|start| start <= frame.saturating_add(frame_tolerance))
                && end.is_some_and(|end| frame <= end.saturating_add(frame_tolerance))
        })
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
                .map_or(Palette::TEXT, |cursor| cursor.color());
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
                .bg(rgb(Palette::CYAN))
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
                        .hover(|style| style.bg(rgb(Palette::TEXT))),
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
                .bg(rgb(Palette::CYAN))
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
