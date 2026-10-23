//! View-only ordinary-track insertion feedback. Returns and Main remain pinned.
//!
//! Pointer motion reads the variable-height list's logical scroll position and
//! bounded row metadata; it never clones the project, prepares audio or moves a
//! track. Release sends one source-stamped worker request. The final insertion
//! line stays owned by its exact ordinary receipt until canonical installation.
use super::{
    EditorMode, EditorView,
    pending_edit::{Disposition, Receipt},
};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, MouseMoveEvent, MouseUpEvent, Window, canvas, div, prelude::*, px,
    rgb,
};
use hex_project::TrackId;
use hex_session::service::{Event, Request};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct TrackReorder {
    gesture: Option<Gesture>,
    pending: Option<Pending>,
    painted_gap: Option<PaintedGap>,
}
struct Gesture {
    generation: u64,
    revision: u64,
    track: TrackId,
    origin: (f32, f32),
    pointer: (f32, f32),
    moved: bool,
    destination: Option<Destination>,
    last_scroll: Instant,
}
#[derive(Clone, Copy)]
struct Destination {
    before: Option<TrackId>,
}
#[derive(Clone, Copy, PartialEq)]
struct PaintedGap {
    generation: u64,
    revision: u64,
    track: TrackId,
    before: Option<TrackId>,
    scroll: (usize, f32),
}
struct Pending {
    generation: u64,
    revision: u64,
    track: TrackId,
    destination: Destination,
    receipt: Receipt,
}
impl EditorView {
    pub(super) fn track_reorder_active(&self) -> bool {
        self.track_reorder.gesture.is_some() || self.track_reorder.pending.is_some()
    }
    pub(super) fn begin_track_reorder(
        &mut self,
        track: TrackId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode != EditorMode::Arrangement
            || !self.drag_available()
            || self.edit_pending()
            || self.track_reorder_active()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.time_range_move_gesture.is_some()
            || self.time_selection_gesture.is_some()
            || self.gain_lane.draft.is_some()
            || self.sections.naming()
            || self.tuning.open
            || self.theme_settings.open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.changelog.is_open()
        {
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        if state.project.track(track).is_none() {
            return;
        }
        let revision = state.revision;
        let pointer = (f32::from(position.x), f32::from(position.y));
        if !pointer.0.is_finite() || !pointer.1.is_finite() {
            return;
        }
        self.track_header_focus(track, cx).focus(window);
        self.track_reorder.gesture = Some(Gesture {
            generation: self.project_generation,
            revision,
            track,
            origin: pointer,
            pointer,
            moved: false,
            destination: None,
            last_scroll: Instant::now(),
        });
    }
    fn track_reorder_current(&self, g: &Gesture) -> bool {
        self.mode == EditorMode::Arrangement
            && g.generation == self.project_generation
            && !self.edit_pending()
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == g.revision && s.project.track(g.track).is_some())
    }
    pub(super) fn cancel_track_reorder(&mut self) -> bool {
        self.track_reorder.gesture.take().is_some()
    }
    pub(super) fn reconcile_track_reorder(&mut self, window: &Window) {
        if self
            .track_reorder
            .gesture
            .as_ref()
            .is_some_and(|g| !self.track_reorder_current(g) || !self.track_header_focused(window))
        {
            self.cancel_track_reorder();
        } else if let Some(g) = &self.track_reorder.gesture {
            // Wheel/scrollbar motion can change list geometry without a pointer
            // move. The painted gap and release hit test share this mapping.
            self.update_track_reorder(gpui::point(px(g.pointer.0), px(g.pointer.1)));
        }
    }
    fn track_reorder_destination(
        &self,
        pointer: (f32, f32),
        source: TrackId,
    ) -> Option<Destination> {
        let bounds = self.arrangement_bounds?;
        let (x, y) = pointer;
        if !x.is_finite()
            || !y.is_finite()
            || x < bounds.left
            || x > bounds.left + bounds.width
            || y < bounds.top
            || y > bounds.top + bounds.height
        {
            return None;
        }
        let tracks = self.state.as_ref()?.project.tracks();
        let top = self.track_scroll.list.logical_scroll_top();
        let mut row_y = bounds.top - f32::from(top.offset_in_item);
        let mut index = top.item_ix;
        for track in tracks.iter().skip(index) {
            let height = self.track_height(track.id());
            if y < row_y + height / 2.0 {
                break;
            }
            row_y += height;
            index += 1;
        }
        let source_index = tracks.iter().position(|t| t.id() == source)?;
        // Both sides of the source row describe the same no-op gap.
        if index == source_index || index == source_index + 1 {
            return None;
        }
        if row_y < bounds.top || row_y > bounds.top + bounds.height {
            return None;
        }
        Some(Destination {
            before: tracks.get(index).map(|t| t.id()),
        })
    }

    fn update_track_reorder(&mut self, position: gpui::Point<gpui::Pixels>) -> bool {
        let Some(mut g) = self.track_reorder.gesture.take() else {
            return false;
        };
        if !self.track_reorder_current(&g) {
            return true;
        }
        g.pointer = (f32::from(position.x), f32::from(position.y));
        g.moved |= (g.pointer.0 - g.origin.0)
            .abs()
            .max((g.pointer.1 - g.origin.1).abs())
            >= 4.0;
        g.destination = g
            .moved
            .then(|| self.track_reorder_destination(g.pointer, g.track))
            .flatten();
        self.track_reorder.gesture = Some(g);
        true
    }
    fn track_gap_witness(&self, g: &Gesture) -> Option<PaintedGap> {
        let top = self.track_scroll.list.logical_scroll_top();
        Some(PaintedGap {
            generation: g.generation,
            revision: g.revision,
            track: g.track,
            before: g.destination?.before,
            scroll: (top.item_ix, f32::from(top.offset_in_item)),
        })
    }
    pub(super) fn track_reorder_painted(&mut self) {
        self.track_reorder.painted_gap = self
            .track_reorder
            .gesture
            .as_ref()
            .and_then(|g| self.track_gap_witness(g));
    }
    fn finish_track_reorder(&mut self, position: gpui::Point<gpui::Pixels>) -> bool {
        self.update_track_reorder(position);
        let captured = self.track_reorder.gesture.as_ref().is_some_and(|g| g.moved);
        let Some(g) = self.track_reorder.gesture.take() else {
            return captured;
        };
        // A wheel/scrollbar or final pointer event can arrive before a repaint.
        // Never commit a different gap than the user actually saw highlighted.
        if self.track_gap_witness(&g) != self.track_reorder.painted_gap {
            return captured;
        }
        if let Some(destination) = g.destination {
            self.track_reorder.pending = Some(Pending {
                generation: g.generation,
                revision: g.revision,
                track: g.track,
                destination,
                receipt: Receipt::new(g.generation),
            });
            if !self.enqueue(Request::ReorderTrack {
                project_generation: g.generation,
                revision: g.revision,
                track: g.track,
                before: destination.before,
            }) {
                self.retire_pending_track_reorder();
            }
        }
        captured
    }
    pub(super) fn tick_track_reorder(&mut self, now: Instant) -> bool {
        let Some(g) = self.track_reorder.gesture.as_ref() else {
            return false;
        };
        if !self.track_reorder_current(g) {
            return self.cancel_track_reorder();
        }
        let Some(b) = self.arrangement_bounds else {
            return false;
        };
        let (x, y) = g.pointer;
        if !g.moved
            || x < b.left
            || x > b.left + b.width
            || y < b.top
            || y > b.top + b.height
            || now.saturating_duration_since(g.last_scroll) < Duration::from_millis(50)
        {
            return false;
        }
        let direction = if y < b.top + 24.0 {
            -1.0
        } else if y > b.top + b.height - 24.0 {
            1.0
        } else {
            return false;
        };
        let pointer = gpui::point(px(x), px(y));
        let offset = self.track_scroll.offset();
        let next_y = (f32::from(offset.y) - direction * 24.0)
            .clamp(-f32::from(self.track_scroll.max_offset().height), 0.0);
        if let Some(g) = self.track_reorder.gesture.as_mut() {
            g.last_scroll = now;
        }
        if next_y == f32::from(offset.y) {
            return false;
        }
        self.track_scroll
            .set_offset(gpui::point(offset.x, px(next_y)));
        self.drag_layout_generation = self.drag_layout_generation.wrapping_add(1);
        if let Some(g) = self.track_reorder.gesture.as_mut() {
            g.last_scroll = now;
        }
        self.update_track_reorder(pointer);
        true
    }
    pub(super) fn track_reorder_line(&self) -> Option<Div> {
        let destination = self
            .track_reorder
            .pending
            .as_ref()
            .map(|p| p.destination)
            .or_else(|| self.track_reorder.gesture.as_ref()?.destination)?;
        let bounds = self.arrangement_bounds?;
        let tracks = self.state.as_ref()?.project.tracks();
        let index = match destination.before {
            Some(id) => tracks.iter().position(|t| t.id() == id)?,
            None => tracks.len(),
        };
        let top = self.track_scroll.list.logical_scroll_top();
        let mut y = -f32::from(top.offset_in_item);
        if index >= top.item_ix {
            for track in tracks.iter().skip(top.item_ix).take(index - top.item_ix) {
                y += self.track_height(track.id());
            }
        } else {
            for track in tracks.iter().skip(index).take(top.item_ix - index) {
                y -= self.track_height(track.id());
            }
        }
        if y < 0.0 || y > bounds.height {
            return None;
        }
        Some(
            div()
                .debug_selector(|| "track-insertion-line".into())
                .absolute()
                .left_0()
                .right_0()
                .top(px(y.max(1.0) - 1.0))
                .h(px(2.0))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        )
    }
    pub(super) fn track_reorder_events(&self, cx: &mut Context<Self>) -> gpui::Canvas<()> {
        let entity = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if !phase.capture() {
                        return;
                    }
                    moved.update(cx, |v, cx| {
                        let changed = if event.pressed_button == Some(MouseButton::Left) {
                            v.update_track_reorder(event.position)
                        } else {
                            v.cancel_track_reorder()
                        };
                        if changed {
                            cx.notify();
                            cx.stop_propagation();
                        }
                    });
                });
                let up = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    up.update(cx, |v, cx| {
                        if v.finish_track_reorder(event.position) {
                            cx.notify();
                            cx.stop_propagation();
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()
    }
    pub(super) fn pending_track_reorder_matches(&self, request: &Request) -> bool {
        let Some(p) = &self.track_reorder.pending else {
            return false;
        };
        matches!(request, Request::ReorderTrack { project_generation, revision, track, before } if p.receipt.is_queued() && *project_generation==p.generation && *revision==p.revision && *track==p.track && *before==p.destination.before)
    }
    pub(super) fn track_reorder_submitted(&mut self) {
        if let Some(p) = self.track_reorder.pending.as_mut() {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_pending_track_reorder(&mut self) -> bool {
        self.track_reorder.pending.take().is_some()
    }
    pub(super) fn track_reorder_complete(&mut self, event: &Event) {
        if self.track_reorder.pending.as_mut().is_some_and(|p| {
            p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        }) {
            self.retire_pending_track_reorder();
        }
        if event.project_replaced
            || self
                .track_reorder
                .gesture
                .as_ref()
                .is_some_and(|g| g.revision != event.state.revision)
        {
            self.cancel_track_reorder();
        }
    }
}
