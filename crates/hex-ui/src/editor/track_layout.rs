//! Arrangement-only layout state; resizing never edits the project or audio.
use super::{
    EditorView,
    tracks::{TRACK_HEADER_WIDTH, TRACK_ROW_HEIGHT},
};
use gpui::{
    Context, Div, ListState, MouseButton, MouseMoveEvent, MouseUpEvent, Window, canvas, div,
    prelude::*, px,
};
use hex_project::TrackId;
use std::collections::BTreeMap;

// Reuse the browser/workspace split's existing room budget. The default
// header and its complete controls still fit at the minimum size.
const MAX_HEADER_WIDTH: f32 = super::browser::DEFAULT_WIDTH;
const MIN_LANE_WIDTH: f32 = super::browser::MIN_WORKSPACE_WIDTH - TRACK_HEADER_WIDTH;

#[derive(Default)]
pub(super) struct TrackLayout {
    width: Option<f32>,
    pub(super) available_width: Option<f32>,
    heights: BTreeMap<TrackId, f32>,
    drag: Option<ResizeDrag>,
    generation: u64,
    tracks: Vec<TrackId>,
    revision: Option<u64>,
}
#[derive(Clone, Copy)]
struct ResizeDrag {
    track: Option<TrackId>,
    pointer: f32,
    original: f32,
    generation: u64,
    revision: u64,
    previous: Option<f32>,
    maximum: f32,
}

/// Keep pan/autoscroll's pixel interface while GPUI owns variable-row layout.
#[derive(Clone)]
pub(super) struct TrackScroll {
    pub(super) list: ListState,
}
impl TrackScroll {
    pub(super) fn new() -> Self {
        Self {
            list: ListState::new(0, gpui::ListAlignment::Top, px(100.0)).measure_all(),
        }
    }
    pub(super) fn offset(&self) -> gpui::Point<gpui::Pixels> {
        self.list.scroll_px_offset_for_scrollbar()
    }
    pub(super) fn max_offset(&self) -> gpui::Size<gpui::Pixels> {
        self.list.max_offset_for_scrollbar()
    }
    pub(super) fn set_offset(&self, offset: gpui::Point<gpui::Pixels>) {
        self.list.set_offset_from_scrollbar(offset);
    }
    pub(super) fn scroll_to_item(&self, index: usize, strategy: gpui::ScrollStrategy) {
        if strategy == gpui::ScrollStrategy::Top {
            self.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: px(0.0),
            });
        } else {
            self.list.scroll_to_reveal_item(index);
        }
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn logical_scroll_top_index(&self) -> usize {
        self.list.logical_scroll_top().item_ix
    }
    pub(super) fn remeasure(&self) {
        let top = self.list.logical_scroll_top();
        self.list.reset(self.list.item_count());
        self.list.scroll_to(top);
    }
    fn invalidate(&self, range: std::ops::Range<usize>, count: usize) {
        let top = self.list.logical_scroll_top();
        self.list.splice(range, count);
        self.list.scroll_to(top);
    }
}
impl EditorView {
    fn maximum_header_width(&self) -> f32 {
        self.track_layout
            .available_width
            .map_or(MAX_HEADER_WIDTH, |width| {
                (width - MIN_LANE_WIDTH).clamp(TRACK_HEADER_WIDTH, MAX_HEADER_WIDTH)
            })
    }
    pub(super) fn track_header_width(&self) -> f32 {
        self.track_layout
            .width
            .unwrap_or(TRACK_HEADER_WIDTH)
            .clamp(TRACK_HEADER_WIDTH, self.maximum_header_width())
    }
    pub(super) fn track_resize_active(&self) -> bool {
        self.track_layout.drag.is_some()
    }
    pub(super) fn track_height(&self, track: TrackId) -> f32 {
        self.track_layout
            .heights
            .get(&track)
            .copied()
            .unwrap_or(TRACK_ROW_HEIGHT)
    }
    pub(super) fn sync_track_layout(&mut self) {
        let facts = match self.state.as_ref() {
            Some(state) => state.read_with(|snapshot| {
                let revision = Some(snapshot.revision());
                let ids = (self.track_layout.generation != self.project_generation
                    || self.track_layout.revision != revision)
                    .then(|| {
                        snapshot
                            .model()
                            .tracks()
                            .map(|track| track.id())
                            .collect::<Vec<_>>()
                    });
                (revision, ids)
            }),
            None => Ok((
                None,
                (self.track_layout.generation != self.project_generation
                    || self.track_layout.revision.is_some())
                .then(Vec::new),
            )),
        };
        let Ok((revision, ids)) = facts else {
            return;
        };
        if self.track_layout.generation != self.project_generation {
            self.track_layout.heights.clear();
            self.track_layout.drag = None;
            self.track_layout.generation = self.project_generation;
            self.track_layout.tracks.clear();
            self.track_scroll.list.reset(0);
            self.track_layout.revision = None;
        }
        if self.track_layout.revision == revision {
            return;
        }
        self.track_layout.revision = revision;
        let Some(ids) = ids else {
            return;
        };
        if self.track_layout.tracks != ids {
            if let Some(drag) = self.track_layout.drag.take() {
                self.restore_track_resize(drag);
            }
            self.track_layout.heights.retain(|id, _| ids.contains(id));
            let mut top = self.track_scroll.list.logical_scroll_top();
            let anchor = self.track_layout.tracks.get(top.item_ix).copied();
            top.item_ix = anchor
                .and_then(|id| ids.iter().position(|item| *item == id))
                .unwrap_or(top.item_ix.min(ids.len().saturating_sub(1)));
            // Re-measure the bounded (<=256) row set only on a structural edit.
            // Ordinary frames and width drags render visible rows; height drags
            // invalidate only the captured row, retaining the logical scroll.
            self.track_scroll.list.reset(ids.len());
            self.track_scroll.list.scroll_to(top);
            self.track_layout.tracks = ids;
        }
    }
    fn invalidate_track_size(&self, track: Option<TrackId>) {
        if let Some(id) = track {
            if let Some(index) = self.track_layout.tracks.iter().position(|t| *t == id) {
                self.track_scroll.invalidate(index..index + 1, 1);
            }
        }
    }
    fn begin_track_resize(
        &mut self,
        track: Option<TrackId>,
        pointer: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !pointer.is_finite()
            || self.mode != super::EditorMode::Arrangement
            || !self.drag_available()
            || self.track_resize_active()
            || self.edit_pending()
            || self.context_menu.is_some()
            || self.arrangement_pan.is_some()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.loop_rail_gesture.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.active_native_device_drag.is_some()
        {
            return;
        }
        let Some(revision) = self.state.as_ref().and_then(|state| {
            state
                .read_with(|snapshot| {
                    track
                        .is_none_or(|id| snapshot.model().track(id).is_some())
                        .then_some(snapshot.revision())
                })
                .ok()
                .flatten()
        }) else {
            return;
        };
        let previous = track.map_or(self.track_layout.width, |id| {
            self.track_layout.heights.get(&id).copied()
        });
        let maximum = if track.is_some() {
            self.arrangement_bounds
                .map_or(TRACK_ROW_HEIGHT, |body| body.height.max(TRACK_ROW_HEIGHT))
        } else {
            self.maximum_header_width()
        };
        let original = track.map_or_else(|| self.track_header_width(), |id| self.track_height(id));
        self.track_layout.drag = Some(ResizeDrag {
            track,
            pointer,
            original,
            generation: self.project_generation,
            revision,
            previous,
            maximum,
        });
        self.focus.focus(window);
        cx.notify();
    }
    fn track_resize_current(
        &self,
        drag: ResizeDrag,
    ) -> Result<bool, hex_session::snapshot_exports::SnapshotTransportError> {
        let current = self
            .state
            .as_ref()
            .map(|state| {
                state.read_with(|snapshot| {
                    snapshot.revision() == drag.revision
                        && drag
                            .track
                            .is_none_or(|id| snapshot.model().track(id).is_some())
                })
            })
            .transpose()?;
        Ok(self.mode == super::EditorMode::Arrangement
            && drag.generation == self.project_generation
            && current.unwrap_or(false))
    }
    pub(super) fn reconcile_track_resize(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = self.track_layout.drag else {
            return;
        };
        let Ok(current) = self.track_resize_current(drag) else {
            return;
        };
        if !current || !self.focus.is_focused(window) {
            self.cancel_track_resize(cx);
        }
    }
    fn update_track_resize(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.track_layout.drag else {
            return;
        };
        let Ok(current) = self.track_resize_current(drag) else {
            return;
        };
        if !current {
            self.cancel_track_resize(cx);
            return;
        }
        let pointer = if drag.track.is_some() { y } else { x };
        if !pointer.is_finite() {
            return;
        }
        if let Some(track) = drag.track {
            let height =
                (drag.original + pointer - drag.pointer).clamp(TRACK_ROW_HEIGHT, drag.maximum);
            if height == self.track_height(track) {
                return;
            }
            self.track_layout.heights.insert(track, height);
        } else {
            let width = drag.original - (pointer - drag.pointer);
            let width = width.clamp(TRACK_HEADER_WIDTH, drag.maximum);
            if width == self.track_header_width() {
                return;
            }
            self.track_layout.width = Some(width);
        }
        self.invalidate_track_size(drag.track);
        cx.notify();
    }
    fn restore_track_resize(&mut self, drag: ResizeDrag) {
        if drag.generation != self.project_generation {
            return;
        }
        if let Some(track) = drag.track {
            if let Some(previous) = drag.previous {
                self.track_layout.heights.insert(track, previous);
            } else {
                self.track_layout.heights.remove(&track);
            }
        } else {
            self.track_layout.width = drag.previous;
        }
        self.invalidate_track_size(drag.track);
    }
    pub(super) fn cancel_track_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.track_layout.drag.take() else {
            return false;
        };
        self.restore_track_resize(drag);
        cx.notify();
        true
    }
    pub(super) fn track_width_resize_handle(
        &self,
        track: Option<TrackId>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let label = track.map_or_else(
            || "main-track-width-resize".to_owned(),
            |id| format!("track-width-resize-{}", id.value()),
        );
        div()
            .id(gpui::SharedString::from(label.clone()))
            .debug_selector(move || label.clone())
            .absolute()
            .left_0()
            .top_0()
            .bottom(px(if track.is_some() { 3.0 } else { 0.0 }))
            .w(px(5.0))
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.begin_track_resize(None, f32::from(event.position.x), window, cx);
                }),
            )
    }
    pub(super) fn track_resize_handles(
        &self,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> [gpui::Stateful<Div>; 2] {
        [
            self.track_width_resize_handle(Some(track), cx),
            div()
                .id(("track-height-resize", track.value()))
                .debug_selector(move || format!("track-height-resize-{}", track.value()))
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(3.0))
                .cursor_row_resize()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        view.begin_track_resize(
                            Some(track),
                            f32::from(event.position.y),
                            window,
                            cx,
                        );
                    }),
                ),
        ]
    }
    pub(super) fn track_resize_events(&self, cx: &mut Context<Self>) -> gpui::Canvas<()> {
        let entity = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if !phase.capture() {
                        return;
                    }
                    moved.update(cx, |view, cx| {
                        if view.track_layout.drag.is_none() {
                            return;
                        }
                        if event.pressed_button != Some(MouseButton::Left) {
                            view.cancel_track_resize(cx);
                        } else {
                            view.update_track_resize(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                                cx,
                            );
                        }
                        cx.stop_propagation();
                    });
                });
                let released = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    released.update(cx, |view, cx| {
                        if view.track_layout.drag.is_none() {
                            return;
                        }
                        view.update_track_resize(
                            f32::from(event.position.x),
                            f32::from(event.position.y),
                            cx,
                        );
                        view.track_layout.drag = None;
                        cx.notify();
                        cx.stop_propagation();
                    });
                });
            },
        )
        .absolute()
        .inset_0()
    }
}
