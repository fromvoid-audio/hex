//! Shared painted/captured loop zones in outer clip pixels. Musical projection
//! comes from project queries; ordinary trim/warp/fade policies stay with owners.
use super::*;
use crate::theme;
use gpui::{CursorStyle, relative, rgb, rgba};

pub(in crate::editor) const BOTTOM_RADIUS: f32 = 3.0;
const WIDTH: f32 = 8.0;

#[derive(Clone, Copy)]
pub(super) struct LoopControl {
    pub(super) kind: LoopHandleKind,
    left: f32,
    top: f32,
    height: f32,
}
pub(in crate::editor) struct LoopControls {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    pub(super) handles: [Option<LoopControl>; 2],
}
impl LoopControls {
    pub(in crate::editor) fn hit(&self, x: f32, y: f32) -> bool {
        let x = x - self.left;
        let y = y - self.top;
        x >= 0.0
            && x < self.width
            && y >= 0.0
            && y < self.height
            && self
                .handles
                .iter()
                .flatten()
                .any(|h| x >= h.left && x < h.left + WIDTH && y >= h.top && y < h.top + h.height)
    }
}

fn geometry(
    start: u64,
    end: u64,
    boundary: Option<u64>,
    lane: LaneBounds,
    viewport: Viewport,
    minimum: f32,
) -> Option<LoopControls> {
    if viewport.length == 0
        || !lane.width.is_finite()
        || lane.width <= 0.0
        || !lane.height.is_finite()
        || lane.height <= 0.0
    {
        return None;
    }
    let a = start.max(viewport.start);
    let b = end.min(viewport.end()?);
    if a >= b {
        return None;
    }
    let left =
        lane.left + lane.width * ((a - viewport.start) as f64 / viewport.length as f64) as f32;
    let width = rendered_clip_width(lane.width, (b - a) as f64 / viewport.length as f64, minimum);
    let body_top = super::super::clip_name::CLIP_NAME_HEIGHT + 1.0;
    let mut handles = [None, None];
    if width >= 24.0 && lane.height >= 10.0 {
        if b == end {
            handles[0] = Some(LoopControl {
                kind: LoopHandleKind::Outer,
                left: width - 17.0,
                top: lane.height - 9.0,
                height: 8.0,
            });
        }
        if lane.height - body_top >= 13.0
            && let Some(frame) = boundary.filter(|frame| *frame > a && *frame < b)
        {
            handles[1] = Some(LoopControl {
                kind: LoopHandleKind::FirstBoundary,
                left: lane.width * ((frame - a) as f64 / viewport.length as f64) as f32
                    - WIDTH / 2.0,
                top: body_top,
                height: (lane.height - body_top - 9.0).max(4.0),
            });
        }
    }
    Some(LoopControls {
        left,
        top: lane.top,
        width,
        height: lane.height,
        handles,
    })
}

pub(in crate::editor) fn audio_repeats(clip: &Clip) -> bool {
    clip.audio_edit()
        .and_then(|edit| edit.placement)
        .map_or(clip.looping(), |p| {
            p.mode == hex_project::AudioViewMode::Repeat
        })
}
pub(in crate::editor) fn audio_controls(
    clip: &Clip,
    project: &Project,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<LoopControls> {
    let end = clip.start().checked_add(clip.duration())?;
    // A retained processing rebase can refuse these existing operations. Never
    // advertise an interactive handle which that canonical owner cannot edit.
    hex_project::preview_view_resize(
        clip,
        project.tempo_map(),
        hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: end },
    )
    .ok()?;
    let boundary = audio_repeats(clip)
        .then(|| hex_project::AudioViewProjection::new(clip, project.tempo_map()).ok())
        .flatten()
        .and_then(|p| u64::try_from(p.logical_first_boundary()).ok());
    geometry(clip.start(), end, boundary, lane, viewport, 3.0)
}
pub(in crate::editor) fn midi_controls(
    clip: &MidiClip,
    project: &Project,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<LoopControls> {
    let end = BeatTime::from_raw(clip.start().raw().checked_add(clip.length().raw())?);
    clip.preview_loop_resize(hex_project::MidiViewResizeOperation::RepeatOuterEnd { beat: end })
        .ok()?;
    let tempo = project.tempo_map();
    geometry(
        tempo.beat_to_frame(clip.start()).ok()?,
        tempo.beat_to_frame(end).ok()?,
        clip.first_repeat_boundary()
            .and_then(|b| tempo.beat_to_frame(b).ok()),
        lane,
        viewport,
        4.0,
    )
}

pub(super) fn base(edge: TrimEdge, width: f32, control: Option<LoopControl>) -> Div {
    let Some(control) = control else {
        return hex_widgets::rectangle_edge(
            super::super::drag::rectangle_part(Some(edge)),
            handle_width(width),
            -1.0,
        );
    };
    // Absolute children start inside border_1; the shared hit zones use outer
    // clip coordinates, so remove that border inset exactly once here.
    div()
        .absolute()
        .left(px(control.left - 1.0))
        .top(px(control.top - 1.0))
        .w(px(WIDTH))
        .h(px(control.height))
        .cursor(CursorStyle::ResizeLeftRight)
        .flex()
        .items_center()
        .justify_center()
}

pub(super) fn decorate(surface: Stateful<Div>, control: Option<LoopControl>) -> Stateful<Div> {
    let Some(control) = control else {
        return surface;
    };
    surface
        .hover(|s| s.bg(rgba(0xffffff22)))
        .tooltip(move |_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                description: if control.kind == LoopHandleKind::Outer {
                    "drag to repeat clip · cmd: invert snapping"
                } else {
                    "drag first repeat boundary · keep clip duration · cmd: invert snapping"
                }
                .into(),
            })
            .into()
        })
}

/// Round the fill and selection stroke together. Body layers stop above the
/// corner radius because GPUI's descendant content mask is rectangular.
pub(in crate::editor) fn surface<T: Styled + FluentBuilder>(surface: T, looping: bool) -> T {
    surface.when(looping, |s| {
        s.rounded_bl(px(BOTTOM_RADIUS))
            .rounded_br(px(BOTTOM_RADIUS))
    })
}
pub(in crate::editor) fn body(body: Div, looping: bool) -> Div {
    body.when(looping, |s| s.bottom(px(BOTTOM_RADIUS)))
}

// Bound display work independently of a potentially enormous repeat count.
// A lower-bound search jumps directly to the visible repetitions. A marker is
// decoration, not a second hit surface or a source of playback geometry.
fn markers(
    start: u64,
    end: u64,
    count: u64,
    first: Option<u64>,
    frame: impl Fn(u64) -> Option<u64>,
    outer_end_visible: bool,
) -> Div {
    let mut layer = div()
        .debug_selector(|| "clip-loop-markers".into())
        .absolute()
        .inset_0();
    if start >= end {
        return layer;
    }
    let mut lo = 0;
    let mut hi = count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match frame(mid) {
            Some(value) if value <= start => lo = mid + 1,
            _ => hi = mid,
        }
    }
    for index in lo..count.min(lo.saturating_add(32)) {
        let Some(value) = frame(index).filter(|value| *value < end) else {
            break;
        };
        let primary = Some(value) == first;
        layer = layer.child(
            div()
                .debug_selector(move || format!("clip-repeat-marker-{index}"))
                .absolute()
                .left(relative(
                    (value.saturating_sub(start) as f64 / (end - start) as f64) as f32,
                ))
                .top(px(super::super::clip_name::CLIP_NAME_HEIGHT + 1.0))
                .bottom(px(BOTTOM_RADIUS))
                .w(px(if primary { 1.0 } else { 0.5 }))
                .bg(rgb(theme::color(theme::PaletteRole::Text)))
                .opacity(if primary { 0.85 } else { 0.3 }),
        );
    }
    if outer_end_visible {
        layer = layer.child(
            div()
                .absolute()
                .right(px(9.0))
                .bottom(px(1.0))
                .w(px(WIDTH))
                .h(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(8.0))
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                .child(theme::text("↻")),
        );
    }
    layer
}

pub(in crate::editor) fn audio_markers(
    clip: &Clip,
    project: &Project,
    start: u64,
    end: u64,
) -> Div {
    if !audio_repeats(clip) {
        return div();
    }
    let Ok(projection) = hex_project::AudioViewProjection::new(clip, project.tempo_map()) else {
        return div();
    };
    markers(
        start,
        end,
        projection.occurrence_count().saturating_sub(1),
        u64::try_from(projection.logical_first_boundary()).ok(),
        |index| {
            projection
                .occurrence(index.checked_add(1)?)
                .ok()
                .map(|o| o.destination_start)
        },
        end == projection.outer_frame_bounds().1,
    )
}

pub(in crate::editor) fn midi_markers(
    clip: &MidiClip,
    preview: Option<MidiEdgePreview>,
    project: &Project,
    start: u64,
    end: u64,
) -> Div {
    if !preview.map_or(clip.looping(), |p| p.looping(clip)) {
        return div();
    }
    let playback = match preview {
        Some(p) => p.source_playback(clip),
        None => clip.source_playback(),
    };
    let Ok(playback) = playback else {
        return div();
    };
    let Some((a, b)) = playback.loop_range() else {
        return div();
    };
    let Some(period) = b.raw().checked_sub(a.raw()).filter(|n| *n > 0) else {
        return div();
    };
    let Some(first) = playback.first_repeat_boundary() else {
        return div();
    };
    let origin = preview.map_or(clip.start(), |p| p.start());
    let length = preview.map_or(clip.length(), |p| p.length());
    let count = length.raw().saturating_sub(first.raw()) / period + 1;
    let frame = |index: u64| {
        let beat = origin
            .raw()
            .checked_add(first.raw())?
            .checked_add(index.checked_mul(period)?)?;
        project
            .tempo_map()
            .beat_to_frame(BeatTime::from_raw(beat))
            .ok()
    };
    let outer = origin.raw().checked_add(length.raw()).and_then(|b| {
        project
            .tempo_map()
            .beat_to_frame(BeatTime::from_raw(b))
            .ok()
    });
    markers(start, end, count, frame(0), frame, outer == Some(end))
}
