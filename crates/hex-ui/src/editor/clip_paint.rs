//! Captured double-click drawing; only release submits a canonical clip edit.
use super::{EditorMode, EditorView, musical_time::SnapGrid, viewport::Viewport};
use crate::theme;
use gpui::{Div, div, prelude::*, px, relative, rgb};
use hex_project::{BeatTime, Command, TrackId};
use hex_session::service::Request;

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
            && !self.automation_view.enabled
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

    fn clip_paint_is_current(&self, gesture: ClipPaintGesture) -> bool {
        self.clip_paint_available()
            && self.state.as_ref().is_some_and(|state| {
                gesture.generation == self.project_generation
                    && gesture.revision == state.revision
                    && gesture.viewport == self.visible_viewport(state.timeline_frames)
                    && gesture.snap == self.snap_grid
                    && state.project.track(gesture.track).is_some()
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
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(frame) = super::seek::pointer_to_frame(x, lane.left, lane.width, viewport) else {
            return false;
        };
        let grid = self.snap_grid.with_platform_modifier(free);
        let Some(start) = state
            .project
            .tempo_map()
            .frame_to_beat(frame)
            .ok()
            .and_then(|beat| grid.snap_beat(&state.project, beat, viewport, lane.width))
        else {
            return false;
        };
        if self.clip_handle_at_pointer(track, x, y, viewport) {
            return false;
        }
        // A double click without travel creates one grid cell (one beat when free).
        let minimum_end = if grid.is_off() {
            start
                .raw()
                .checked_add(BeatTime::ONE.raw())
                .map(BeatTime::from_raw)
        } else {
            grid.adjacent_beat(&state.project, start, viewport, lane.width, true)
        };
        let Some(minimum_end) = minimum_end.filter(|end| *end > start) else {
            return false;
        };
        self.clip_paint_gesture = Some(ClipPaintGesture {
            track,
            anchor: start,
            start,
            end: minimum_end,
            minimum_end,
            generation: self.project_generation,
            revision: state.revision,
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
        if !self.clip_paint_is_current(gesture) {
            return self.cancel_clip_paint();
        }
        // Keep ownership of the pointer outside the lane; outside release cancels.
        if let Some((track, lane)) = self.visible_lane_at(x, y)
            && track == gesture.track
            && let Some(state) = self.state.as_ref()
            && let Some(frame) =
                super::seek::pointer_to_frame(x, lane.left, lane.width, gesture.viewport)
            && let Ok(beat) = state.project.tempo_map().frame_to_beat(frame)
            && let Some(end) = self.snap_grid.with_platform_modifier(free).snap_beat(
                &state.project,
                beat,
                gesture.viewport,
                lane.width,
            )
        {
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
        if self.clip_paint_gesture.is_none() {
            return false;
        }
        self.update_clip_paint(x, y, free);
        let Some(gesture) = self.clip_paint_gesture.take() else {
            return true;
        };
        self.suppress_cursor_click = true;
        if !self.clip_paint_is_current(gesture)
            || self
                .visible_lane_at(x, y)
                .is_none_or(|(track, _)| track != gesture.track)
        {
            return true;
        }
        self.enqueue(Request::Edit(Command::PlaceMidiClipWithOverwrite {
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
        if gesture.track != track || !self.clip_paint_is_current(gesture) {
            return None;
        }
        let tempo = self.state.as_ref()?.project.tempo_map();
        let start = tempo.beat_to_frame(gesture.start).ok()?.max(viewport.start);
        let end = tempo.beat_to_frame(gesture.end).ok()?.min(viewport.end()?);
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
                    self.state.as_ref()?.project.track(track)?.color(),
                    hex_project::ClipFlavor::Muted,
                    false,
                )))
                .opacity(0.3),
        )
    }
}
