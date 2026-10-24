//! Shared painted/captured loop zones in outer clip pixels. Musical projection
//! comes from project queries; ordinary trim/warp/fade policies stay with owners.
use super::*;
use crate::theme;
use gpui::{rgb, rgba};

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
    if width >= 24.0 && lane.height - body_top >= 8.0 {
        if b == end {
            handles[0] = Some(LoopControl {
                kind: LoopHandleKind::Outer,
                left: width - 17.0,
                top: lane.height - 9.0,
                height: 8.0,
            });
        }
        if let Some(frame) = boundary.filter(|frame| *frame > a && *frame < b) {
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

pub(super) fn surface(
    surface: Stateful<Div>,
    edge: TrimEdge,
    width: f32,
    control: Option<LoopControl>,
) -> Stateful<Div> {
    let Some(control) = control else {
        return surface
            .absolute()
            .top(px(-1.0))
            .bottom(px(-1.0))
            .w(px(handle_width(width)))
            .when(edge == TrimEdge::Left, |d| d.left(px(-1.0)))
            .when(edge == TrimEdge::Right, |d| d.right(px(-1.0)))
            .cursor(bracket_cursor(edge));
    };
    // Absolute children start inside border_1; the shared hit zones use outer
    // clip coordinates, so remove that border inset exactly once here.
    surface
        .absolute()
        .left(px(control.left - 1.0))
        .top(px(control.top - 1.0))
        .w(px(WIDTH))
        .h(px(control.height))
        .cursor(CursorStyle::ResizeLeftRight)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(8.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
        .hover(|s| s.bg(rgba(0xffffff22)))
        .child(if control.kind == LoopHandleKind::Outer {
            div().child(theme::text("↻"))
        } else {
            div()
                .w(px(1.0))
                .h_full()
                .bg(rgb(theme::color(theme::PaletteRole::Text)))
        })
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
