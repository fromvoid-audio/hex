//! Batched custom-canvas Arrangement track lane renderer and analytic pointer
//! dispatcher.
//!
//! Instead of instantiating per-clip GPUI `Div` / `Stateful<Div>` / `canvas`
//! subtrees, Taffy layout nodes, element-state maps, and hitboxes for thousands
//! of resting clips on every pan/zoom frame, each visible track lane paints all
//! resting clips directly in deterministic z-order `window.paint_layer` passes
//! and resolves pointer hits analytically in `O(log N)` against
//! [`super::arrangement_clip_index::ArrangementClipIndex`].
//!
//! Clips with active live widgets (inline rename `TextInput`, active trim or
//! scope preview, time-range exclusions, fade handles when hovering or holding
//! fade mode, or active gain drag/readout) continue to mount their existing
//! overlay controls on top of the canvas.
use super::arrangement_clip_index::{IndexedClip, IndexedClipKind, TrackClipIndex};
use super::clip_boundary;
use super::drag::{self, ClipDrag, ClipPosition};
use super::musical_time::GridLine;
use super::tracks::narrow_clips;
use super::trim::{self, LoopHandleKind, loop_handles};
use super::viewport::Viewport;
use super::waveform::{self, fade_envelope_samples, projected_fade_envelope_samples};
use super::{EditorView, ViewState};
use crate::theme;
use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, Corners, CursorStyle, Div, Edges, Element,
    ElementId, Entity, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, PathBuilder, Pixels, Point, Style, Window, div,
    fill, point, prelude::*, px, quad, rgb, rgba, size,
};
use hex_project::{Clip, ClipId, MidiClip, Track, TrackId, TrimEdge};
use std::{rc::Rc, sync::Arc};

const MAX_DENSE_COLUMNS: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HoveredArrangementClip {
    pub(super) generation: u64,
    pub(super) track: TrackId,
    pub(super) clip: ClipId,
    pub(super) zone: LaneHitZone,
    pub(super) repeat_available: bool,
    pub(super) cursor: CursorStyle,
}

impl HoveredArrangementClip {
    pub(super) fn resolved_cursor(self, option_held: bool) -> CursorStyle {
        match self.zone {
            LaneHitZone::TrimRight if option_held && self.repeat_available => CursorStyle::Loop,
            LaneHitZone::TrimRight => drag::rectangle_part(Some(TrimEdge::Right)).cursor(),
            LaneHitZone::GainHandle if option_held => CursorStyle::ResizeUpDown,
            LaneHitZone::GainHandle => self.cursor,
            _ => self.cursor,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaneHitZone {
    GainHandle,
    TrimLeft,
    TrimRight,
    LoopHandle(LoopHandleKind),
    Header,
    MoveSurface,
    Body,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LaneHit {
    pub(super) clip_id: ClipId,
    pub(super) is_midi: bool,
    pub(super) clip_start: u64,
    pub(super) clip_end: u64,
    pub(super) visible_start: u64,
    pub(super) visible_end: u64,
    pub(super) clip_left: f32,
    pub(super) clip_width: f32,
    pub(super) raw_clip_left: f32,
    pub(super) raw_clip_width: f32,
    pub(super) zone: LaneHitZone,
    pub(super) repeat_available: bool,
    pub(super) cursor: CursorStyle,
}

#[derive(Clone, Copy)]
struct NarrowInk {
    color: u32,
    selected: bool,
}

struct WideClipPlan {
    id: ClipId,
    is_midi: bool,
    index: usize,
    #[cfg(feature = "ui-test-support")]
    clip_start: u64,
    #[cfg(feature = "ui-test-support")]
    clip_end: u64,
    visible_start: u64,
    visible_end: u64,
    bounds: Bounds<Pixels>,
    pixel_width: f32,
    header_fill: u32,
    body_fill: u32,
    outline: u32,
    hover_outline: u32,
    waveform_ink: u32,
    selected: bool,
    hovered: bool,
    active: bool,
    border: bool,
    notched: bool,
    shows_detail: bool,
    looping: bool,
}

pub(super) struct TrackLaneCanvas {
    editor: Entity<EditorView>,
    snapshot: Arc<ViewState>,
    track_id: TrackId,
    track_index: usize,
    viewport: Viewport,
    grid: Rc<[GridLine]>,
    /// Clip IDs excluded from canvas painting (rendered by live overlays or hidden).
    excluded: Rc<[ClipId]>,
    is_committed_edit_view: bool,
}

impl TrackLaneCanvas {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        editor: Entity<EditorView>,
        snapshot: Arc<ViewState>,
        track_id: TrackId,
        track_index: usize,
        viewport: Viewport,
        grid: Rc<[GridLine]>,
        excluded: Rc<[ClipId]>,
        is_committed_edit_view: bool,
    ) -> Self {
        Self {
            editor,
            snapshot,
            track_id,
            track_index,
            viewport,
            grid,
            excluded,
            is_committed_edit_view,
        }
    }
}

impl IntoElement for TrackLaneCanvas {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TrackLaneCanvas {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            position: gpui::Position::Absolute,
            inset: gpui::Edges::all(gpui::Length::Definite(px(0.0).into())),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_lane(bounds, window, cx);
    }
}

fn snap_px(val: f64, scale: f32) -> f32 {
    let s = f64::from(scale.max(1.0));
    ((val * s).round() / s) as f32
}

fn can_reserve_layer(bounds: Bounds<Pixels>, mask: Bounds<Pixels>, scale: f32) -> bool {
    let clipped = bounds.intersect(&mask);
    let reserved = clipped.scale(scale);
    !clipped.is_empty()
        && !reserved.is_empty()
        && [
            reserved.left(),
            reserved.right(),
            reserved.top(),
            reserved.bottom(),
        ]
        .into_iter()
        .all(|edge| f64::from(edge).is_finite())
}

fn with_lane_layer(bounds: Bounds<Pixels>, window: &mut Window, f: impl FnOnce(&mut Window)) {
    let mask = window.content_mask().bounds;
    if can_reserve_layer(bounds, mask, window.scale_factor()) {
        window.paint_layer(bounds, f);
    } else {
        f(window);
    }
}

fn paint_narrow_columns(
    narrow_spans: &[(f64, f64, NarrowInk)],
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    if narrow_spans.is_empty() {
        return;
    }
    let visible = bounds.intersect(&window.content_mask().bounds);
    if visible.is_empty() {
        return;
    }
    let scale = f64::from(window.scale_factor());
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }
    let left = f64::from(bounds.left()) * scale;
    let width = f64::from(bounds.size.width) * scale;
    let first = (f64::from(visible.left()) * scale).floor() as i32;
    let last = (f64::from(visible.right()) * scale).ceil() as i32;
    if last <= first {
        return;
    }
    let column_count = usize::try_from(last.saturating_sub(first)).unwrap_or(usize::MAX);
    let emit_run = |window: &mut Window, start: i32, end: i32, color: u32| {
        let bar = Bounds::new(
            point(px((f64::from(start) / scale) as f32), bounds.top()),
            size(
                px((f64::from(end - start) / scale) as f32),
                bounds.size.height,
            ),
        )
        .intersect(&visible);
        if !bar.is_empty() {
            window.paint_quad(fill(bar, rgb(color)));
        }
    };
    if column_count <= MAX_DENSE_COLUMNS {
        let mut columns: Vec<Option<NarrowInk>> = vec![None; column_count];
        for &(span_start, span_end, ink) in narrow_spans {
            let start = (left + span_start * width).floor() as i32;
            let end = (left + span_end * width).ceil() as i32;
            let clamped_start = start.max(first);
            let clamped_end = end.min(last);
            if clamped_start >= clamped_end {
                continue;
            }
            let slice_start = (clamped_start - first) as usize;
            let slice_end = (clamped_end - first) as usize;
            for slot in &mut columns[slice_start..slice_end] {
                match slot {
                    Some(existing) if !ink.selected && existing.selected => {}
                    _ => *slot = Some(ink),
                }
            }
        }
        let mut idx = 0;
        while idx < columns.len() {
            let Some(ink) = columns[idx] else {
                idx += 1;
                continue;
            };
            let start_idx = idx;
            idx += 1;
            while idx < columns.len() && columns[idx].is_some_and(|next| next.color == ink.color) {
                idx += 1;
            }
            emit_run(
                window,
                first + start_idx as i32,
                first + idx as i32,
                ink.color,
            );
        }
    } else {
        let mut columns = std::collections::BTreeMap::<i32, NarrowInk>::new();
        for &(span_start, span_end, ink) in narrow_spans {
            let start = (left + span_start * width).floor() as i32;
            let end = (left + span_end * width).ceil() as i32;
            for column in start.max(first)..end.min(last) {
                let existing = columns.entry(column).or_insert(ink);
                if ink.selected || !existing.selected {
                    *existing = ink;
                }
            }
        }
        let mut iter = columns.into_iter().peekable();
        while let Some((start, ink)) = iter.next() {
            let mut end = start + 1;
            while iter
                .next_if(|(column, next)| *column == end && next.color == ink.color)
                .is_some()
            {
                end += 1;
            }
            emit_run(window, start, end, ink.color);
        }
    }
}

fn paint_fade_overlay(
    clip_bounds: Bounds<Pixels>,
    samples: &[(f32, f32)],
    editing: bool,
    clip_active: bool,
    window: &mut Window,
) {
    if samples.len() < 2 {
        return;
    }
    let top = f32::from(clip_bounds.top()) + super::clip_name::CLIP_NAME_HEIGHT;
    let bottom = f32::from(clip_bounds.bottom());
    let left = f32::from(clip_bounds.left());
    let width = f32::from(clip_bounds.size.width);
    let height = bottom - top;
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let mut shade = PathBuilder::fill();
    if let Some(&(x, _)) = samples.first() {
        shade.move_to(point(px(left + width * x), px(top)));
    }
    let mut path = editing.then(|| PathBuilder::stroke(px(1.25)));
    for (index, &(x, gain)) in samples.iter().enumerate() {
        let pt = point(px(left + width * x), px(top + height * (1.0 - gain)));
        if let Some(path) = path.as_mut() {
            if index == 0 {
                path.move_to(pt);
            } else {
                path.line_to(pt);
            }
        }
        shade.line_to(pt);
    }
    if let Some(&(x, _)) = samples.last() {
        shade.line_to(point(px(left + width * x), px(top)));
    }
    shade.close();
    if let Ok(shade) = shade.build() {
        let ink = Hsla::from(rgba(theme::CLIP_FADE_SHADE))
            .opacity(theme::clip_detail_opacity(clip_active));
        window.paint_path(shade, ink);
    }
    if let Some(path) = path
        && let Ok(path) = path.build()
    {
        window.paint_path(path, rgba(0x000000e0));
    }
}

fn paint_alias_badge(
    clip_id: ClipId,
    clip_bounds: Bounds<Pixels>,
    fill_color: u32,
    notched: bool,
    window: &mut Window,
) {
    let _ = clip_id;
    let ink = rgb(theme::readable_text(fill_color, 0x000000));
    let right_inset = if notched { 7.0 } else { 2.0 };
    let badge_left = clip_bounds.right() - px(right_inset + 10.0);
    let badge_top = clip_bounds.top() + px(2.0);
    if badge_left < clip_bounds.left() {
        return;
    }
    let sq1 = Bounds::new(
        point(badge_left, badge_top + px(3.0)),
        size(px(7.0), px(6.0)),
    );
    let sq2 = Bounds::new(
        point(badge_left + px(3.0), badge_top),
        size(px(7.0), px(6.0)),
    );
    for sq in [sq1, sq2] {
        window.paint_quad(quad(
            sq,
            Corners::all(px(1.0)),
            gpui::transparent_black(),
            Edges::all(px(1.0)),
            ink,
            Default::default(),
        ));
    }
    #[cfg(feature = "ui-test-support")]
    window.record_debug_bounds(
        format!("clip-alias-{}", clip_id.value()),
        Bounds::new(point(badge_left, badge_top), size(px(10.0), px(9.0))),
    );
}

#[cfg(feature = "ui-test-support")]
fn record_narrow_clip_debug_bounds(
    window: &mut Window,
    clip_id: ClipId,
    is_midi: bool,
    clip_bounds: Bounds<Pixels>,
    header_hit_height: f32,
    drag_available: bool,
) {
    let visual_sel = if is_midi {
        format!("midi-clip-visual-{}", clip_id.value())
    } else {
        format!("clip-visual-{}", clip_id.value())
    };
    window.record_debug_bounds(visual_sel, clip_bounds);
    let header_bounds = Bounds::new(
        clip_bounds.origin,
        size(
            clip_bounds.size.width,
            px(super::clip_name::CLIP_NAME_HEIGHT).min(clip_bounds.size.height),
        ),
    );
    window.record_debug_bounds(format!("clip-name-{}", clip_id.value()), header_bounds);
    if drag_available {
        let move_bounds = Bounds::new(
            clip_bounds.origin,
            size(
                clip_bounds.size.width,
                px(header_hit_height).min(clip_bounds.size.height),
            ),
        );
        window.record_debug_bounds(
            format!("clip-drag-surface-{}", clip_id.value()),
            move_bounds,
        );
    }
}

fn clip_flavor_active(
    view: &EditorView,
    track: &Track,
    entry: IndexedClip,
) -> (hex_project::ClipFlavor, bool) {
    match entry.kind {
        IndexedClipKind::Audio(i) => track
            .clips()
            .get(i)
            .map_or((hex_project::ClipFlavor::Muted, true), |c| {
                (c.flavor(), view.displayed_clip_active(c.id(), c.active()))
            }),
        IndexedClipKind::Midi(i) => track
            .midi_clips()
            .get(i)
            .map_or((hex_project::ClipFlavor::Muted, true), |c| {
                (c.flavor(), view.displayed_clip_active(c.id(), c.active()))
            }),
    }
}

mod interaction;
mod paint;
