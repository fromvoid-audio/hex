//! Arrangement-only layout state; resizing never edits the project or audio.
use super::{EditorView, tracks::{TRACK_HEADER_WIDTH, TRACK_ROW_HEIGHT}};
use gpui::{Context, Div, ListState, MouseButton, MouseMoveEvent, MouseUpEvent, Window, canvas, div, prelude::*, px};
use hex_project::TrackId;
use std::collections::BTreeMap;

const MAX_HEADER_WIDTH: f32 = 480.0;
const MIN_LANE_WIDTH: f32 = 240.0;
const MAX_TRACK_HEIGHT: f32 = TRACK_ROW_HEIGHT * 8.0;

#[derive(Default)]
pub(super) struct TrackLayout {
    pub(super) width: Option<f32>,
    heights: BTreeMap<TrackId, f32>,
    drag: Option<ResizeDrag>,
    generation: u64,
    tracks: Vec<TrackId>,
}
#[derive(Clone, Copy)]
struct ResizeDrag {
    track: Option<TrackId>,
    pointer: f32,
    original: f32,
    generation: u64,
}

/// Keep pan/autoscroll's pixel interface while GPUI owns variable-row layout.
#[derive(Clone)]
pub(super) struct TrackScroll {
    pub(super) list: ListState,
}
impl TrackScroll {
    pub(super) fn new() -> Self {
        Self { list: ListState::new(0, gpui::ListAlignment::Top, px(100.0)).measure_all() }
    }
    pub(super) fn offset(&self) -> gpui::Point<gpui::Pixels> { self.list.scroll_px_offset_for_scrollbar() }
    pub(super) fn max_offset(&self) -> gpui::Size<gpui::Pixels> { self.list.max_offset_for_scrollbar() }
    pub(super) fn set_offset(&self, offset: gpui::Point<gpui::Pixels>) { self.list.set_offset_from_scrollbar(offset); }
    pub(super) fn scroll_to_item(&self, index: usize, strategy: gpui::ScrollStrategy) {
        if strategy == gpui::ScrollStrategy::Top {
            self.list.scroll_to(gpui::ListOffset { item_ix: index, offset_in_item: px(0.0) });
        } else { self.list.scroll_to_reveal_item(index); }
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn logical_scroll_top_index(&self) -> usize { self.list.logical_scroll_top().item_ix }
    fn invalidate(&self, range: std::ops::Range<usize>, count: usize) {
        let top = self.list.logical_scroll_top();
        self.list.splice(range, count);
        self.list.scroll_to(top);
    }
}
impl EditorView {
    pub(super) fn track_header_width(&self) -> f32 {
        let maximum = self.arrangement_bounds.map_or(MAX_HEADER_WIDTH, |body|
            (body.width - MIN_LANE_WIDTH).clamp(TRACK_HEADER_WIDTH, MAX_HEADER_WIDTH));
        self.track_layout.width.unwrap_or(TRACK_HEADER_WIDTH).clamp(TRACK_HEADER_WIDTH, maximum)
    }
    pub(super) fn track_height(&self, track: TrackId) -> f32 {
        self.track_layout.heights.get(&track).copied().unwrap_or(TRACK_ROW_HEIGHT)
    }
    pub(super) fn sync_track_layout(&mut self, tracks: &[hex_project::Track]) {
        if self.track_layout.generation != self.project_generation {
            self.track_layout.heights.clear();
            self.track_layout.drag = None;
            self.track_layout.generation = self.project_generation;
        }
        let ids = tracks.iter().map(|t| t.id()).collect::<Vec<_>>();
        if self.track_layout.tracks != ids {
            self.track_layout.heights.retain(|id, _| ids.contains(id));
            self.track_scroll.invalidate(0..self.track_scroll.list.item_count(), ids.len());
            self.track_layout.tracks = ids;
        }
    }
    fn invalidate_track_size(&self, track: Option<TrackId>) {
        if let Some(id) = track {
            if let Some(index) = self.track_layout.tracks.iter().position(|t| *t == id) {
                self.track_scroll.invalidate(index..index+1, 1);
            }
        } else {
            let count = self.track_scroll.list.item_count();
            self.track_scroll.invalidate(0..count, count);
        }
    }
    fn begin_track_resize(&mut self, track: Option<TrackId>, pointer: f32, window: &mut Window, cx: &mut Context<Self>) {
        if !pointer.is_finite() || self.edit_pending() || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some() || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some() || self.active_native_device_drag.is_some() {
            return;
        }
        let original = track.map_or_else(|| self.track_header_width(), |id| self.track_height(id));
        self.track_layout.drag = Some(ResizeDrag { track, pointer, original, generation: self.project_generation });
        self.focus.focus(window);
        cx.notify();
    }
    fn update_track_resize(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.track_layout.drag else { return; };
        if drag.generation != self.project_generation { self.track_layout.drag = None; return; }
        let pointer = if drag.track.is_some() { y } else { x };
        if !pointer.is_finite() { return; }
        if let Some(track) = drag.track {
            let height = (drag.original + pointer - drag.pointer).clamp(TRACK_ROW_HEIGHT, MAX_TRACK_HEIGHT);
            if height == self.track_height(track) { return; }
            self.track_layout.heights.insert(track, height);
        } else {
            let width = drag.original - (pointer-drag.pointer);
            let maximum = self.arrangement_bounds.map_or(MAX_HEADER_WIDTH, |body|
                (body.width-MIN_LANE_WIDTH).clamp(TRACK_HEADER_WIDTH,MAX_HEADER_WIDTH));
            let width = width.clamp(TRACK_HEADER_WIDTH,maximum);
            if width == self.track_header_width() { return; }
            self.track_layout.width = Some(width);
        }
        self.invalidate_track_size(drag.track);
        cx.notify();
    }
    pub(super) fn cancel_track_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.track_layout.drag.take() else { return false; };
        if drag.generation == self.project_generation {
            if let Some(track) = drag.track { self.track_layout.heights.insert(track,drag.original); }
            else { self.track_layout.width = Some(drag.original); }
            self.invalidate_track_size(drag.track);
        }
        cx.notify();
        true
    }
    pub(super) fn track_resize_handles(&self, track: TrackId, cx: &mut Context<Self>) -> Div {
        div().absolute().inset_0().children([
            div().id(("track-width-resize",track.value())).debug_selector(move || format!("track-width-resize-{}",track.value()))
                .absolute().left_0().top_0().bottom(px(5.0)).w(px(5.0)).cursor_col_resize()
                .on_mouse_down(MouseButton::Left,cx.listener(|view,event:&gpui::MouseDownEvent,window,cx| {
                    cx.stop_propagation(); view.begin_track_resize(None,f32::from(event.position.x),window,cx);
                })),
            div().id(("track-height-resize",track.value())).debug_selector(move || format!("track-height-resize-{}",track.value()))
                .absolute().left_0().right_0().bottom_0().h(px(5.0)).cursor_row_resize()
                .on_mouse_down(MouseButton::Left,cx.listener(move |view,event:&gpui::MouseDownEvent,window,cx| {
                    cx.stop_propagation(); view.begin_track_resize(Some(track),f32::from(event.position.y),window,cx);
                })),
        ])
    }
    pub(super) fn track_resize_events(&self, cx: &mut Context<Self>) -> gpui::Canvas<()> {
        let entity = cx.entity();
        canvas(|_,_,_| (),move |_,_,window,_| {
            let moved = entity.clone();
            window.on_mouse_event(move |event:&MouseMoveEvent,phase,_,cx| {
                if !phase.capture() { return; }
                moved.update(cx,|view,cx| {
                    if view.track_layout.drag.is_none() { return; }
                    if event.pressed_button != Some(MouseButton::Left) { view.cancel_track_resize(cx); }
                    else { view.update_track_resize(f32::from(event.position.x),f32::from(event.position.y),cx); }
                    cx.stop_propagation();
                });
            });
            let released = entity.clone();
            window.on_mouse_event(move |event:&MouseUpEvent,phase,_,cx| {
                if !phase.capture() || event.button != MouseButton::Left { return; }
                released.update(cx,|view,cx| {
                    if view.track_layout.drag.is_none() { return; }
                    view.update_track_resize(f32::from(event.position.x),f32::from(event.position.y),cx);
                    view.track_layout.drag = None;
                    cx.notify(); cx.stop_propagation();
                });
            });
        }).absolute().inset_0()
    }
}
