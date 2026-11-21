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

impl TrackLaneCanvas {
    fn paint_lane(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let visible = bounds.intersect(&window.content_mask().bounds);
        if visible.is_empty() || self.viewport.length == 0 {
            return;
        }
        let Some(viewport_end) = self.viewport.end() else {
            return;
        };
        let lane_width = f32::from(bounds.size.width);
        let lane_height = f32::from(bounds.size.height);
        if !lane_width.is_finite()
            || lane_width <= 0.0
            || !lane_height.is_finite()
            || lane_height <= 0.0
        {
            return;
        }
        let scale = window.scale_factor().max(1.0);
        let stroke = 1.0 / scale;

        let view = self.editor.read(cx);
        let snapshot_matches = view
            .state
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &self.snapshot))
            || (self.is_committed_edit_view
                && view
                    .arrangement_duplicate_view()
                    .or_else(|| view.committed_section_copy_view())
                    .or_else(|| view.arrangement_paste_view())
                    .or_else(|| view.time_range_copy_view())
                    .is_some_and(|s| Arc::ptr_eq(s, &self.snapshot)));
        if !snapshot_matches {
            return;
        }
        view.arrangement_clip_index
            .borrow_mut()
            .ensure_current(view.project_generation, &self.snapshot);
        let Some(track) = self
            .snapshot
            .project
            .tracks()
            .get(self.track_index)
            .filter(|t| t.id() == self.track_id)
        else {
            return;
        };

        let index_ref = view.arrangement_clip_index.borrow();
        let Some(track_index_data) = index_ref.track(self.track_id) else {
            return;
        };
        let track_color = track.color();
        let track_height = view.track_height(self.track_id);
        let track_body_height = view.track_body_height(self.track_id);
        let hovered_clip = view
            .hovered_arrangement_clip
            .filter(|h| h.generation == view.project_generation && h.track == self.track_id)
            .map(|h| h.clip);
        #[cfg(feature = "ui-test-support")]
        let drag_available = view.drag_available();
        #[cfg(feature = "ui-test-support")]
        let trim_admitted = trim::drag_admitted(view);
        #[cfg(feature = "ui-test-support")]
        let header_hit_height = super::time_selection::header_hit_height(lane_height);
        #[cfg(feature = "ui-test-support")]
        let edge_trim_h = trim::edge_trim_height(lane_height);
        #[cfg(feature = "ui-test-support")]
        let fade_key_held = view.fade_key_held;
        #[cfg(feature = "ui-test-support")]
        let time_selection_key_held = view.time_selection_key_held;
        let rem_size = f32::from(window.rem_size());
        let font_size = px(rem_size * 0.75);
        let line_height = px(super::clip_name::CLIP_NAME_HEIGHT);

        let frames_per_dev_px =
            self.viewport.length as f64 / (f64::from(lane_width) * f64::from(scale));
        let mut narrow_spans: Vec<(f64, f64, NarrowInk)> = Vec::new();
        let mut narrow_fades: Vec<(usize, Bounds<Pixels>, u64, u64, bool)> = Vec::new();
        let mut wide_clips: Vec<WideClipPlan> = Vec::new();

        let is_excluded = |id: ClipId| self.excluded.contains(&id);

        // Ultra-dense LOD fast path: when an entire bin fits inside a sub-pixel
        // column and contains no selected/hovered/excluded/focus-filtered clips,
        // emit one narrow span for the whole bin in O(1).
        let use_lod = !view.arrangement_focus.projection.enabled
            && self.excluded.is_empty()
            && view.selected_clip().is_none()
            && view.selected_clip_ids().is_empty()
            && hovered_clip.is_none();

        if use_lod && let Some(lod_bins) = track_index_data.lod_level_for_pixel(frames_per_dev_px) {
            for bin in TrackClipIndex::candidate_bins(lod_bins, self.viewport.start, viewport_end) {
                if bin.end <= self.viewport.start || bin.start >= viewport_end {
                    continue;
                }
                let vis_start = bin.start.max(self.viewport.start);
                let vis_end = bin.end.min(viewport_end);
                if vis_start >= vis_end {
                    continue;
                }
                let left_frac =
                    (vis_start - self.viewport.start) as f64 / self.viewport.length as f64;
                let width_frac = (vis_end - vis_start) as f64 / self.viewport.length as f64;
                let rep = track_index_data.clips.get(bin.representative as usize);
                let (flavor, active) = rep
                    .map(|c| clip_flavor_active(view, track, *c))
                    .unwrap_or((hex_project::ClipFlavor::Muted, true));
                let header_fill = theme::clip_activity_fill(track_color, flavor, false, active);
                let fill_color =
                    theme::arrangement_clip_activity_body_fill(header_fill, false, active);
                narrow_spans.push((
                    left_frac,
                    left_frac + width_frac,
                    NarrowInk {
                        color: fill_color,
                        selected: false,
                    },
                ));
            }
        } else {
            let candidates = track_index_data.candidate_slice(self.viewport.start, viewport_end);
            for &entry in candidates {
                if entry.end <= self.viewport.start || entry.start >= viewport_end {
                    continue;
                }
                if is_excluded(entry.id)
                    || (!self.is_committed_edit_view && !view.arrangement_clip_visible(entry.id))
                {
                    continue;
                }
                let vis_start = entry.start.max(self.viewport.start);
                let vis_end = entry.end.min(viewport_end);
                if vis_start >= vis_end {
                    continue;
                }
                let left_frac =
                    (vis_start - self.viewport.start) as f64 / self.viewport.length as f64;
                let width_frac = (vis_end - vis_start) as f64 / self.viewport.length as f64;
                let pixel_width = trim::rendered_clip_width(lane_width, width_frac);
                let selected = view.clip_is_selected(entry.id);
                let (flavor, active, is_midi, idx, looping) = match entry.kind {
                    IndexedClipKind::Audio(i) => {
                        let Some(clip) = track.clips().get(i) else {
                            continue;
                        };
                        let active = view.displayed_clip_active(clip.id(), clip.active());
                        (
                            clip.flavor(),
                            active,
                            false,
                            i,
                            loop_handles::audio_repeats(clip),
                        )
                    }
                    IndexedClipKind::Midi(i) => {
                        let Some(clip) = track.midi_clips().get(i) else {
                            continue;
                        };
                        let active = view.displayed_clip_active(clip.id(), clip.active());
                        (clip.flavor(), active, true, i, clip.looping())
                    }
                };
                let header_fill = theme::clip_activity_fill(track_color, flavor, false, active);
                let body_fill =
                    theme::arrangement_clip_activity_body_fill(header_fill, selected, active);
                let narrow = narrow_clips::is_narrow(pixel_width, scale);
                let lane_left_f64 = f64::from(bounds.left());
                let raw_left = lane_left_f64 + left_frac * f64::from(lane_width);
                let raw_right = lane_left_f64 + (left_frac + width_frac) * f64::from(lane_width);
                // Match Taffy's physical-pixel rounding so resting canvas clips
                // and promoted overlay clips share identical endpoints at every scale.
                let snapped_left = snap_px(raw_left, scale);
                let snapped_right = snap_px(raw_right, scale);
                let clip_bounds = if narrow {
                    Bounds::new(
                        point(px(snapped_left), bounds.top()),
                        size(
                            px((snapped_right - snapped_left).max(stroke)),
                            bounds.size.height,
                        ),
                    )
                } else {
                    Bounds::new(
                        point(px(snapped_left), bounds.top()),
                        size(
                            px((snapped_right - snapped_left).max(0.0)),
                            bounds.size.height,
                        ),
                    )
                };
                let resolved_width = f32::from(clip_bounds.size.width);

                if narrow {
                    narrow_spans.push((
                        left_frac,
                        left_frac + width_frac,
                        NarrowInk {
                            color: body_fill,
                            selected,
                        },
                    ));
                    if !is_midi
                        && track_height > super::track_layout::HEADER_ONLY_HEIGHT
                        && let Some(audio_clip) = track.clips().get(idx)
                    {
                        let fade = view
                            .fade_preview(audio_clip.id())
                            .unwrap_or(audio_clip.retained_fade());
                        if !fade.is_neutral() {
                            narrow_fades.push((idx, clip_bounds, vis_start, vis_end, active));
                        }
                    }
                    #[cfg(feature = "ui-test-support")]
                    {
                        record_narrow_clip_debug_bounds(
                            window,
                            entry.id,
                            is_midi,
                            clip_bounds,
                            header_hit_height,
                            drag_available,
                        );
                    }
                    continue;
                }

                let border = hex_widgets::rectangle_border_width(resolved_width) > 0.0;
                let notched = border && vis_end == entry.end;
                let outline = theme::clip_activity_outline(
                    active,
                    if selected {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::ClipOutline
                    },
                );
                let hover_outline = if is_midi {
                    outline
                } else {
                    theme::clip_activity_outline(
                        active,
                        if selected {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::Text
                        },
                    )
                };
                let waveform_ink = if is_midi {
                    0
                } else {
                    theme::clip_waveform_ink(
                        track_color,
                        theme::ClipWaveformBackground::Arrangement { flavor, selected },
                        active,
                    )
                };
                let height_allows_detail = if is_midi {
                    track_body_height > super::track_layout::HEADER_ONLY_HEIGHT
                } else {
                    track_height > super::track_layout::HEADER_ONLY_HEIGHT
                };
                let shows_detail = height_allows_detail
                    && narrow_clips::shows_body_detail(
                        lane_width,
                        entry.end - entry.start,
                        self.viewport.length,
                    );
                wide_clips.push(WideClipPlan {
                    id: entry.id,
                    is_midi,
                    index: idx,
                    #[cfg(feature = "ui-test-support")]
                    clip_start: entry.start,
                    #[cfg(feature = "ui-test-support")]
                    clip_end: entry.end,
                    visible_start: vis_start,
                    visible_end: vis_end,
                    bounds: clip_bounds,
                    pixel_width: resolved_width,
                    header_fill,
                    body_fill,
                    outline,
                    hover_outline,
                    waveform_ink,
                    selected,
                    hovered: hovered_clip == Some(entry.id),
                    active,
                    border,
                    notched,
                    shows_detail,
                    looping,
                });
            }
        }

        // Record narrow-clip-bars debug selector when narrow bars exist.
        #[cfg(feature = "ui-test-support")]
        if !narrow_spans.is_empty() {
            window.record_debug_bounds(
                format!("narrow-clip-bars-{}", self.track_id.value()),
                bounds,
            );
        }

        // PASS 1: Narrow bars + Wide clip backdrops (fills, headers, borders,
        // notches, header gradients) + unselected trailing edges in ONE paint_layer.
        with_lane_layer(bounds, window, |window| {
            paint_narrow_columns(&narrow_spans, bounds, window);
            for clip in &wide_clips {
                let outline_color = Hsla::from(rgb(if clip.hovered {
                    clip.hover_outline
                } else {
                    clip.outline
                }));
                if clip.border {
                    let cut = if clip.notched {
                        clip_boundary::corner_extent(clip.pixel_width, lane_height)
                    } else {
                        0.0
                    };
                    let header_gradient = if clip.is_midi {
                        track.midi_clips().get(clip.index).and_then(|midi_clip| {
                            if let Some(values) =
                                view.midi_header_loop_values(midi_clip.id(), self.snapshot.revision)
                            {
                                super::clip_header_loop::midi_source(
                                    midi_clip,
                                    values,
                                    self.snapshot.project.tempo_map(),
                                    clip.visible_start..clip.visible_end,
                                    clip.pixel_width * scale,
                                )
                            } else if clip.looping {
                                super::clip_header_loop::midi(
                                    midi_clip,
                                    None,
                                    self.snapshot.project.tempo_map(),
                                    clip.visible_start..clip.visible_end,
                                    clip.pixel_width * scale,
                                )
                            } else {
                                None
                            }
                        })
                    } else if clip.looping {
                        track.clips().get(clip.index).and_then(|audio_clip| {
                            super::clip_header_loop::audio(
                                audio_clip,
                                self.snapshot.project.tempo_map(),
                                clip.visible_start..clip.visible_end,
                                clip.pixel_width * scale,
                            )
                        })
                    } else {
                        None
                    };
                    clip_boundary::paint_backdrop_with_cut(
                        clip.bounds,
                        clip.body_fill,
                        clip.header_fill,
                        stroke,
                        super::clip_name::CLIP_NAME_HEIGHT,
                        header_gradient.as_ref(),
                        cut,
                        outline_color,
                        window,
                    );
                    // Header bottom divider line (matches clip_name_surface border_b_1).
                    let text_rgb = theme::readable_text(
                        clip.header_fill,
                        theme::color(theme::PaletteRole::Text),
                    );
                    let header_h =
                        px(super::clip_name::CLIP_NAME_HEIGHT).min(clip.bounds.size.height);
                    let header_bounds = Bounds::from_corners(
                        point(clip.bounds.left() + px(stroke - 1.0), clip.bounds.top()),
                        point(clip.bounds.right() + px(1.0), clip.bounds.top() + header_h),
                    );
                    let inner_mask = Bounds::from_corners(
                        point(clip.bounds.left() + px(stroke), clip.bounds.top() + px(1.0)),
                        point(clip.bounds.right(), clip.bounds.bottom() - px(1.0)),
                    );
                    window.with_content_mask(Some(ContentMask { bounds: inner_mask }), |window| {
                        window.paint_quad(quad(
                            header_bounds,
                            Corners::default(),
                            gpui::transparent_black(),
                            Edges {
                                top: px(0.0),
                                right: px(0.0),
                                bottom: px(1.0),
                                left: px(0.0),
                            },
                            rgba((text_rgb << 8) | 0x70),
                            Default::default(),
                        ));
                    });
                    if !clip.selected {
                        clip_boundary::paint_trailing_with_cut(
                            clip.bounds,
                            stroke,
                            clip.notched.then_some(cut),
                            outline_color,
                            window,
                        );
                    }
                } else {
                    // Sub-border wide clip (> 4 device px, <= rectangle border threshold):
                    // solid body + header + leading/trailing physical hairlines.
                    window.paint_quad(fill(clip.bounds, rgb(clip.body_fill)));
                    let header_rect = Bounds::new(
                        clip.bounds.origin,
                        size(
                            clip.bounds.size.width,
                            px(super::clip_name::CLIP_NAME_HEIGHT).min(clip.bounds.size.height),
                        ),
                    );
                    window.paint_quad(fill(header_rect, rgb(clip.header_fill)));
                    let leading = Bounds::new(
                        clip.bounds.origin,
                        size(px(stroke), clip.bounds.size.height),
                    );
                    window.paint_quad(fill(leading, outline_color));
                    if !clip.selected {
                        let trailing = Bounds::new(
                            point(clip.bounds.right(), clip.bounds.top()),
                            size(px(stroke), clip.bounds.size.height),
                        );
                        window.paint_quad(fill(trailing, outline_color));
                    }
                }
            }
        });

        // PASS 2: Clip-internal grid lines & loop repeat markers (single paint_layer).
        let any_detail_or_loop = wide_clips.iter().any(|c| c.shows_detail || c.looping);
        if any_detail_or_loop {
            with_lane_layer(bounds, window, |window| {
                let grid_color = Hsla::from(rgb(theme::color(theme::PaletteRole::Grid)));
                let meter_color = Hsla::from(rgb(theme::color(theme::PaletteRole::Primary)));
                for clip in &wide_clips {
                    let detail_opacity = theme::clip_detail_opacity(clip.active);
                    if clip.shows_detail {
                        let body_top = clip.bounds.top() + px(super::clip_name::CLIP_NAME_HEIGHT);
                        let body_h = (clip.bounds.bottom() - body_top).max(px(0.0));
                        let span = clip.visible_end.saturating_sub(clip.visible_start);
                        if body_h > px(0.0) && span > 0 {
                            let first_line =
                                self.grid.partition_point(|l| l.frame <= clip.visible_start);
                            #[cfg(feature = "ui-test-support")]
                            let mut any_line = false;
                            for line in &self.grid[first_line..] {
                                if line.frame >= clip.visible_end {
                                    break;
                                }
                                #[cfg(feature = "ui-test-support")]
                                {
                                    any_line = true;
                                }
                                // Compute X directly in lane coordinates rounded to
                                // integer logical pixels so clip grid lines share the
                                // exact physical column as the lane's background grid.
                                let lane_frac = (line.frame.saturating_sub(self.viewport.start))
                                    as f64
                                    / self.viewport.length as f64;
                                let x = px(snap_px(
                                    f64::from(bounds.left()) + lane_frac * f64::from(lane_width),
                                    scale,
                                ));
                                let rect =
                                    Bounds::new(point(x, body_top), size(px(stroke), body_h));
                                let alpha = if line.meter_marker || line.bar_start {
                                    0.42
                                } else {
                                    0.24
                                } * detail_opacity;
                                let color = if line.meter_marker {
                                    meter_color.opacity(alpha)
                                } else {
                                    grid_color.opacity(alpha)
                                };
                                window.paint_quad(fill(rect, color));
                                #[cfg(feature = "ui-test-support")]
                                window.record_debug_bounds(
                                    format!("clip-grid-line-{}", line.frame),
                                    rect,
                                );
                            }
                            #[cfg(feature = "ui-test-support")]
                            if any_line {
                                let grid_bounds = Bounds::new(
                                    point(clip.bounds.left(), body_top),
                                    size(clip.bounds.size.width, body_h),
                                );
                                window.record_debug_bounds("clip-musical-grid", grid_bounds);
                            }
                        }
                    }
                    if clip.looping {
                        if clip.is_midi {
                            if let Some(midi_clip) = track.midi_clips().get(clip.index) {
                                loop_handles::paint_midi_markers(
                                    clip.bounds,
                                    midi_clip,
                                    &self.snapshot.project,
                                    clip.visible_start,
                                    clip.visible_end,
                                    detail_opacity,
                                    window,
                                );
                            }
                        } else if let Some(audio_clip) = track.clips().get(clip.index) {
                            loop_handles::paint_audio_markers(
                                clip.bounds,
                                audio_clip,
                                &self.snapshot.project,
                                clip.visible_start,
                                clip.visible_end,
                                detail_opacity,
                                window,
                            );
                        }
                    }
                }
            });
        }

        let title_cache_rc = Rc::clone(&view.clip_title_cache);
        let mut missing_labels: Vec<(&'static str, Bounds<Pixels>)> = Vec::new();
        let mut alias_flags: Vec<bool> = Vec::with_capacity(wide_clips.len());
        for clip in &wide_clips {
            alias_flags.push(view.clip_has_alias(clip.id));
        }

        // PASS 3: Audio waveforms, MIDI notes/tiles, and non-raised fade overlays.
        let waveform_grid = view.waveform_grid_for(self.track_id, self.viewport, scale);
        for clip in &wide_clips {
            if !clip.is_midi {
                let Some(audio_clip) = track.clips().get(clip.index) else {
                    continue;
                };
                if clip.shows_detail {
                    let y_border = if clip.border { 1.0 } else { 0.0 };
                    let body_top =
                        clip.bounds.top() + px(y_border + super::clip_name::CLIP_NAME_HEIGHT);
                    let body_bottom = (clip.bounds.bottom() - px(y_border)).max(body_top);
                    let body_bounds = Bounds::new(
                        point(
                            clip.bounds.left() + px(if clip.border { stroke } else { 0.0 }),
                            body_top,
                        ),
                        size(
                            px((clip.pixel_width - if clip.border { stroke } else { 0.0 })
                                .max(0.0)),
                            body_bottom - body_top,
                        ),
                    );
                    if let Some((waveform_data, mapping)) = view.resolve_audio_waveform_source(
                        &self.snapshot,
                        audio_clip,
                        clip.visible_start,
                        clip.visible_end,
                    ) {
                        waveform::paint_timeline_waveform_cached(
                            body_bounds,
                            &waveform_data,
                            &mapping,
                            clip.visible_start,
                            clip.visible_end,
                            &view.waveform_geometry_cache,
                            waveform_grid,
                            clip.waveform_ink,
                            clip.active,
                            window,
                        );
                    } else {
                        let label = if audio_clip.audio_edit().is_some() {
                            "prepared waveform unavailable"
                        } else {
                            "missing media"
                        };
                        missing_labels.push((label, body_bounds));
                    }
                    #[cfg(feature = "ui-test-support")]
                    window.record_debug_bounds("clip-waveform-body", body_bounds);
                }
                // Non-raised fade overlay.
                let fade = view
                    .fade_preview(audio_clip.id())
                    .unwrap_or(audio_clip.retained_fade());
                if track_height > super::track_layout::HEADER_ONLY_HEIGHT
                    && clip.pixel_width > 0.0
                    && !fade.is_neutral()
                {
                    let samples = if audio_clip.audio_edit().is_some() {
                        projected_fade_envelope_samples(
                            audio_clip,
                            self.snapshot.project.tempo_map(),
                            clip.visible_start,
                            clip.visible_end,
                            None,
                        )
                        .unwrap_or_default()
                    } else {
                        fade_envelope_samples(
                            fade,
                            audio_clip.start(),
                            clip.visible_start,
                            clip.visible_end,
                        )
                    };
                    if samples.len() >= 2 {
                        paint_fade_overlay(clip.bounds, &samples, false, clip.active, window);
                        #[cfg(feature = "ui-test-support")]
                        {
                            let shade_bounds = Bounds::new(
                                point(
                                    clip.bounds.left(),
                                    clip.bounds.top() + px(super::clip_name::CLIP_NAME_HEIGHT),
                                ),
                                size(
                                    clip.bounds.size.width,
                                    (clip.bounds.size.height
                                        - px(super::clip_name::CLIP_NAME_HEIGHT))
                                    .max(px(0.0)),
                                ),
                            );
                            window.record_debug_bounds(
                                format!("clip-fade-shade-{}", audio_clip.id().value()),
                                shade_bounds,
                            );
                        }
                    }
                }
            } else if clip.shows_detail {
                let Some(midi_clip) = track.midi_clips().get(clip.index) else {
                    continue;
                };
                let y_border = if clip.border { 1.0 } else { 0.0 };
                let body_top =
                    clip.bounds.top() + px(y_border + super::clip_name::CLIP_NAME_HEIGHT);
                let body_bottom = (clip.bounds.bottom() - px(y_border)).max(body_top);
                let body_bounds = Bounds::new(
                    point(
                        clip.bounds.left() + px(if clip.border { stroke } else { 0.0 }),
                        body_top,
                    ),
                    size(
                        px((clip.pixel_width - if clip.border { stroke } else { 0.0 }).max(0.0)),
                        body_bottom - body_top,
                    ),
                );
                let merge_frames = super::midi_loop_notes::merge_frames(
                    clip.visible_end.saturating_sub(clip.visible_start),
                    clip.pixel_width,
                    scale,
                );
                let visible_notes = match view.midi_visible_notes.try_borrow_mut() {
                    Ok(mut cache) => cache.get_tiled(
                        &self.snapshot.project,
                        midi_clip,
                        clip.visible_start,
                        clip.visible_end,
                        merge_frames,
                    ),
                    Err(_) => super::midi_loop_notes::visible_notes_tiled(
                        &self.snapshot.project,
                        midi_clip,
                        clip.visible_start,
                        clip.visible_end,
                        merge_frames,
                    )
                    .ok()
                    .map(Rc::new),
                };
                if let Some(visible_notes) = visible_notes {
                    let note_color = Hsla::from(rgb(theme::color(theme::PaletteRole::ClipText)))
                        .opacity(theme::clip_detail_opacity(clip.active));
                    let fit = visible_notes.fit;
                    let clipped_body = body_bounds.intersect(&window.content_mask().bounds);
                    if !clipped_body.is_empty() {
                        window.with_content_mask(
                            Some(ContentMask {
                                bounds: clipped_body,
                            }),
                            |window| {
                                if !visible_notes.intervals.is_empty() {
                                    with_lane_layer(body_bounds, window, |window| {
                                        for &(n_start, n_end, pitch) in &visible_notes.intervals {
                                            if let Some((x, w, pitch)) =
                                                super::midi_thumbnail::note_shape(
                                                    n_start,
                                                    n_end,
                                                    pitch,
                                                    clip.visible_start,
                                                    clip.visible_end,
                                                )
                                                && let Some(rect) =
                                                    super::midi_thumbnail::paint_bounds(
                                                        body_bounds,
                                                        x,
                                                        w,
                                                        pitch,
                                                        fit,
                                                        scale,
                                                    )
                                            {
                                                window.paint_quad(fill(rect, note_color));
                                            }
                                        }
                                    });
                                }
                                if visible_notes.tiles.is_some() {
                                    super::midi_loop_notes::paint_tiles(
                                        window,
                                        &view.midi_tile_images,
                                        &self.snapshot.project,
                                        midi_clip.id(),
                                        &visible_notes,
                                        body_bounds,
                                        (clip.visible_start, clip.visible_end),
                                    );
                                }
                            },
                        );
                    }
                    #[cfg(feature = "ui-test-support")]
                    {
                        window.record_debug_bounds("clip-waveform-body", body_bounds);
                        if !visible_notes.intervals.is_empty() || visible_notes.tiles.is_some() {
                            window.record_debug_bounds(
                                format!("clip-notes-{}", midi_clip.id().value()),
                                body_bounds,
                            );
                        }
                    }
                }
            }
        }
        for &(idx, clip_bounds, vis_start, vis_end, active) in &narrow_fades {
            let Some(audio_clip) = track.clips().get(idx) else {
                continue;
            };
            let fade = view
                .fade_preview(audio_clip.id())
                .unwrap_or(audio_clip.retained_fade());
            let samples = if audio_clip.audio_edit().is_some() {
                projected_fade_envelope_samples(
                    audio_clip,
                    self.snapshot.project.tempo_map(),
                    vis_start,
                    vis_end,
                    None,
                )
                .unwrap_or_default()
            } else {
                fade_envelope_samples(fade, audio_clip.start(), vis_start, vis_end)
            };
            if samples.len() >= 2 {
                paint_fade_overlay(clip_bounds, &samples, false, active, window);
                #[cfg(feature = "ui-test-support")]
                {
                    let shade_bounds = Bounds::new(
                        point(
                            clip_bounds.left(),
                            clip_bounds.top() + px(super::clip_name::CLIP_NAME_HEIGHT),
                        ),
                        size(
                            clip_bounds.size.width,
                            (clip_bounds.size.height - px(super::clip_name::CLIP_NAME_HEIGHT))
                                .max(px(0.0)),
                        ),
                    );
                    window.record_debug_bounds(
                        format!("clip-fade-shade-{}", audio_clip.id().value()),
                        shade_bounds,
                    );
                }
            }
        }
        // Record loop handle debug bounds for wide clips while `view` is still in scope.
        #[cfg(feature = "ui-test-support")]
        if drag_available && trim_admitted {
            let lane_bounds = super::drag::LaneBounds {
                left: f32::from(bounds.left()),
                top: f32::from(bounds.top()),
                width: lane_width,
                height: lane_height,
            };
            for clip in &wide_clips {
                if !clip.looping || trim::handle_width(clip.pixel_width) <= 0.0 {
                    continue;
                }
                let controls = if clip.is_midi {
                    track.midi_clips().get(clip.index).and_then(|c| {
                        loop_handles::midi_controls(
                            c,
                            &self.snapshot.project,
                            lane_bounds,
                            self.viewport,
                        )
                    })
                } else {
                    track.clips().get(clip.index).and_then(|c| {
                        loop_handles::audio_controls(
                            c,
                            &self.snapshot.project,
                            lane_bounds,
                            self.viewport,
                        )
                    })
                };
                if let Some(controls) = controls {
                    for handle in controls.handles.into_iter().flatten() {
                        let kind_str = match handle.kind {
                            LoopHandleKind::FirstBoundary => "cycle",
                            LoopHandleKind::Outer => "outer",
                        };
                        let sel = format!(
                            "{}clip-loop-{kind_str}-{}",
                            if clip.is_midi { "midi-" } else { "" },
                            clip.id.value()
                        );
                        let rect = Bounds::new(
                            point(
                                clip.bounds.left() + px(handle.left),
                                clip.bounds.top() + px(handle.top),
                            ),
                            size(px(loop_handles::WIDTH), px(handle.height)),
                        );
                        window.record_debug_bounds(sel, rect);
                    }
                }
            }
        }
        // Release immutable borrow on `cx` before painting cached `ShapedLine` titles.
        drop(index_ref);
        let _ = view;

        // PASS 4: Cached shaped clip titles, missing-media notices & alias badges in ONE paint_layer.
        if !wide_clips.is_empty() || !missing_labels.is_empty() {
            with_lane_layer(bounds, window, |window| {
                let mut title_cache = title_cache_rc.try_borrow_mut().ok();
                if let Some(cache) = title_cache.as_mut() {
                    let muted_color = theme::color(theme::PaletteRole::Muted);
                    for (label, body_bounds) in &missing_labels {
                        cache.paint_clipped(
                            label,
                            *body_bounds,
                            super::clip_title_cache::TitlePaintStyle {
                                font_size,
                                line_height,
                                color: muted_color,
                            },
                            window,
                            cx,
                        );
                    }
                }
                for (clip, &has_alias) in wide_clips.iter().zip(alias_flags.iter()) {
                    if has_alias && clip.pixel_width >= 14.0 {
                        paint_alias_badge(
                            clip.id,
                            clip.bounds,
                            clip.header_fill,
                            clip.notched,
                            window,
                        );
                    }
                    let omit_label = clip.notched
                        && super::clip_name::notched_label_is_clipped(
                            clip.pixel_width,
                            rem_size,
                            scale,
                        );
                    if omit_label {
                        continue;
                    }
                    let name = if clip.is_midi {
                        track.midi_clips().get(clip.index).map(MidiClip::name)
                    } else {
                        track.clips().get(clip.index).map(Clip::name)
                    };
                    let Some(name) = name else {
                        continue;
                    };
                    let pad_left = 4.0 + if clip.border { stroke } else { 0.0 };
                    let pad_right = if has_alias {
                        if clip.notched { 20.0 } else { 15.0 }
                    } else if clip.notched {
                        6.0
                    } else {
                        4.0
                    };
                    let text_width = clip.pixel_width - pad_left - pad_right;
                    if text_width <= 1.0 {
                        continue;
                    }
                    let text_bounds = Bounds::new(
                        point(clip.bounds.left() + px(pad_left), clip.bounds.top()),
                        size(
                            px(text_width),
                            px(super::clip_name::CLIP_NAME_HEIGHT).min(clip.bounds.size.height),
                        ),
                    );
                    let text_color = theme::readable_text(
                        clip.header_fill,
                        theme::color(theme::PaletteRole::Text),
                    );
                    if let Some(cache) = title_cache.as_mut() {
                        cache.paint_clipped(
                            name,
                            text_bounds,
                            super::clip_title_cache::TitlePaintStyle {
                                font_size,
                                line_height,
                                color: text_color,
                            },
                            window,
                            cx,
                        );
                    }
                    #[cfg(feature = "ui-test-support")]
                    window.record_debug_bounds(
                        format!("clip-name-label-{}", clip.id.value()),
                        text_bounds,
                    );
                }
            });
        }

        // PASS 5: Selected clip borders & trailing edges painted LAST so the
        // Primary selection stroke always wins over adjacent clips.
        let any_selected = wide_clips.iter().any(|c| c.selected);
        if any_selected {
            with_lane_layer(bounds, window, |window| {
                for clip in wide_clips.iter().filter(|c| c.selected) {
                    let outline_color = Hsla::from(rgb(clip.outline));
                    if clip.border {
                        let cut = if clip.notched {
                            clip_boundary::corner_extent(clip.pixel_width, lane_height)
                        } else {
                            0.0
                        };
                        clip_boundary::paint_trailing_with_cut(
                            clip.bounds,
                            stroke,
                            clip.notched.then_some(cut),
                            outline_color,
                            window,
                        );
                        #[cfg(feature = "ui-test-support")]
                        {
                            let edge = Bounds::new(
                                point(clip.bounds.right(), clip.bounds.top()),
                                size(px(stroke), clip.bounds.size.height),
                            );
                            let sel = if clip.is_midi {
                                format!("midi-clip-selected-trailing-{}", clip.id.value())
                            } else {
                                format!("clip-selected-trailing-{}", clip.id.value())
                            };
                            window.record_debug_bounds(sel, edge);
                        }
                    } else {
                        let trailing = Bounds::new(
                            point(clip.bounds.right(), clip.bounds.top()),
                            size(px(stroke), clip.bounds.size.height),
                        );
                        window.paint_quad(fill(trailing, outline_color));
                    }
                }
            });
        }

        // Record test-support debug bounds for wide clips (compiled out in release!).
        #[cfg(feature = "ui-test-support")]
        for clip in &wide_clips {
            let visual_sel = if clip.is_midi {
                format!("midi-clip-visual-{}", clip.id.value())
            } else {
                format!("clip-visual-{}", clip.id.value())
            };
            window.record_debug_bounds(visual_sel, clip.bounds);
            let trailing_sel = if clip.is_midi {
                format!("midi-clip-trailing-{}", clip.id.value())
            } else {
                format!("clip-trailing-{}", clip.id.value())
            };
            let trailing_edge = Bounds::new(
                point(clip.bounds.right(), clip.bounds.top()),
                size(px(stroke), clip.bounds.size.height),
            );
            window.record_debug_bounds(trailing_sel, trailing_edge);
            let header_bounds = if clip.border {
                Bounds::from_corners(
                    point(clip.bounds.left() + px(stroke - 1.0), clip.bounds.top()),
                    point(
                        clip.bounds.right() + px(1.0),
                        clip.bounds.top()
                            + px(super::clip_name::CLIP_NAME_HEIGHT).min(clip.bounds.size.height),
                    ),
                )
            } else {
                Bounds::new(
                    clip.bounds.origin,
                    size(
                        clip.bounds.size.width,
                        px(super::clip_name::CLIP_NAME_HEIGHT).min(clip.bounds.size.height),
                    ),
                )
            };
            window.record_debug_bounds(format!("clip-name-{}", clip.id.value()), header_bounds);
            if drag_available {
                let hw = if trim_admitted {
                    trim::handle_width(clip.pixel_width)
                } else {
                    0.0
                };
                let left_inset = if hw > 0.0 && clip.visible_start == clip.clip_start {
                    hw
                } else {
                    0.0
                };
                let right_inset = if hw > 0.0 && clip.visible_end == clip.clip_end {
                    hw
                } else {
                    0.0
                };
                let move_bounds = Bounds::new(
                    point(clip.bounds.left() + px(left_inset), clip.bounds.top()),
                    size(
                        px((clip.pixel_width - left_inset - right_inset).max(0.0)),
                        px(header_hit_height).min(clip.bounds.size.height),
                    ),
                );
                window.record_debug_bounds(
                    format!("clip-drag-surface-{}", clip.id.value()),
                    move_bounds,
                );
                if hw > 0.0 {
                    if clip.visible_start == clip.clip_start {
                        let left_rect = Bounds::new(
                            clip.bounds.origin,
                            size(px(hw), px(edge_trim_h).min(clip.bounds.size.height)),
                        );
                        let sel = if clip.is_midi {
                            format!("midi-clip-trim-left-{}", clip.id.value())
                        } else {
                            format!("clip-trim-left-{}", clip.id.value())
                        };
                        window.record_debug_bounds(sel, left_rect);
                    }
                    if clip.visible_end == clip.clip_end {
                        let right_rect = Bounds::new(
                            point(clip.bounds.right() - px(hw), clip.bounds.top()),
                            size(px(hw), px(edge_trim_h).min(clip.bounds.size.height)),
                        );
                        let sel = if clip.is_midi {
                            format!("midi-clip-trim-right-{}", clip.id.value())
                        } else {
                            format!("clip-trim-right-{}", clip.id.value())
                        };
                        window.record_debug_bounds(sel, right_rect);
                    }
                }
            }
            if !clip.is_midi
                && clip.pixel_width >= 28.0
                && track_height > super::track_layout::HEADER_ONLY_HEIGHT
                && !fade_key_held
                && !time_selection_key_held
            {
                let gain_rect = Bounds::new(
                    point(clip.bounds.left() + px(9.0), clip.bounds.top()),
                    size(
                        px((clip.pixel_width - 18.0).max(0.0)),
                        px(4.0).min(clip.bounds.size.height),
                    ),
                );
                window.record_debug_bounds(
                    format!("clip-gain-handle-{}", clip.id.value()),
                    gain_rect,
                );
            }
        }
    }
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

impl EditorView {
    /// Analytically hit-test resting clips on `track_id` in `O(log N)` using
    /// the revision-cached `ArrangementClipIndex`.
    pub(super) fn hit_test_lane_clip(
        &self,
        track_id: TrackId,
        position: Point<Pixels>,
        viewport: Viewport,
        scale: f32,
    ) -> Option<LaneHit> {
        let state = self.state.as_ref()?;
        let &lane = self.drag_lanes.get(&track_id)?;
        let viewport_end = viewport.end()?;
        if viewport.length == 0 || lane.width <= 0.0 || lane.height <= 0.0 {
            return None;
        }
        let px_x = f32::from(position.x);
        let px_y = f32::from(position.y);
        if px_x < lane.left
            || px_x > lane.left + lane.width
            || px_y < lane.top
            || px_y > lane.top + lane.height
        {
            return None;
        }
        let local_x = px_x - lane.left;
        let local_y = px_y - lane.top;
        let track = state.project.track(track_id)?;

        // Ensure index is populated (uses interior mutability on RefCell).
        {
            let mut idx = self.arrangement_clip_index.borrow_mut();
            idx.ensure_current(self.project_generation, state);
        }
        let index_ref = self.arrangement_clip_index.borrow();
        let track_index_data = index_ref.track(track_id)?;

        // Query a narrow frame window around pointer X (margin of 2 logical px)
        // so sub-pixel clips and boundary handles are always included.
        let margin_frames = ((2.0 / f64::from(lane.width)) * viewport.length as f64).ceil() as u64;
        let frac = (f64::from(local_x) / f64::from(lane.width)).clamp(0.0, 1.0);
        let pointer_frame = viewport
            .start
            .saturating_add((frac * viewport.length as f64) as u64);
        let query_start = pointer_frame
            .saturating_sub(margin_frames)
            .max(viewport.start);
        let query_end = pointer_frame
            .saturating_add(margin_frames.max(1))
            .min(viewport_end);

        let candidates = track_index_data.candidate_slice(query_start, query_end);
        let trim_admitted = trim::drag_admitted(self);
        let header_hit_h = super::time_selection::header_hit_height(lane.height);
        let edge_trim_h = trim::edge_trim_height(lane.height);
        let track_height = self.track_height(track_id);

        // Reverse order so later-painted clips win ties just like DOM order.
        for &entry in candidates.iter().rev() {
            if entry.end <= viewport.start || entry.start >= viewport_end {
                continue;
            }
            if !self.arrangement_clip_visible(entry.id) {
                continue;
            }
            let vis_start = entry.start.max(viewport.start);
            let vis_end = entry.end.min(viewport_end);
            if vis_start >= vis_end {
                continue;
            }
            let left_frac = (vis_start - viewport.start) as f64 / viewport.length as f64;
            let width_frac = (vis_end - vis_start) as f64 / viewport.length as f64;
            let raw_left = f64::from(lane.left) + left_frac * f64::from(lane.width);
            let raw_right = f64::from(lane.left) + (left_frac + width_frac) * f64::from(lane.width);
            let raw_width = trim::rendered_clip_width(lane.width, width_frac);
            let raw_clip_left = raw_left as f32;
            let raw_clip_width = raw_width;
            let narrow = narrow_clips::is_narrow(raw_width, scale);
            let clip_left = snap_px(raw_left, scale);
            let clip_right = snap_px(raw_right, scale);
            let clip_width = if narrow {
                (clip_right - clip_left).max(1.0 / scale.max(1.0))
            } else {
                (clip_right - clip_left).max(0.0)
            };
            // Allow sub-pixel clips to be clicked at their rendered pixel column.
            let hit_width = clip_width.max(1.0 / scale.max(1.0));
            if px_x < clip_left || px_x > clip_left + hit_width {
                continue;
            }
            let rel_x = px_x - clip_left;
            let is_midi = matches!(entry.kind, IndexedClipKind::Midi(_));

            // 1. Audio clip gain handle (top 4px strip, inset 9px each side, width >= 28).
            if !is_midi
                && !narrow
                && clip_width >= 28.0
                && track_height > super::track_layout::HEADER_ONLY_HEIGHT
                && !self.fade_key_held
                && !self.time_selection_key_held
                && local_y < 4.0
                && rel_x >= 9.0
                && rel_x <= clip_width - 9.0
            {
                let cursor = if self.option_held {
                    CursorStyle::ResizeUpDown
                } else if self.drag_available() {
                    CursorStyle::OpenHand
                } else {
                    CursorStyle::Arrow
                };
                return Some(LaneHit {
                    clip_id: entry.id,
                    is_midi,
                    clip_start: entry.start,
                    clip_end: entry.end,
                    visible_start: vis_start,
                    visible_end: vis_end,
                    clip_left,
                    clip_width,
                    raw_clip_left,
                    raw_clip_width,
                    zone: LaneHitZone::GainHandle,
                    repeat_available: false,
                    cursor,
                });
            }

            // 2. Trim & loop handles when admitted and wide enough.
            let hw = if trim_admitted {
                trim::handle_width(clip_width)
            } else {
                0.0
            };
            if hw > 0.0 && local_y < edge_trim_h {
                if vis_start == entry.start && rel_x <= hw {
                    return Some(LaneHit {
                        clip_id: entry.id,
                        is_midi,
                        clip_start: entry.start,
                        clip_end: entry.end,
                        visible_start: vis_start,
                        visible_end: vis_end,
                        clip_left,
                        clip_width,
                        raw_clip_left,
                        raw_clip_width,
                        zone: LaneHitZone::TrimLeft,
                        repeat_available: false,
                        cursor: drag::rectangle_part(Some(TrimEdge::Left)).cursor(),
                    });
                }
                if vis_end == entry.end && rel_x >= clip_width - hw {
                    let repeat_available = match entry.kind {
                        IndexedClipKind::Audio(i) => track.clips().get(i).is_some_and(|c| {
                            loop_handles::audio_repeat_available(c, &state.project)
                        }),
                        IndexedClipKind::Midi(i) => track
                            .midi_clips()
                            .get(i)
                            .is_some_and(loop_handles::midi_repeat_available),
                    };
                    let cursor = if self.option_held && repeat_available {
                        CursorStyle::Loop
                    } else {
                        drag::rectangle_part(Some(TrimEdge::Right)).cursor()
                    };
                    return Some(LaneHit {
                        clip_id: entry.id,
                        is_midi,
                        clip_start: entry.start,
                        clip_end: entry.end,
                        visible_start: vis_start,
                        visible_end: vis_end,
                        clip_left,
                        clip_width,
                        raw_clip_left,
                        raw_clip_width,
                        zone: LaneHitZone::TrimRight,
                        repeat_available,
                        cursor,
                    });
                }
            }

            // 2b. Loop first-repeat-boundary handle in body.
            if hw > 0.0 {
                let controls = match entry.kind {
                    IndexedClipKind::Audio(i) => track.clips().get(i).and_then(|c| {
                        loop_handles::audio_controls(c, &state.project, lane, viewport)
                    }),
                    IndexedClipKind::Midi(i) => track.midi_clips().get(i).and_then(|c| {
                        loop_handles::midi_controls(c, &state.project, lane, viewport)
                    }),
                };
                if let Some(controls) = controls {
                    for handle in controls.handles.into_iter().flatten() {
                        if rel_x >= handle.left
                            && rel_x < handle.left + loop_handles::WIDTH
                            && local_y >= handle.top
                            && local_y < handle.top + handle.height
                        {
                            let cursor = match handle.kind {
                                LoopHandleKind::FirstBoundary => CursorStyle::ResizeLeftRight,
                                LoopHandleKind::Outer => CursorStyle::Loop,
                            };
                            return Some(LaneHit {
                                clip_id: entry.id,
                                is_midi,
                                clip_start: entry.start,
                                clip_end: entry.end,
                                visible_start: vis_start,
                                visible_end: vis_end,
                                clip_left,
                                clip_width,
                                raw_clip_left,
                                raw_clip_width,
                                zone: LaneHitZone::LoopHandle(handle.kind),
                                repeat_available: false,
                                cursor,
                            });
                        }
                    }
                }
            }

            // 3. Clip name header strip vs move surface vs body.
            let grab_cursor = if self.drag_available() {
                CursorStyle::OpenHand
            } else {
                CursorStyle::Arrow
            };
            let (zone, cursor) = if local_y < super::clip_name::CLIP_NAME_HEIGHT {
                (LaneHitZone::Header, grab_cursor)
            } else if local_y < header_hit_h {
                (LaneHitZone::MoveSurface, grab_cursor)
            } else {
                (LaneHitZone::Body, CursorStyle::Arrow)
            };
            return Some(LaneHit {
                clip_id: entry.id,
                is_midi,
                clip_start: entry.start,
                clip_end: entry.end,
                visible_start: vis_start,
                visible_end: vis_end,
                clip_left,
                clip_width,
                raw_clip_left,
                raw_clip_width,
                zone,
                repeat_available: false,
                cursor,
            });
        }
        None
    }

    /// Wrap a track lane's `TrackLaneCanvas` + optional live overlays with
    /// lane-level analytic pointer and drag handlers (3 lightweight wrappers
    /// per visible track instead of thousands of per-clip widgets).
    pub(super) fn wrap_track_lane_canvas(
        &self,
        track_id: TrackId,
        viewport: Viewport,
        scale_factor: f32,
        canvas: TrackLaneCanvas,
        overlays: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let generation = self.project_generation;
        let lane_cursor = self
            .hovered_arrangement_clip
            .filter(|h| h.generation == generation && h.track == track_id)
            .map_or(CursorStyle::Arrow, |h| h.resolved_cursor(self.option_held));

        let inner = div()
            .id(("lane-canvas-surface", track_id.value()))
            .absolute()
            .inset_0()
            .cursor(lane_cursor)
            .child(canvas)
            .children(overlays)
            .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
                let hit = view.hit_test_lane_clip(track_id, event.position, viewport, scale_factor);
                let next_hover = hit.map(|h| HoveredArrangementClip {
                    generation,
                    track: track_id,
                    clip: h.clip_id,
                    zone: h.zone,
                    repeat_available: h.repeat_available,
                    cursor: h.cursor,
                });
                let mut changed = false;
                if view.hovered_arrangement_clip != next_hover {
                    view.hovered_arrangement_clip = next_hover;
                    changed = true;
                }
                if let Some(h) = hit.filter(|h| !h.is_midi) {
                    if view.update_fade_hover(
                        track_id,
                        h.clip_id,
                        generation,
                        Some(f32::from(event.position.y)),
                    ) {
                        changed = true;
                    }
                } else if let Some((fade_gen, fade_clip)) = view.hovered_fade_clip
                    && fade_gen == generation
                    && view.state.as_ref().is_some_and(|s| {
                        s.project
                            .track(track_id)
                            .is_some_and(|t| t.clips().iter().any(|c| c.id() == fade_clip))
                    })
                    && view.update_fade_hover(track_id, fade_clip, generation, None)
                {
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                if !*hovered {
                    let mut changed = false;
                    if view
                        .hovered_arrangement_clip
                        .is_some_and(|h| h.track == track_id)
                    {
                        view.hovered_arrangement_clip = None;
                        changed = true;
                    }
                    if let Some((fade_gen, fade_clip)) = view.hovered_fade_clip
                        && fade_gen == generation
                        && view.state.as_ref().is_some_and(|s| {
                            s.project
                                .track(track_id)
                                .is_some_and(|t| t.clips().iter().any(|c| c.id() == fade_clip))
                        })
                        && view.update_fade_hover(track_id, fade_clip, generation, None)
                    {
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    if let Some(hit) =
                        view.hit_test_lane_clip(track_id, event.position, viewport, scale_factor)
                    {
                        cx.stop_propagation();
                        view.open_clip_context_menu(
                            event.position,
                            track_id,
                            hit.clip_id,
                            window,
                            cx,
                        );
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    let Some(hit) =
                        view.hit_test_lane_clip(track_id, event.position, viewport, scale_factor)
                    else {
                        return;
                    };
                    match hit.zone {
                        LaneHitZone::GainHandle => {
                            if event.modifiers.alt
                                && view.begin_clip_gain_mouse_down(
                                    track_id,
                                    hit.clip_id,
                                    viewport,
                                    event,
                                    window,
                                    cx,
                                )
                            {
                                cx.stop_propagation();
                            } else {
                                view.press_clip_move_from_lane(
                                    track_id,
                                    hit,
                                    viewport,
                                    event.position,
                                );
                            }
                        }
                        LaneHitZone::TrimLeft
                        | LaneHitZone::TrimRight
                        | LaneHitZone::LoopHandle(_) => {
                            let (edge, loop_kind) = match hit.zone {
                                LaneHitZone::TrimLeft => (TrimEdge::Left, None),
                                LaneHitZone::TrimRight => (TrimEdge::Right, None),
                                LaneHitZone::LoopHandle(kind) => (TrimEdge::Right, Some(kind)),
                                _ => return,
                            };
                            let target = trim::LaneTrimTarget {
                                track: track_id,
                                clip_id: hit.clip_id,
                                edge,
                                loop_kind,
                            };
                            if hit.is_midi {
                                view.press_midi_trim_from_lane(
                                    target,
                                    viewport,
                                    f32::from(event.position.x),
                                    window,
                                );
                            } else {
                                view.press_audio_trim_from_lane(
                                    target,
                                    viewport,
                                    f32::from(event.position.x),
                                    window,
                                );
                            }
                        }
                        LaneHitZone::Header | LaneHitZone::MoveSurface => {
                            view.press_clip_move_from_lane(track_id, hit, viewport, event.position);
                        }
                        LaneHitZone::Body => {}
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, event: &gpui::MouseUpEvent, _, _| {
                    if let Some(hit) =
                        view.hit_test_lane_clip(track_id, event.position, viewport, scale_factor)
                    {
                        let edge = match hit.zone {
                            LaneHitZone::TrimLeft => Some(TrimEdge::Left),
                            LaneHitZone::TrimRight | LaneHitZone::LoopHandle(_) => {
                                Some(TrimEdge::Right)
                            }
                            _ => None,
                        };
                        view.clear_pointer_down(generation, hit.clip_id, edge);
                    }
                }),
            )
            .on_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    if cx.has_active_drag() || view.active_clip_drag.is_some() {
                        return;
                    }
                    let Some(pos) = event.mouse_position() else {
                        return;
                    };
                    let Some(hit) = view.hit_test_lane_clip(track_id, pos, viewport, scale_factor)
                    else {
                        return;
                    };
                    match hit.zone {
                        LaneHitZone::Header
                        | LaneHitZone::MoveSurface
                        | LaneHitZone::GainHandle => {
                            if matches!(hit.zone, LaneHitZone::GainHandle) && event.modifiers().alt
                            {
                                cx.stop_propagation();
                                return;
                            }
                            view.clear_pointer_down(generation, hit.clip_id, None);
                            cx.stop_propagation();
                            if view.clip_name_is_editing(hit.clip_id) {
                                return;
                            }
                            if view.clip_name_edit.is_some() {
                                view.finish_clip_name(true, window, cx);
                            }
                            view.select_clip_header(
                                track_id,
                                hit.clip_id,
                                event.modifiers(),
                                window,
                                cx,
                            );
                            view.focus.focus(window);
                            cx.notify();
                        }
                        LaneHitZone::TrimLeft
                        | LaneHitZone::TrimRight
                        | LaneHitZone::LoopHandle(_) => {
                            cx.stop_propagation();
                        }
                        LaneHitZone::Body => {}
                    }
                }),
            );

        if !self.drag_available() {
            return div().size_full().relative().child(inner);
        }

        let audio_trim_entity = cx.entity();
        let midi_trim_entity = cx.entity();
        let move_entity = cx.entity();

        let with_audio_trim = div()
            .id(("lane-audio-trim-drag", track_id.value()))
            .absolute()
            .inset_0()
            .child(inner)
            .on_drag_from_press(move |press, _, window, cx| {
                audio_trim_entity.update(cx, |view, cx| {
                    let hit =
                        view.hit_test_lane_clip(track_id, press.position, viewport, scale_factor)?;
                    if hit.is_midi {
                        return None;
                    }
                    let (edge, loop_kind) = match hit.zone {
                        LaneHitZone::TrimLeft => (TrimEdge::Left, None),
                        LaneHitZone::TrimRight => (TrimEdge::Right, None),
                        LaneHitZone::LoopHandle(kind) => (TrimEdge::Right, Some(kind)),
                        _ => return None,
                    };
                    view.start_audio_trim_drag_from_lane(
                        trim::LaneTrimTarget {
                            track: track_id,
                            clip_id: hit.clip_id,
                            edge,
                            loop_kind,
                        },
                        viewport,
                        window,
                        cx,
                    )
                })
            });

        let with_midi_trim = div()
            .id(("lane-midi-trim-drag", track_id.value()))
            .absolute()
            .inset_0()
            .child(with_audio_trim)
            .on_drag_from_press(move |press, _, window, cx| {
                midi_trim_entity.update(cx, |view, cx| {
                    let hit =
                        view.hit_test_lane_clip(track_id, press.position, viewport, scale_factor)?;
                    if !hit.is_midi {
                        return None;
                    }
                    let (edge, loop_kind) = match hit.zone {
                        LaneHitZone::TrimLeft => (TrimEdge::Left, None),
                        LaneHitZone::TrimRight => (TrimEdge::Right, None),
                        LaneHitZone::LoopHandle(kind) => (TrimEdge::Right, Some(kind)),
                        _ => return None,
                    };
                    view.start_midi_trim_drag_from_lane(
                        trim::LaneTrimTarget {
                            track: track_id,
                            clip_id: hit.clip_id,
                            edge,
                            loop_kind,
                        },
                        viewport,
                        window,
                        cx,
                    )
                })
            });

        let with_clip_move = div()
            .id(("lane-clip-move-drag", track_id.value()))
            .absolute()
            .inset_0()
            .child(with_midi_trim)
            .on_drop(cx.listener(move |view, drag: &ClipDrag, window, cx| {
                view.drop_clip(drag, track_id, window, cx);
            }))
            .on_drag_from_press(move |press, _, window, cx| {
                move_entity.update(cx, |view, cx| {
                    let hit =
                        view.hit_test_lane_clip(track_id, press.position, viewport, scale_factor)?;
                    if !matches!(
                        hit.zone,
                        LaneHitZone::Header | LaneHitZone::MoveSurface | LaneHitZone::GainHandle
                    ) {
                        return None;
                    }
                    view.start_clip_move_drag_from_lane(track_id, hit, viewport, window, cx)
                })
            });

        div().size_full().relative().child(with_clip_move)
    }

    fn press_clip_move_from_lane(
        &mut self,
        track_id: TrackId,
        hit: LaneHit,
        viewport: Viewport,
        pos: Point<Pixels>,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(position) = self.clip_position_for(state, track_id, hit.clip_id, hit.is_midi)
        else {
            return;
        };
        let generation = self.project_generation;
        let revision = state.revision;
        let payload_check = ClipDrag {
            project_generation: generation,
            revision,
            source_track: track_id,
            clip: hit.clip_id,
            group: Default::default(),
            position,
            grab_offset: Default::default(),
            snap_edge: Default::default(),
            source_span: Default::default(),
            capture: Default::default(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Default::default(),
            copy_modifier: Default::default(),
            preview: Default::default(),
            destination: Default::default(),
            insertion_line_y: Default::default(),
            ghost_label: String::new(),
            ghost_waveform: None,
            lane_hovered: Default::default(),
            last_pointer: Default::default(),
        };
        if self.drag_start_is_current(&payload_check) {
            self.remember_pointer_down(generation, hit.clip_id, None, f32::from(pos.x));
            let edge = drag::clip_snap_edge(
                f32::from(pos.x),
                hit.clip_left,
                hit.clip_width,
                hit.visible_start,
                hit.visible_end,
                hit.clip_start,
                hit.clip_end,
            );
            if let Some(press) = self.pointer_down.as_mut() {
                press.snap_edge = edge;
            }
        } else {
            self.clear_pointer_down(generation, hit.clip_id, None);
        }
    }

    fn clip_position_for(
        &self,
        state: &ViewState,
        track_id: TrackId,
        clip_id: ClipId,
        is_midi: bool,
    ) -> Option<ClipPosition> {
        let track = state.project.track(track_id)?;
        if is_midi {
            let clip = track.midi_clips().iter().find(|c| c.id() == clip_id)?;
            Some(ClipPosition::Midi {
                start: clip.start(),
                length: clip.length(),
            })
        } else {
            let clip = track.clips().iter().find(|c| c.id() == clip_id)?;
            Some(ClipPosition::Audio {
                start: clip.start(),
                duration: clip.duration(),
            })
        }
    }

    fn start_clip_move_drag_from_lane(
        &mut self,
        track_id: TrackId,
        hit: LaneHit,
        viewport: Viewport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(ClipDrag, Entity<super::tracks::ClipDragGhost>)> {
        let state = self.state.as_ref()?.clone();
        let position = self.clip_position_for(&state, track_id, hit.clip_id, hit.is_midi)?;
        let kind = if hit.is_midi { "midi" } else { "audio" };
        let ghost_waveform = if !hit.is_midi {
            state
                .project
                .track(track_id)
                .and_then(|t| t.clips().iter().find(|c| c.id() == hit.clip_id))
                .filter(|c| c.audio_edit().is_none())
                .and_then(|c| state.waveforms.get(&c.asset()).cloned())
        } else {
            None
        };
        let source_start_beat = match position {
            ClipPosition::Midi { start, .. } => Some(start),
            ClipPosition::Audio { .. } => None,
        };
        let source_span = Rc::new(std::cell::Cell::new(Some((hit.clip_left, hit.clip_width))));
        let payload = ClipDrag {
            project_generation: self.project_generation,
            revision: state.revision,
            source_track: track_id,
            clip: hit.clip_id,
            group: Default::default(),
            position,
            grab_offset: Default::default(),
            snap_edge: Default::default(),
            source_span,
            capture: Default::default(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Default::default(),
            copy_modifier: Default::default(),
            preview: Default::default(),
            destination: Default::default(),
            insertion_line_y: Default::default(),
            ghost_label: format!("{kind} clip {}", hit.clip_id.value()),
            ghost_waveform,
            lane_hovered: Default::default(),
            last_pointer: Default::default(),
        };
        let capture = if self.drag_start_is_current(&payload) {
            self.take_pointer_down(payload.project_generation, payload.clip, None)
        } else {
            self.clear_pointer_down(payload.project_generation, payload.clip, None);
            None
        };
        payload.capture.set(capture);
        payload
            .snap_edge
            .set(capture.and_then(|press| press.snap_edge));
        let original_down_x = capture.map(drag::PointerDown::origin_x);
        let frame_offset = original_down_x.and_then(|x| {
            drag::grab_offset_from_pointer(
                x,
                hit.raw_clip_left,
                hit.raw_clip_width,
                hit.visible_start,
                hit.visible_end,
                hit.clip_start,
            )
        });
        let offset = match (payload.position, frame_offset, source_start_beat) {
            (ClipPosition::Audio { .. }, Some(offset), _) => Some(offset),
            (ClipPosition::Midi { .. }, Some(offset), Some(start_beat)) => hit
                .clip_start
                .checked_add(offset)
                .and_then(|grabbed_frame| {
                    state.project.tempo_map().frame_to_beat(grabbed_frame).ok()
                })
                .map(|beat| beat.raw().saturating_sub(start_beat.raw())),
            _ => None,
        };
        payload.grab_offset.set(offset);
        payload.snap_modifier.set(window.modifiers().platform);
        payload.copy_modifier.set(window.modifiers().alt);
        cx.set_active_drag_cursor_style(drag::copy_cursor(payload.copy_modifier.get()), window);
        let pointer = window.mouse_position();
        let pointer = (f32::from(pointer.x), f32::from(pointer.y));
        payload.last_pointer.set(Some(pointer));
        self.begin_clip_drag(&payload);
        let (preview, lane_hovered) = self.clip_drag_drop_position(&payload, pointer.0, pointer.1);
        payload.preview.set(preview);
        payload.lane_hovered.set(lane_hovered);
        self.sync_held_clip_move_audition();
        cx.notify();
        Some((payload, cx.new(|_| super::tracks::ClipDragGhost)))
    }
}
