//! Hover and momentary Arrangement fade controls. Pointer previews never compile audio or
//! mutate history; release submits one validated canonical envelope command.
use super::{EditorView, ViewState, drag::LaneBounds, viewport::Viewport};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, PathBuilder, Pixels, Point, Window, canvas, div, point, prelude::*,
    px, relative, rgb, rgba,
};
use hex_project::{Clip, ClipFade, ClipId, Command, TrackId, TrimEdge};
use hex_session::service::Request;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handle {
    InTop,
    InBottom,
    InCurve,
    OutTop,
    OutBottom,
    OutCurve,
}
impl Handle {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::InTop => "fade-in-top",
            Self::InBottom => "fade-in-bottom",
            Self::InCurve => "fade-in-curve",
            Self::OutTop => "fade-out-top",
            Self::OutBottom => "fade-out-bottom",
            Self::OutCurve => "fade-out-curve",
        }
    }
}
struct FadeControls {
    lane: LaneBounds,
    local_fade: ClipFade,
    reanchors: bool,
    positions: Vec<(Handle, Point<f32>)>,
}

// The hit target stays square; the painted curve control is a diamond.
pub(super) fn handle_shape(curve: bool, active: bool) -> impl gpui::IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let left = f32::from(bounds.left()) + 0.75;
            let top = f32::from(bounds.top()) + 0.75;
            let right = f32::from(bounds.right()) - 0.75;
            let bottom = f32::from(bounds.bottom()) - 0.75;
            let vertices = if curve {
                [
                    ((left + right) / 2.0, top),
                    (right, (top + bottom) / 2.0),
                    ((left + right) / 2.0, bottom),
                    (left, (top + bottom) / 2.0),
                ]
            } else {
                [(left, top), (right, top), (right, bottom), (left, bottom)]
            };
            let color: gpui::Hsla = if active {
                rgb(theme::color(theme::PaletteRole::Secondary)).into()
            } else {
                rgba(0x000000cc).into()
            };
            for fill in [true, false] {
                let mut path = if fill {
                    PathBuilder::fill()
                } else {
                    PathBuilder::stroke(px(1.0))
                };
                for (index, (x, y)) in vertices.into_iter().enumerate() {
                    if index == 0 {
                        path.move_to(point(px(x), px(y)));
                    } else {
                        path.line_to(point(px(x), px(y)));
                    }
                }
                path.close();
                if let Ok(path) = path.build() {
                    let paint = if fill {
                        if active {
                            gpui::Hsla { a: 0.45, ..color }
                        } else {
                            rgba(0x00000020).into()
                        }
                    } else {
                        color
                    };
                    window.paint_path(path, paint);
                }
            }
        },
    )
    .absolute()
    .inset_0()
}

#[derive(Clone)]
pub(super) struct FadeDrag {
    generation: u64,
    revision: u64,
    track: TrackId,
    clip: Rc<Clip>,
    viewport: Viewport,
    lane: LaneBounds,
    handle: Handle,
    pointer: Point<Pixels>,
    original: ClipFade,
    pub(super) preview: ClipFade,
    moved: bool,
    preview_clip: Option<Clip>,
    trim_frame: Option<u64>,
    snap_grid: super::musical_time::SnapGrid,
}

// Curve motion retains subframe precision; endpoint lengths still round to frames.
pub(super) fn edited_fade(
    fade: ClipFade,
    handle: Handle,
    frames: f64,
    bend: i16,
) -> Option<ClipFade> {
    if !frames.is_finite() {
        return None;
    }
    let whole_frames = frames.round() as i128;
    let span = fade.span();
    let start = fade.in_start();
    let end_offset = fade.out_end_offset();
    let mut fade_in = fade.fade_in();
    let mut fade_out = fade.fade_out();
    let mut in_curve = fade.in_curve();
    let mut out_curve = fade.out_curve();
    let mut in_position = fade.in_position();
    let mut out_position = fade.out_position();
    // The top handles push each other: the dragged ramp reaches the opposite
    // silent endpoint and the other ramp gives way. Recomputing from the drag's
    // original fade springs the pushed ramp back to, and no further than, its
    // pre-drag length when the same gesture reverses.
    let body = span.saturating_sub(start).saturating_sub(end_offset);
    match handle {
        Handle::InTop => {
            fade_in = i128::from(fade_in)
                .saturating_add(whole_frames)
                .clamp(0, i128::from(body)) as u64;
            fade_out = fade_out.min(body - fade_in);
        }
        Handle::OutTop => {
            fade_out = i128::from(fade_out)
                .saturating_sub(whole_frames)
                .clamp(0, i128::from(body)) as u64;
            fade_in = fade_in.min(body - fade_out);
        }
        Handle::InBottom | Handle::OutBottom => return None,
        Handle::InCurve | Handle::OutCurve => {
            let out = handle == Handle::OutCurve;
            let curve = if out { &mut out_curve } else { &mut in_curve };
            // The diamond stays between channel centers (central half of body).
            // Opening a legacy bend never changes it. An explicit horizontal
            // edit fits hidden extremes to the visible ordinate, so all four
            // corners remain reachable without moving the diamond vertically.
            if bend != 0 {
                let control = (*curve).clamp(-500, 500);
                let next = control.saturating_add(bend).clamp(-500, 500);
                // Legacy bends can lie beyond the visual band. Start movement
                // from the visible edge, with no dead zone or outward-only edit.
                if next != control || *curve == control {
                    *curve = next;
                }
            }
            if frames != 0.0 {
                *curve = (*curve).clamp(-500, 500);
            }
            let length = if out { fade.fade_out() } else { fade.fade_in() };
            let changed = if out {
                out_curve != fade.out_curve()
            } else {
                in_curve != fade.in_curve()
            };
            if length > 1 && (frames != 0.0 || changed) {
                let original = fade.position_at_gain(out, curve_handle_gain(fade, out))?;
                let target = (original + frames / (length - 1) as f64).clamp(0.001, 0.999);
                // Explicit edits use the bounded ordinate, whose mapped half
                // progress is the canonical horizontal position.
                let position = (target * 1000.0).round().clamp(1.0, 999.0) as u16;
                if out {
                    out_position = position;
                } else {
                    in_position = position;
                }
            }
        }
    }
    ClipFade::new(span, 0, fade_in, fade_out)
        .ok()?
        .with_shape(start, end_offset, in_curve, out_curve)
        .ok()?
        .with_curve_positions(in_position, out_position)
        .ok()
}

// Each handle belongs to its own ramp, including legacy bends. Overlapping
// ramps multiply in playback/the shaded envelope, but must not pull the other
// ramp's control outside its editing box.
fn curve_handle_gain(fade: ClipFade, out: bool) -> f64 {
    fade.curve_midpoint_gain(out).clamp(0.25, 0.75)
}

pub(super) fn knots(fade: ClipFade) -> [(Handle, (u64, f64), f32); 6] {
    let end = fade.span() - fade.out_end_offset() - 1;
    let in_top = fade.in_start() + fade.fade_in().saturating_sub(1);
    let out_top = (end + 1).saturating_sub(fade.fade_out()).min(end);
    let interior = |start: u64, end: u64, out| {
        let length = end - start;
        let offset = length as f64
            * fade
                .position_at_gain(out, curve_handle_gain(fade, out))
                .unwrap_or(0.5);
        let whole = (offset.floor() as u64).min(length);
        (start + whole, (offset - whole as f64).clamp(0.0, 1.0))
    };
    [
        (Handle::InTop, (in_top, 0.0), 1.0),
        (Handle::InBottom, (fade.in_start(), 0.0), 0.0),
        (
            Handle::InCurve,
            interior(fade.in_start(), in_top, false),
            curve_handle_gain(fade, false) as f32,
        ),
        (Handle::OutTop, (out_top, 0.0), 1.0),
        (Handle::OutBottom, (end, 0.0), 0.0),
        (
            Handle::OutCurve,
            interior(out_top, end, true),
            curve_handle_gain(fade, true) as f32,
        ),
    ]
}

impl EditorView {
    // Window capture keeps previews moving over occluding controls and outside
    // the clip body, matching the existing loop-rail pointer contract.
    pub(super) fn fade_pointer_events(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let entity = cx.entity();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let release_entity = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, window, cx| {
                    if phase.capture() && event.button == MouseButton::Left {
                        release_entity.update(cx, |view, cx| {
                            if view.finish_audio_event_fade_pointer(
                                event.position,
                                event.modifiers.platform,
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                            }
                            if view.active_fade_drag.is_some() {
                                view.update_fade_gesture(
                                    event.position,
                                    event.modifiers.platform,
                                    cx,
                                );
                                view.finish_fade_gesture(window, cx);
                                cx.stop_propagation();
                            }
                        });
                    }
                });
                let entity = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                    if phase.capture() {
                        entity.update(cx, |view, cx| {
                            if view.update_audio_event_fade_pointer(
                                event.position,
                                event.modifiers.platform,
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                            }
                            view.update_fade_gesture(event.position, event.modifiers.platform, cx)
                        });
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
    pub(super) fn fade_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.automation_view.enabled
            && self.mode == super::EditorMode::Arrangement
            && self.focus.is_focused(window)
        {
            return false;
        }
        if self.audio_event_focus.is_focused(window) {
            return self.event_fade_key_down(event, window, cx);
        }
        if event.keystroke.key == "escape" && self.active_fade_drag.take().is_some() {
            cx.stop_propagation();
            cx.notify();
            return true;
        }
        if event.keystroke.key != "f"
            || event.keystroke.modifiers.modified()
            || !self.focus.is_focused(window)
            || self.mode != super::EditorMode::Arrangement
            || self.computer_keyboard.enabled
            || self.theme_settings.open
            || self.context_menu.is_some()
            || self.gain_numeric_editing(cx)
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return false;
        }
        if !event.is_held && !self.fade_key_held {
            self.fade_key_held = true;
            cx.notify();
        }
        cx.stop_propagation();
        true
    }
    pub(super) fn cancel_fade_gesture(&mut self, cx: &mut Context<Self>) {
        let removed = self.active_fade_drag.take().is_some();
        let changed = self.fade_key_held || removed;
        self.fade_key_held = false;
        if changed {
            cx.notify();
        }
    }
    fn fade_drag_current(&self, drag: &FadeDrag) -> bool {
        self.mode == super::EditorMode::Arrangement
            && self.drag_available()
            && self.clip_paint_gesture.is_none()
            && self.active_clip_drag.is_none()
            && self.active_trim_drag.is_none()
            && self.active_midi_trim_drag.is_none()
            && !self.save_dialog_open
            && !self.export_dialog_open
            && !self.theme_settings.open
            && self.context_menu.is_none()
            && self.state.as_ref().is_some_and(|state| {
                self.project_generation == drag.generation
                    && state.revision == drag.revision
                    && self.visible_viewport(state.timeline_frames) == drag.viewport
                    && self.drag_lanes.get(&drag.track).is_some_and(|lane| {
                        lane.left == drag.lane.left
                            && lane.width == drag.lane.width
                            && lane.top == drag.lane.top
                            && lane.height == drag.lane.height
                    })
                    && state.project.clip(drag.clip.id()) == Some(drag.clip.as_ref())
            })
    }
    pub(super) fn reconcile_fade_gesture(&mut self) {
        if self.hovered_fade_clip.is_some_and(|(generation, clip)| {
            generation != self.project_generation
                || self.mode != super::EditorMode::Arrangement
                || self
                    .state
                    .as_ref()
                    .is_none_or(|state| state.project.clip(clip).is_none())
        }) {
            self.hovered_fade_clip = None;
        }
        if self
            .active_fade_drag
            .as_ref()
            .is_some_and(|drag| !self.fade_drag_current(drag))
        {
            self.active_fade_drag = None;
        }
    }
    pub(super) fn update_fade_gesture(
        &mut self,
        position: Point<Pixels>,
        snap_modifier: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.active_fade_drag.as_ref() else {
            return;
        };
        if !self.fade_drag_current(drag) {
            self.active_fade_drag = None;
            cx.notify();
            return;
        }
        let dx = f32::from(position.x - drag.pointer.x);
        let dy = f32::from(position.y - drag.pointer.y);
        if !dx.is_finite() || !dy.is_finite() || drag.lane.width <= 0.0 {
            return;
        }
        let frame_delta = f64::from(dx) / f64::from(drag.lane.width) * drag.viewport.length as f64;
        let frames = frame_delta.round() as i128;
        let bend = (dy * 2002.0 / (drag.lane.height - super::clip_name::CLIP_NAME_HEIGHT).max(8.0))
            .round()
            .clamp(-2000.0, 2000.0) as i16;
        let lower_edge = match drag.handle {
            Handle::InBottom => Some(hex_project::TrimEdge::Left),
            Handle::OutBottom => Some(hex_project::TrimEdge::Right),
            _ => None,
        };
        let (next, preview_clip, trim_frame) = if let Some(edge) = lower_edge {
            let Some(state) = self.state.as_ref() else {
                return;
            };
            let Some(asset) = state.project.asset(drag.clip.asset()) else {
                return;
            };
            // Delta is measured from the actual lower knot, including authored
            // inward offsets. Zero horizontal displacement is a true no-op.
            if frames == 0 {
                (drag.original, None, None)
            } else {
                let endpoint = match edge {
                    TrimEdge::Left => drag.clip.start() + drag.original.in_start(),
                    TrimEdge::Right => {
                        drag.clip.start() + drag.clip.duration() - drag.original.out_end_offset()
                    }
                };
                let frame = i128::from(endpoint)
                    .saturating_add(frames)
                    .clamp(0, i128::from(u64::MAX)) as u64;
                // Lower knots resize freely by default. Command opts into the
                // configured grid; Off uses the ordinary modifier's adaptive grid.
                let grid = if snap_modifier {
                    match drag.snap_grid {
                        grid if grid.is_off() => grid.toggled(),
                        grid => grid,
                    }
                } else {
                    super::musical_time::SnapGrid::Off
                };
                let Some(frame) =
                    grid.snap_frame(&state.project, frame, drag.viewport, drag.lane.width)
                else {
                    return;
                };
                match drag.clip.preview_fade_edge_trim(
                    edge,
                    frame,
                    asset.playback_frames(),
                    state.project.tempo_map(),
                ) {
                    Ok(clip) if clip == *drag.clip => (drag.original, None, None),
                    Ok(clip) => (clip.retained_fade(), Some(clip), Some(frame)),
                    Err(_) => (drag.original, None, None),
                }
            }
        } else {
            let delta = if matches!(drag.handle, Handle::InCurve | Handle::OutCurve) {
                let Some(end) = drag
                    .clip
                    .start()
                    .checked_add(drag.clip.duration())
                    .zip(drag.viewport.end())
                else {
                    return;
                };
                let visible = end
                    .0
                    .min(end.1)
                    .saturating_sub(drag.clip.start().max(drag.viewport.start))
                    .max(1);
                let width =
                    (drag.lane.width * (visible as f32 / drag.viewport.length as f32)).max(3.0);
                f64::from(dx) * visible.saturating_sub(1).max(1) as f64 / f64::from(width)
            } else {
                frame_delta
            };
            let Some(next) = edited_fade(drag.original, drag.handle, delta, bend) else {
                return;
            };
            (next, None, None)
        };
        if let Some(drag) = self.active_fade_drag.as_mut() {
            drag.moved |= dx.abs() >= 1.0 || dy.abs() >= 1.0;
            drag.trim_frame = trim_frame;
            if drag.preview != next || drag.preview_clip != preview_clip {
                drag.preview = next;
                drag.preview_clip = preview_clip;
                cx.notify();
            }
        }
    }
    pub(super) fn finish_fade_gesture(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = self.active_fade_drag.take() else {
            return;
        };
        if self.focus.is_focused(window)
            && self.fade_drag_current(&drag)
            && drag.moved
            && (drag.preview != drag.original
                || drag
                    .preview_clip
                    .as_ref()
                    .is_some_and(|clip| clip != drag.clip.as_ref()))
        {
            let command = match (drag.handle, drag.trim_frame) {
                (Handle::InBottom | Handle::OutBottom, Some(frame)) => Command::TrimClipFadeEdge {
                    clip: drag.clip.id(),
                    edge: if drag.handle == Handle::InBottom {
                        TrimEdge::Left
                    } else {
                        TrimEdge::Right
                    },
                    frame,
                },
                _ => Command::SetClipFadeEnvelope {
                    clip: drag.clip.id(),
                    fade: drag.preview,
                },
            };
            self.pending_fade = Some(PendingFade {
                receipt: super::pending_edit::Receipt::new(self.project_generation),
                command: command.clone(),
                drag,
            });
            if !self.enqueue(Request::Edit(command)) {
                self.retire_pending_fade();
            }
        }
        cx.notify();
    }
    fn displayed_fade_drag(&self) -> Option<&FadeDrag> {
        self.active_fade_drag
            .as_ref()
            .filter(|d| self.fade_drag_current(d))
            .or_else(|| {
                self.pending_fade
                    .as_ref()
                    .filter(|p| p.source_current(self))
                    .map(|p| &p.drag)
            })
    }
    pub(super) fn fade_preview(&self, clip: ClipId) -> Option<ClipFade> {
        self.displayed_fade_drag()
            .filter(|drag| drag.clip.id() == clip && drag.moved && drag.preview != drag.original)
            .map(|drag| drag.preview)
    }
    pub(super) fn fade_trim_preview(&self, clip: ClipId) -> Option<&Clip> {
        self.displayed_fade_drag()
            .filter(|drag| drag.clip.id() == clip && drag.moved)
            .and_then(|drag| drag.preview_clip.as_ref())
    }
    pub(super) fn fade_curve_active(&self, clip: ClipId) -> bool {
        self.displayed_fade_drag()
            .is_some_and(|d| d.clip.id() == clip)
    }

    fn fade_controls(
        &self,
        track: TrackId,
        clip: &Clip,
        visible: (u64, u64),
        viewport: Viewport,
    ) -> Option<FadeControls> {
        // Header-only rows have no visible fade body; arbitration and rendered
        // handles must both refuse those hidden targets.
        if self.track_height(track) <= super::track_layout::HEADER_ONLY_HEIGHT {
            return None;
        }
        // Held `f` reveals every clip; otherwise only the hovered clip (or the
        // one whose gesture is still displayed) shows its controls.
        let preview = self.fade_preview(clip.id());
        let active = self.fade_curve_active(clip.id());
        if !self.fade_key_held
            && !active
            && self.hovered_fade_clip != Some((self.project_generation, clip.id()))
        {
            return None;
        }
        let fade = preview.unwrap_or(clip.retained_fade());
        let lane = self.drag_lanes.get(&track).copied()?;
        if !lane.width.is_finite()
            || lane.width <= 0.0
            || !lane.height.is_finite()
            || lane.height <= 0.0
            || viewport.length == 0
            || visible.0 >= visible.1
        {
            return None;
        }
        // Hover and held `f` share these local controls, so the hovered clip
        // shows exactly the handle set and corner positions that `f` shows.
        let controls = fade.reanchor(clip.duration()).ok()?;
        let reanchors =
            clip.audio_edit().is_some() || fade.offset() != 0 || fade.span() != clip.duration();
        let mut positions = Vec::with_capacity(6);
        for (handle, (domain_frame, subframe), gain) in knots(controls) {
            if matches!(handle, Handle::InCurve) && controls.fade_in() < 3
                || matches!(handle, Handle::OutCurve) && controls.fade_out() < 3
            {
                continue;
            }
            // Subtract integer origins before conversion, preserving short-clip
            // precision even near the maximum absolute project frame. Local
            // knots are clip frames, so a subframe is already a frame fraction.
            let relative = (i128::from(domain_frame) - i128::from(visible.0 - clip.start())) as f64
                + subframe.max(0.0);
            if !relative.is_finite() || relative < 0.0 || relative >= (visible.1 - visible.0) as f64
            {
                continue;
            }
            let fraction =
                (relative / (visible.1 - visible.0).saturating_sub(1).max(1) as f64) as f32;
            let pixel_width =
                (lane.width * ((visible.1 - visible.0) as f32 / viewport.length as f32)).max(3.0);
            if !pixel_width.is_finite() {
                continue;
            }
            let inset = (4.0_f32).min(pixel_width / 2.0);
            let fraction = (fraction * pixel_width).clamp(inset, pixel_width - inset) / pixel_width;
            let name_height = super::clip_name::CLIP_NAME_HEIGHT;
            let body_height = (lane.height - name_height).max(8.0);
            let vertical_inset = (4.0 / body_height).min(0.5);
            let curve_y = (name_height
                + body_height * (1.0 - gain).clamp(vertical_inset, 1.0 - vertical_inset))
                / lane.height.max(1.0);
            let center_y = match handle {
                Handle::InTop | Handle::OutTop => (name_height + 4.0) / lane.height,
                Handle::InBottom | Handle::OutBottom => (lane.height - 4.0) / lane.height,
                Handle::InCurve | Handle::OutCurve => curve_y,
            };
            positions.push((handle, gpui::point(fraction, center_y)));
        }
        Some(FadeControls {
            lane,
            local_fade: controls,
            reanchors,
            positions,
        })
    }
    pub(super) fn fade_handle_hit(
        &self,
        track: TrackId,
        clip: &Clip,
        x: f32,
        y: f32,
        viewport: Viewport,
    ) -> bool {
        let Some(end) = clip.start().checked_add(clip.duration()) else {
            return false;
        };
        let Some(viewport_end) = viewport.end() else {
            return false;
        };
        let visible = (clip.start().max(viewport.start), end.min(viewport_end));
        let Some(controls) = self.fade_controls(track, clip, visible, viewport) else {
            return false;
        };
        let left = controls.lane.left
            + controls.lane.width
                * ((visible.0 - viewport.start) as f64 / viewport.length as f64) as f32;
        let width = (controls.lane.width
            * ((visible.1 - visible.0) as f32 / viewport.length as f32))
            .max(3.0);
        controls.positions.iter().any(|(_, point)| {
            let center_x = left + point.x * width;
            let center_y = controls.lane.top + point.y * controls.lane.height;
            // Account for the clip's one-pixel border without reserving its body.
            (x - center_x).abs() <= 5.0 && (y - center_y).abs() <= 5.0
        })
    }
    pub(super) fn fade_handles(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &Clip,
        visible: (u64, u64),
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::Stateful<Div>> {
        let Some(controls) = self.fade_controls(track, clip, visible, viewport) else {
            return Vec::new();
        };
        let lane = controls.lane;
        let local_fade = controls.local_fade;
        let reanchors = controls.reanchors;
        let original_clip = Rc::new(clip.clone());
        let mut handles = Vec::new();
        for (handle, position) in controls.positions {
            let clip_id = clip.id();
            let active = self
                .active_fade_drag
                .as_ref()
                .is_some_and(|drag| drag.clip.id() == clip_id && drag.handle == handle);
            let selector = format!("{}-{}", handle.name(), clip_id.value());
            let drag = FadeDrag {
                generation: self.project_generation,
                revision: state.revision,
                track,
                clip: Rc::clone(&original_clip),
                viewport,
                lane,
                handle,
                pointer: gpui::point(px(0.0), px(0.0)),
                original: local_fade,
                preview: local_fade,
                moved: false,
                preview_clip: None,
                trim_frame: None,
                snap_grid: self.snap_grid,
            };
            handles.push(
                div()
                    .id((handle.name(), clip_id.value()))
                    .debug_selector(move || selector.clone())
                    .absolute()
                    .left(relative(position.x))
                    .ml(px(-4.0))
                    .w(px(8.0))
                    .h(px(8.0))
                    .top(relative(position.y))
                    .mt(px(-4.0))
                    .children([handle_shape(
                        matches!(handle, Handle::InCurve | Handle::OutCurve),
                        active,
                    )])
                    .cursor(if matches!(handle, Handle::InCurve | Handle::OutCurve) {
                        gpui::CursorStyle::Crosshair
                    } else {
                        gpui::CursorStyle::ResizeLeftRight
                    })
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::waveform::ClipTooltip {
                            kind: crate::tooltips::Kind::control(handle.name().replace('-', " ")),
                            name: format!(
                                "{}{} · hold f to show all fade handles",
                                handle.name().replace('-', " "),
                                if reanchors {
                                    " · editing fits fades to this clip"
                                } else {
                                    ""
                                }
                            ),
                        })
                        .into()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                            if view.fade_drag_current(&drag)
                                && view.active_fade_drag.is_none()
                                && view.set_selected_clip(track, clip_id, window, cx)
                            {
                                view.active_fade_drag = Some(FadeDrag {
                                    pointer: event.position,
                                    ..drag.clone()
                                });
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(|_, _, cx| cx.stop_propagation()),
            );
        }
        handles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upper_lengths_and_curves_remain_independent() {
        let fade = ClipFade::new(100, 0, 40, 30)
            .unwrap()
            .with_shape(10, 10, 0, 0)
            .unwrap();
        let bent = edited_fade(fade, Handle::InCurve, 0.0, 700).unwrap();
        assert_eq!(
            (bent.in_start(), bent.fade_in(), bent.fade_out()),
            (10, 40, 30)
        );
        assert!(bent.gain_at(20) < fade.gain_at(20));
        let pushed = edited_fade(bent, Handle::InTop, (i128::MAX - 100) as f64, 0).unwrap();
        assert_eq!((pushed.fade_in(), pushed.fade_out()), (80, 0));
        let pushed = edited_fade(bent, Handle::OutTop, i128::MIN as f64, 0).unwrap();
        assert_eq!((pushed.fade_in(), pushed.fade_out()), (0, 80));
    }

    #[test]
    fn top_handles_push_each_other_and_spring_back() {
        let original = ClipFade::new(100, 0, 30, 40)
            .unwrap()
            .with_shape(5, 5, 300, -200)
            .unwrap()
            .with_curve_positions(400, 600)
            .unwrap();
        let lengths = |handle, frames| {
            let fade = edited_fade(original, handle, frames, 0).unwrap();
            assert_eq!(
                (
                    fade.in_start(),
                    fade.out_end_offset(),
                    fade.in_curve(),
                    fade.out_curve(),
                    fade.in_position(),
                    fade.out_position()
                ),
                (5, 5, 300, -200, 400, 600)
            );
            (fade.fade_in(), fade.fade_out())
        };
        // Body is 90 frames; the fades meet once their lengths sum to 90.
        assert_eq!(lengths(Handle::InTop, 20.0), (50, 40));
        assert_eq!(lengths(Handle::InTop, 35.0), (65, 25));
        // Reversing the same drag restores the pushed fade, and no further.
        assert_eq!(lengths(Handle::InTop, 10.0), (40, 40));
        assert_eq!(lengths(Handle::InTop, -30.0), (0, 40));
        assert_eq!(lengths(Handle::InTop, 1e9), (90, 0));
        assert_eq!(lengths(Handle::OutTop, -35.0), (15, 75));
        assert_eq!(lengths(Handle::OutTop, -10.0), (30, 50));
        assert_eq!(lengths(Handle::OutTop, 50.0), (30, 0));
        assert_eq!(lengths(Handle::OutTop, -1e9), (0, 90));
        // Opening an already-overlapping legacy fade only adjusts it once edited.
        let legacy = ClipFade::new(100, 0, 70, 60).unwrap();
        let fade = edited_fade(legacy, Handle::InTop, 0.0, 0).unwrap();
        assert_eq!((fade.fade_in(), fade.fade_out()), (70, 30));
        // Zero-length body leaves both fades empty.
        let empty = ClipFade::new(10, 0, 0, 0)
            .unwrap()
            .with_shape(5, 5, 0, 0)
            .unwrap();
        for handle in [Handle::InTop, Handle::OutTop] {
            let fade = edited_fade(empty, handle, 7.0, 0).unwrap();
            assert_eq!((fade.fade_in(), fade.fade_out()), (0, 0));
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "fade_handles_ui_test.rs"]
mod ui_tests;

// Released input transfers its immutable preview to the ordinary request receipt.
// Canonical state and this exact terminal retire it in one synchronous receive.
pub(super) struct PendingFade {
    receipt: super::pending_edit::Receipt,
    command: Command,
    drag: FadeDrag,
}
impl PendingFade {
    fn source_current(&self, view: &EditorView) -> bool {
        self.drag.generation == view.project_generation
            && view.state.as_ref().is_some_and(|s| {
                s.project.clip(self.drag.clip.id()) == Some(self.drag.clip.as_ref())
            })
    }
    fn matches(&self, request: &Request) -> bool {
        self.receipt.is_queued()
            && match (&self.command, request) {
                (
                    Command::SetClipFadeEnvelope { clip, fade },
                    Request::Edit(Command::SetClipFadeEnvelope { clip: c, fade: f }),
                ) => (clip, fade) == (c, f),
                (
                    Command::TrimClipFadeEdge { clip, edge, frame },
                    Request::Edit(Command::TrimClipFadeEdge {
                        clip: c,
                        edge: e,
                        frame: f,
                    }),
                ) => (clip, edge, frame) == (c, e, f),
                _ => false,
            }
    }
}
impl EditorView {
    pub(super) fn pending_fade_matches(&self, request: &Request) -> bool {
        self.pending_fade
            .as_ref()
            .is_some_and(|p| p.matches(request))
    }
    pub(super) fn fade_submitted(&mut self) {
        if let Some(p) = &mut self.pending_fade {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn fade_request_complete(&mut self, event: &hex_session::service::Event) {
        if let Some(p) = &mut self.pending_fade
            && p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != super::pending_edit::Disposition::Waiting
        {
            self.retire_pending_fade();
        }
    }
    pub(super) fn retire_pending_fade(&mut self) -> bool {
        self.pending_fade.take().is_some()
    }
    pub(super) fn retire_abandoned_fade(&mut self) -> bool {
        let abandoned = self.pending_fade.as_ref().is_some_and(|p| {
            !p.source_current(self)
                || (p.receipt.is_queued()
                    && !self.pending_requests.iter().any(|r| p.matches(r))
                    && !self.device_gesture_barriers.iter().any(|r| p.matches(r)))
        });
        abandoned && self.retire_pending_fade()
    }
}

#[cfg(test)]
mod two_axis_tests {
    use super::*;
    #[test]
    fn overlapping_fades_keep_each_control_inside_its_own_box() {
        let fade = ClipFade::new(101, 0, 101, 101)
            .unwrap()
            .with_shape(0, 0, 500, -500)
            .unwrap()
            .with_curve_positions(250, 750)
            .unwrap();
        for (handle, _, gain) in knots(fade) {
            if matches!(handle, Handle::InCurve | Handle::OutCurve) {
                assert!((0.25..=0.75).contains(&gain));
            }
        }
    }
    #[test]
    fn rectangular_corners_are_independent_and_legacy_handles_stay_on_curve() {
        let original = ClipFade::new(10001, 0, 5001, 5000).unwrap();
        for original in [original, original.with_shape(0, 0, 1000, -1000).unwrap()] {
            for handle in [Handle::InCurve, Handle::OutCurve] {
                for dx in [-100000, 100000] {
                    for dy in [-2000, 2000] {
                        let edited = edited_fade(original, handle, f64::from(dx), dy).unwrap();
                        let out = handle == Handle::OutCurve;
                        let gain = curve_handle_gain(edited, out);
                        let x = edited.position_at_gain(out, gain).unwrap();
                        assert!((gain - if dy > 0 { 0.25 } else { 0.75 }).abs() < 0.001);
                        assert!((x - if dx > 0 { 0.999 } else { 0.001 }).abs() < 0.001);
                        assert_eq!((edited.fade_in(), edited.fade_out()), (5001, 5000));
                    }
                }
            }
        }
        let legacy = original.with_shape(0, 0, 900, -900).unwrap();
        assert_eq!(edited_fade(legacy, Handle::InCurve, 0.0, 10), Some(legacy));
        let inward = edited_fade(legacy, Handle::InCurve, 0.0, -10).unwrap();
        assert!(curve_handle_gain(inward, false) > curve_handle_gain(legacy, false));
        let moved = edited_fade(legacy, Handle::InCurve, 500.0, 0).unwrap();
        assert!((curve_handle_gain(moved, false) - curve_handle_gain(legacy, false)).abs() < 0.001);
        let before = legacy
            .position_at_gain(false, curve_handle_gain(legacy, false))
            .unwrap();
        let after = moved
            .position_at_gain(false, curve_handle_gain(moved, false))
            .unwrap();
        assert!((after - before - 0.1).abs() < 0.002);
    }
}
