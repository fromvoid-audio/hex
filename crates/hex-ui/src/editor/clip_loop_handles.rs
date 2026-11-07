//! Shared painted/captured loop zones in outer clip pixels. Musical projection
//! comes from project queries; ordinary trim/warp/fade policies stay with owners.
//! First-boundary handles use horizontal arrows. Outer repeat-extend has no
//! corner of its own: option-dragging the right header edge repeats the clip
//! (Pulse #3796) and shows the circle cursor (`CursorStyle::Loop`, drawn from
//! `assets/cursors/loop.svg`), on hover while option is held and for the drag.
//! Edge trimming only starts in the clip's header rail
//! ([`super::edge_trim_height`]); the lower edge belongs to the body and the
//! fade handles, whose fade-out handle sits in the true bottom-right corner.
use super::*;
use crate::theme;
use gpui::{CursorStyle, relative, rgb};

pub(in crate::editor) const BOTTOM_RADIUS: f32 = 8.0;
/// Width of the first-boundary hit box.
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
) -> Option<LoopControls> {
    if viewport.length == 0
        || !lane.left.is_finite()
        || !lane.top.is_finite()
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
    let width = rendered_clip_width(lane.width, (b - a) as f64 / viewport.length as f64);
    if !left.is_finite() || !width.is_finite() {
        return None;
    }
    let body_top = super::super::clip_name::CLIP_NAME_HEIGHT + 1.0;
    let mut handles = [None, None];
    // The first repeat boundary lives in the body below the header rail, away
    // from the header-edge trim strips and the corner fade handles.
    if width >= 24.0
        && lane.height >= 10.0
        && lane.height - body_top >= 4.0
        && let Some(frame) = boundary.filter(|frame| *frame > a && *frame < b)
    {
        handles[1] = Some(LoopControl {
            kind: LoopHandleKind::FirstBoundary,
            left: lane.width * ((frame - a) as f64 / viewport.length as f64) as f32 - WIDTH / 2.0,
            top: body_top,
            height: (lane.height - body_top - 9.0).max(4.0),
        });
    }
    Some(LoopControls {
        left,
        top: lane.top,
        width,
        height: lane.height,
        handles,
    })
}

/// Whether option-dragging the right header edge can repeat-extend this clip.
pub(in crate::editor) fn audio_repeat_available(clip: &Clip, project: &Project) -> bool {
    clip.start()
        .checked_add(clip.duration())
        .is_some_and(|end| {
            hex_project::preview_view_resize(
                clip,
                project.tempo_map(),
                hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: end },
            )
            .is_ok()
        })
}
/// See [`audio_repeat_available`].
pub(in crate::editor) fn midi_repeat_available(clip: &MidiClip) -> bool {
    clip.start()
        .raw()
        .checked_add(clip.length().raw())
        .is_some_and(|end| {
            clip.preview_loop_resize(hex_project::MidiViewResizeOperation::RepeatOuterEnd {
                beat: BeatTime::from_raw(end),
            })
            .is_ok()
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
    // CycleEnd authors a forward base boundary. A directed finite seam has no
    // general one-to-one inverse, so retain the outer handle without exposing
    // a first-seam drag that cannot follow its pointer. Authored settings remain.
    let boundary = (audio_repeats(clip)
        && clip.audio_edit().is_none_or(|e| e.content_view.is_none()))
    .then(|| hex_project::AudioViewProjection::new(clip, project.tempo_map()).ok())
    .flatten()
    .and_then(|p| u64::try_from(p.logical_first_boundary()).ok());
    geometry(clip.start(), end, boundary, lane, viewport)
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
        clip.content_view()
            .is_none()
            .then(|| clip.first_repeat_boundary())
            .flatten()
            .and_then(|b| tempo.beat_to_frame(b).ok()),
        lane,
        viewport,
    )
}

pub(super) fn base(
    edge: TrimEdge,
    width: f32,
    lane_height: f32,
    control: Option<LoopControl>,
    repeat_hover: bool,
    scale: f32,
) -> Div {
    let border = hex_widgets::rectangle_border_width(width);
    let left_border = if border > 0.0 {
        1.0 / scale.max(1.0)
    } else {
        0.0
    };
    let Some(control) = control else {
        let part = super::super::drag::rectangle_part(Some(edge));
        // Edge trimming is limited to the header rail, sharing its height with
        // pointer arbitration in `clip_pointer_handles`.
        return div()
            .absolute()
            .top(px(-border))
            .h(px(super::edge_trim_height(lane_height)))
            .w(px(handle_width(width)))
            .cursor(if repeat_hover {
                CursorStyle::Loop
            } else {
                part.cursor()
            })
            .when(edge == TrimEdge::Left, |d| d.left(px(-left_border)))
            .when(edge == TrimEdge::Right, |d| d.right_0());
    };
    div()
        .absolute()
        .left(px(control.left - left_border))
        .top(px(control.top - border))
        .w(px(WIDTH))
        .h(px(control.height))
        .cursor(match control.kind {
            LoopHandleKind::FirstBoundary => CursorStyle::ResizeLeftRight,
            LoopHandleKind::Outer => CursorStyle::Loop,
        })
}

pub(super) fn decorate(
    surface: Stateful<Div>,
    control: Option<LoopControl>,
    edge: TrimEdge,
    repeat_available: bool,
) -> Stateful<Div> {
    if control.is_none() {
        return surface.tooltip(move |_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control(match edge {
                    TrimEdge::Left => "clip start",
                    TrimEdge::Right => "clip end",
                }),
                description: if repeat_available {
                    "drag: resize · shift: stretch · option: repeat · cmd: invert snapping"
                } else {
                    "drag: resize · shift: stretch · cmd: invert snapping"
                }
                .into(),
            })
            .into()
        });
    }
    // Only the first-boundary control remains; outer repeat is option + edge.
    surface.tooltip(move |_, cx| {
        cx.new(|_| super::super::transport::MiniTooltip {
            kind: crate::tooltips::Kind::control("first repeat boundary"),
            description: "drag first repeat boundary · keep clip duration · cmd: invert snapping"
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
    let first_visible = lo;
    hi = count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match frame(mid) {
            Some(value) if value < end => lo = mid + 1,
            _ => hi = mid,
        }
    }
    let last_visible = lo;
    let stride = last_visible
        .saturating_sub(first_visible)
        .div_ceil(32)
        .max(1);
    for offset in 0_u64..32 {
        let index = first_visible.saturating_add(offset.saturating_mul(stride));
        if index >= last_visible {
            break;
        }
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
    let Ok(plan) = projection.repeat_boundaries() else {
        return div();
    };
    let frame = |index| projection.frame_at_elapsed(plan?.boundary(index)?).ok();
    markers(
        start,
        end,
        plan.map_or(0, |plan| plan.count()),
        frame(0),
        frame,
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
    let Ok(plan) = playback.repeat_boundaries() else {
        return div();
    };
    let origin = preview.map_or(clip.start(), |p| p.start());
    let frame = |index: u64| {
        let beat = origin.raw().checked_add(plan?.boundary(index)?)?;
        project
            .tempo_map()
            .beat_to_frame(BeatTime::from_raw(beat))
            .ok()
    };
    markers(
        start,
        end,
        plan.map_or(0, |plan| plan.count()),
        frame(0),
        frame,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loop_fill_and_border_keep_square_headers_and_only_round_lower_corners() {
        let mut ordinary = surface(div(), false);
        let mut looping = surface(div(), true);
        let plain = ordinary.style();
        assert!(
            plain.corner_radii.bottom_left.is_none() && plain.corner_radii.bottom_right.is_none()
        );
        let rounded = looping.style();
        assert!(
            rounded.corner_radii.top_left.is_none() && rounded.corner_radii.top_right.is_none()
        );
        assert_eq!(
            rounded.corner_radii.bottom_left,
            Some(px(BOTTOM_RADIUS).into())
        );
        assert_eq!(
            rounded.corner_radii.bottom_right,
            rounded.corner_radii.bottom_left
        );
    }
}
