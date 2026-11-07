//! Captured drawing and immutable released display; only the exact FIFO terminal
//! retires a submitted preview together with canonical state installation.
use super::pending_edit::{Disposition, Receipt};
use super::{EditorMode, EditorView, ViewState, musical_time::SnapGrid, viewport::Viewport};
use crate::theme;
use gpui::{Context, CursorStyle, Div, Window, div, prelude::*, px, relative, rgb};
use hex_project::{BeatTime, TrackId};
use hex_session::service::{Event, PaintMidiClip, Request};
use std::sync::Arc;

#[derive(Clone, Copy)]
struct Paint {
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
impl Paint {
    fn stamp(self) -> (u64, u64, TrackId, BeatTime) {
        (self.generation, self.revision, self.track, self.anchor)
    }

    fn intent(self) -> PaintMidiClip {
        PaintMidiClip {
            project_generation: self.generation,
            revision: self.revision,
            track: self.track,
            start: self.start,
            length: BeatTime::from_raw(self.end.raw() - self.start.raw()),
        }
    }
}

pub(super) struct ClipPaintGesture {
    paint: Paint,
    source: Arc<ViewState>,
    // None owns held input; Some owns immutable released display only.
    receipt: Option<Receipt>,
}

impl EditorView {
    pub(super) fn clip_paint_held(&self) -> bool {
        self.clip_paint_gesture
            .as_ref()
            .is_some_and(|g| g.receipt.is_none())
    }

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
            && !self
                .clip_paint_gesture
                .as_ref()
                .is_some_and(|g| g.receipt.is_some())
    }

    fn clip_paint_is_current(&self, paint: Paint) -> bool {
        self.clip_paint_available()
            && self.state.as_ref().is_some_and(|state| {
                paint.generation == self.project_generation
                    && paint.revision == state.revision
                    && paint.viewport == self.visible_viewport(state.timeline_frames)
                    && paint.snap == self.snap_grid
                    && state.project.track(paint.track).is_some()
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
        if self.clip_handle_at_pointer(track, x, y, viewport, true) {
            return false;
        }
        // A stationary double press creates one cell (one beat with snapping off).
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
            paint: Paint {
                track,
                anchor: start,
                start,
                end: minimum_end,
                minimum_end,
                generation: self.project_generation,
                revision: state.revision,
                viewport,
                snap: self.snap_grid,
            },
            source: Arc::clone(state),
            receipt: None,
        });
        self.pointer_down = None;
        self.cancel_time_selection_gesture();
        self.time_selection = None;
        self.time_selection_project_scope = false;
        self.selected_track = Some(track);
        self.selected_clip = None;
        self.cancel_midi_note_gesture();
        true
    }

    /// Native capture owns held input only; its cancellation cannot retire a receipt.
    pub(super) fn capture_clip_paint(&self, window: &mut Window, cx: &Context<Self>) -> bool {
        let Some(held) = self
            .clip_paint_gesture
            .as_ref()
            .filter(|g| g.receipt.is_none())
        else {
            return false;
        };
        let paint = held.paint;
        let stamp = paint.stamp();
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            gpui::MouseButton::Left,
            CursorStyle::Arrow,
            move |event, window, cx| {
                let moved = event.downcast_ref::<gpui::MouseMoveEvent>();
                let released = event
                    .downcast_ref::<gpui::MouseUpEvent>()
                    .filter(|e| e.button == gpui::MouseButton::Left);
                let Some((position, modifiers)) = moved
                    .map(|e| (e.position, e.modifiers))
                    .or_else(|| released.map(|e| (e.position, e.modifiers)))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.clip_paint_gesture.as_ref().is_some_and(|g| {
                        let p = g.paint;
                        g.receipt.is_none() && p.stamp() == stamp
                    }) {
                        return;
                    }
                    if !view.focus.is_focused(window) {
                        view.cancel_clip_paint();
                    } else if released.is_some() {
                        view.finish_clip_paint(
                            f32::from(position.x),
                            f32::from(position.y),
                            modifiers.platform,
                        );
                    } else {
                        view.update_clip_paint(
                            f32::from(position.x),
                            f32::from(position.y),
                            modifiers.platform,
                        );
                    }
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view
                        .clip_paint_gesture
                        .as_ref()
                        .is_some_and(|g| g.receipt.is_none() && g.paint.stamp() == stamp)
                    {
                        view.cancel_clip_paint();
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }

    pub(super) fn update_clip_paint(&mut self, x: f32, y: f32, free: bool) -> bool {
        let Some(mut paint) = self
            .clip_paint_gesture
            .as_ref()
            .filter(|g| g.receipt.is_none())
            .map(|g| g.paint)
        else {
            return false;
        };
        if !self.clip_paint_is_current(paint) {
            return self.cancel_clip_paint();
        }
        // Travel outside the lane retains capture; outside release cancels.
        if let Some((track, lane)) = self.visible_lane_at(x, y)
            && track == paint.track
            && let Some(state) = self.state.as_ref()
            && let Some(frame) =
                super::seek::pointer_to_frame(x, lane.left, lane.width, paint.viewport)
            && let Ok(beat) = state.project.tempo_map().frame_to_beat(frame)
            && let Some(end) = self.snap_grid.with_platform_modifier(free).snap_beat(
                &state.project,
                beat,
                paint.viewport,
                lane.width,
            )
        {
            paint.start = end.min(paint.anchor);
            paint.end = if end == paint.anchor {
                paint.minimum_end
            } else {
                end.max(paint.anchor)
            };
            if let Some(g) = self.clip_paint_gesture.as_mut() {
                g.paint = paint;
            }
        }
        true
    }

    pub(super) fn finish_clip_paint(&mut self, x: f32, y: f32, free: bool) -> bool {
        if !self.clip_paint_held() {
            return false;
        }
        self.update_clip_paint(x, y, free);
        let Some(mut gesture) = self.clip_paint_gesture.take() else {
            return true;
        };
        self.suppress_cursor_click = true;
        if !self.clip_paint_is_current(gesture.paint)
            || self
                .visible_lane_at(x, y)
                .is_none_or(|(track, _)| track != gesture.paint.track)
        {
            return true;
        }
        let intent = gesture.paint.intent();
        gesture.receipt = Some(Receipt::new(gesture.paint.generation));
        self.clip_paint_gesture = Some(gesture);
        self.enqueue(Request::PaintMidiClip(intent));
        true
    }

    pub(super) fn cancel_clip_paint(&mut self) -> bool {
        if !self.clip_paint_held() {
            return false;
        }
        self.clip_paint_gesture.take().is_some()
    }

    pub(super) fn pending_clip_paint_matches(&self, request: &Request) -> bool {
        self.clip_paint_gesture.as_ref().is_some_and(|g| {
            g.receipt.as_ref().is_some_and(Receipt::is_queued)
                && matches!(request, Request::PaintMidiClip(intent) if *intent == g.paint.intent())
        })
    }
    pub(super) fn clip_paint_submitted(&mut self) {
        if let Some(receipt) = self
            .clip_paint_gesture
            .as_mut()
            .and_then(|g| g.receipt.as_mut())
        {
            receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_pending_clip_paint(&mut self) -> bool {
        if !self
            .clip_paint_gesture
            .as_ref()
            .is_some_and(|g| g.receipt.is_some())
        {
            return false;
        }
        self.clip_paint_gesture.take().is_some()
    }
    pub(super) fn clip_paint_request_complete(&mut self, event: &Event) {
        if event.project_replaced {
            self.cancel_clip_paint();
        }
        let disposition = self
            .clip_paint_gesture
            .as_mut()
            .and_then(|g| g.receipt.as_mut())
            .map_or(Disposition::Waiting, |receipt| {
                receipt.observe(
                    self.project_generation,
                    event.project_replaced,
                    event.request_complete,
                )
            });
        if disposition != Disposition::Waiting {
            self.retire_pending_clip_paint();
        }
    }

    pub(super) fn clip_paint_overlay(&self, track: TrackId, viewport: Viewport) -> Option<Div> {
        let gesture = self.clip_paint_gesture.as_ref()?;
        let paint = gesture.paint;
        if paint.track != track || (gesture.receipt.is_none() && !self.clip_paint_is_current(paint))
        {
            return None;
        }
        let project = &gesture.source.project;
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(paint.start).ok()?.max(viewport.start);
        let end = tempo.beat_to_frame(paint.end).ok()?.min(viewport.end()?);
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
                    project.track(track)?.color(),
                    hex_project::ClipFlavor::Muted,
                    false,
                )))
                .opacity(0.3),
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
