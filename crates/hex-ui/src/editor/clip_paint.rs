//! Captured double-click drawing; only release submits a canonical clip edit.
use super::{EditorMode, EditorView, musical_time::SnapGrid, viewport::Viewport};
use crate::theme;
use gpui::{Div, div, prelude::*, px, relative, rgb};
use hex_project::{BeatTime, Command, TrackId};
use hex_session::service::Request;
use hex_session::snapshot_exports::{SnapshotRead, SnapshotTransportError};

#[derive(Clone, Copy)]
pub(super) struct ClipPaintGesture {
    track: TrackId,
    anchor: BeatTime,
    start: BeatTime,
    end: BeatTime,
    minimum_end: BeatTime,
    generation: u64,
    revision: u64,
    viewport: Viewport,
    snap: SnapGrid,
}

impl EditorView {
    fn clip_paint_available(&self) -> bool {
        self.mode == EditorMode::Arrangement
            && self.clip_name_edit.is_none()
            && !self.track_name.is_editing()
            && !self.edit_pending()
            && self.context_menu.is_none()
            && !self.opening_project
            && !self.close_prompt_open
            && !self.save_dialog_open
            && !self.export_dialog_open
            && self.active_clip_drag.is_none()
            && self.active_browser_audio_drag.is_none()
            && self.active_native_device_drag.is_none()
            && self.active_trim_drag.is_none()
            && self.active_midi_trim_drag.is_none()
    }

    fn clip_paint_is_current_in(
        &self,
        gesture: ClipPaintGesture,
        snapshot: SnapshotRead<'_>,
    ) -> bool {
        self.clip_paint_available()
            && gesture.generation == self.project_generation
            && gesture.revision == snapshot.revision()
            && gesture.viewport == self.visible_viewport(snapshot, snapshot.timeline_frames())
            && gesture.snap == self.snap_grid
            && snapshot.model().track(gesture.track).is_some()
    }

    fn clip_paint_projection(
        &self,
        gesture: ClipPaintGesture,
        x: f32,
        y: f32,
        free: bool,
    ) -> Result<Option<(bool, Option<BeatTime>)>, SnapshotTransportError> {
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };
        state.read_with(|snapshot| {
            if !self.clip_paint_is_current_in(gesture, snapshot) {
                return None;
            }
            let project = snapshot.model();
            let lane = self
                .visible_lane_at_in(project, x, y)
                .filter(|(track, _)| *track == gesture.track);
            let end = lane.and_then(|(_, lane)| {
                let frame =
                    super::seek::pointer_to_frame(x, lane.left, lane.width, gesture.viewport)?;
                let beat = project.tempo().frame_to_beat(frame).ok()?;
                self.snap_grid.with_platform_modifier(free).snap_beat(
                    &project,
                    beat,
                    gesture.viewport,
                    lane.width,
                )
            });
            Some((lane.is_some(), end))
        })
    }

    pub(super) fn begin_clip_paint(&mut self, x: f32, y: f32, free: bool) -> bool {
        if !self.clip_paint_available() {
            return false;
        }
        let Some((track, lane)) = self.visible_lane_at(x, y) else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Ok(Some((viewport, start, minimum_end, revision))) = state.read_with(|snapshot| {
            let project = snapshot.model();
            let viewport = self.visible_viewport(snapshot, snapshot.timeline_frames());
            let frame = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport)?;
            let grid = self.snap_grid.with_platform_modifier(free);
            let beat = project.tempo().frame_to_beat(frame).ok()?;
            let start = grid.snap_beat(&project, beat, viewport, lane.width)?;
            let minimum_end = if grid == SnapGrid::Off {
                start
                    .raw()
                    .checked_add(BeatTime::ONE.raw())
                    .map(BeatTime::from_raw)
            } else {
                grid.adjacent_beat(&project, start, viewport, lane.width, true)
            }?;
            (minimum_end > start).then_some((viewport, start, minimum_end, snapshot.revision()))
        }) else {
            return false;
        };
        if self.clip_handle_at_pointer(track, x, y, viewport) {
            return false;
        }
        self.clip_paint_gesture = Some(ClipPaintGesture {
            track,
            anchor: start,
            start,
            end: minimum_end,
            minimum_end,
            generation: self.project_generation,
            revision,
            viewport,
            snap: self.snap_grid,
        });
        self.cancel_time_selection_gesture();
        self.time_selection = None;
        self.time_selection_project_scope = false;
        self.selected_track = Some(track);
        self.selected_clip = None;
        self.cancel_midi_note_gesture();
        true
    }

    pub(super) fn update_clip_paint(&mut self, x: f32, y: f32, free: bool) -> bool {
        let Some(mut gesture) = self.clip_paint_gesture else {
            return false;
        };
        let projection = match self.clip_paint_projection(gesture, x, y, free) {
            Ok(Some(projection)) => projection,
            Ok(None) => return self.cancel_clip_paint(),
            Err(_) => return false,
        };
        if let (_, Some(end)) = projection {
            gesture.start = end.min(gesture.anchor);
            gesture.end = if end == gesture.anchor {
                gesture.minimum_end
            } else {
                end.max(gesture.anchor)
            };
            self.clip_paint_gesture = Some(gesture);
        }
        true
    }

    pub(super) fn finish_clip_paint(&mut self, x: f32, y: f32, free: bool) -> bool {
        let Some(mut gesture) = self.clip_paint_gesture else {
            return false;
        };
        let projection = match self.clip_paint_projection(gesture, x, y, free) {
            Ok(projection) => projection,
            Err(_) => return false,
        };
        self.clip_paint_gesture = None;
        self.suppress_cursor_click = true;
        let Some((true, end)) = projection else {
            return true;
        };
        if let Some(end) = end {
            gesture.start = end.min(gesture.anchor);
            gesture.end = if end == gesture.anchor {
                gesture.minimum_end
            } else {
                end.max(gesture.anchor)
            };
        }
        self.enqueue(Request::Edit(Command::PlaceMidiClip {
            track: gesture.track,
            start: gesture.start,
            length: BeatTime::from_raw(gesture.end.raw() - gesture.start.raw()),
        }));
        true
    }

    pub(super) fn cancel_clip_paint(&mut self) -> bool {
        self.clip_paint_gesture.take().is_some()
    }

    pub(super) fn clip_paint_overlay(&self, track: TrackId, viewport: Viewport) -> Option<Div> {
        let gesture = self.clip_paint_gesture?;
        if gesture.track != track {
            return None;
        }
        let (start, end, color) = self
            .state
            .as_ref()?
            .read_with(|snapshot| {
                if !self.clip_paint_is_current_in(gesture, snapshot) {
                    return None;
                }
                let project = snapshot.model();
                let tempo = project.tempo();
                let start = tempo.beat_to_frame(gesture.start).ok()?.max(viewport.start);
                let end = tempo.beat_to_frame(gesture.end).ok()?.min(viewport.end()?);
                Some((start, end, project.track(track)?.color()))
            })
            .ok()
            .flatten()?;
        if end <= start || viewport.length == 0 {
            return None;
        }
        Some(
            div()
                .debug_selector(|| "clip-paint-preview".to_owned())
                .absolute()
                .top(px(3.0))
                .bottom(px(3.0))
                .left(relative(
                    (start - viewport.start) as f32 / viewport.length as f32,
                ))
                .w(relative((end - start) as f32 / viewport.length as f32))
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Secondary)))
                .bg(rgb(theme::clip_fill(
                    color,
                    hex_project::ClipFlavor::Muted,
                    false,
                )))
                .opacity(0.3),
        )
    }
}
