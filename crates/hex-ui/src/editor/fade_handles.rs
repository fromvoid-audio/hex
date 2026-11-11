//! Hover and momentary Arrangement fade controls. Pointer previews never compile audio or
//! mutate history; release submits one validated canonical envelope command.
use super::{EditorView, ViewState, drag::LaneBounds, viewport::Viewport};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, PathBuilder, Pixels, Point, Window, canvas, div, point, prelude::*,
    px, relative, rgba,
};
use hex_project::{Clip, ClipFade, ClipId, Command, TrackId, TrimEdge};
use hex_session::service::Request;
use std::rc::Rc;

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) mod paint_probe;

#[cfg(all(test, feature = "ui-test-support"))]
use super::fade_handle_geometry::curve_handle_gain;
pub(super) use super::fade_handle_geometry::{Handle, edited_fade, knots};

// Two 8 px endpoints plus an 8 px gap in each dimension. The body starts
// below the title; use post-zoom logical pixels so the threshold is DPI independent.
const MIN_FADE_CONTROL_EXTENT: f32 = 24.0;

fn fade_controls_fit(width: f32, height: f32) -> bool {
    width.is_finite()
        && height.is_finite()
        && width >= MIN_FADE_CONTROL_EXTENT
        && height - super::clip_name::CLIP_NAME_HEIGHT >= MIN_FADE_CONTROL_EXTENT
}

struct FadeControls {
    lane: LaneBounds,
    local_fade: ClipFade,
    reanchors: bool,
    positions: Vec<(Handle, Point<f32>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Hover {
    generation: u64,
    track: TrackId,
    clip: ClipId,
    handle: Handle,
}
impl Hover {
    pub(super) fn in_track(self, generation: u64, track: TrackId) -> Option<(ClipId, Handle)> {
        (self.generation == generation && self.track == track).then_some((self.clip, self.handle))
    }
}

// The hit target stays square; the painted curve control is a diamond.
pub(super) fn handle_shape(curve: bool, state: theme::FadeHandleState) -> impl gpui::IntoElement {
    let style = theme::fade_handle_style(state);
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
                    #[cfg(all(test, feature = "ui-test-support"))]
                    paint_probe::record(bounds, style);
                    window.paint_path(path, rgba(if fill { style.fill } else { style.stroke }));
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
                    && self.drag_lanes.contains_key(&drag.track)
                    && state.project.clip(drag.clip.id()) == Some(drag.clip.as_ref())
            })
    }
    /// Hover follows the same title/body boundary as the waveform surface.
    /// Held and admitted fade display is owned separately by the gesture below.
    pub(super) fn update_fade_hover(
        &mut self,
        track: TrackId,
        clip: ClipId,
        generation: u64,
        pointer_y: Option<f32>,
    ) -> bool {
        if pointer_y.is_some() && generation != self.project_generation {
            return false;
        }
        let target = (generation, clip);
        let over_body = self.track_height(track) > super::track_layout::HEADER_ONLY_HEIGHT
            && self.drag_lane_epochs.get(&track) == Some(&self.drag_layout_generation)
            && pointer_y
                .zip(self.drag_lanes.get(&track))
                .is_some_and(|(y, lane)| {
                    let body_top = lane.top + super::clip_name::CLIP_NAME_HEIGHT;
                    let bottom = lane.top + lane.height;
                    [y, lane.top, lane.height, body_top, bottom]
                        .into_iter()
                        .all(f32::is_finite)
                        && lane.height > super::clip_name::CLIP_NAME_HEIGHT
                        && y >= body_top
                        && y < bottom
                });
        let next = if over_body {
            Some(target)
        } else if self.hovered_fade_clip == Some(target) {
            None
        } else {
            self.hovered_fade_clip
        };
        if self.hovered_fade_clip == next {
            return false;
        }
        self.hovered_fade_clip = next;
        true
    }

    pub(super) fn reconcile_fade_gesture(&mut self) {
        if self.hovered_fade_handle.is_some_and(|hover| {
            hover.generation != self.project_generation
                || self.mode != super::EditorMode::Arrangement
                || self
                    .state
                    .as_ref()
                    .is_none_or(|s| s.project.clip(hover.clip).is_none())
        }) {
            self.hovered_fade_handle = None;
        }
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
        // An admitted gesture keeps its display through sizing changes; its
        // captured lane and viewport continue to own pointer conversion.
        let preview = self.fade_preview(clip.id());
        let active = self.fade_curve_active(clip.id());
        if !active && self.track_height(track) <= super::track_layout::HEADER_ONLY_HEIGHT {
            return None;
        }
        // Held `f` reveals every clip; otherwise only the hovered clip (or the
        // one whose gesture is still displayed) shows its controls.
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
        let visible_width = lane.width * ((visible.1 - visible.0) as f32 / viewport.length as f32);
        if !active && !fade_controls_fit(visible_width, lane.height) {
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
        let Some(mut controls) = self.fade_controls(track, clip, visible, viewport) else {
            return Vec::new();
        };
        let lane = controls.lane;
        let local_fade = controls.local_fade;
        let reanchors = controls.reanchors;
        let original_clip = Rc::new(clip.clone());
        let mut handles = Vec::new();
        // Pushed ramps can share one painted top knot. Keep the captured handle
        // above its idle peer through the same pending receipt, without moving
        // either hit target or changing their stable element identities.
        if let Some(drag) = self
            .displayed_fade_drag()
            .filter(|drag| drag.clip.id() == clip.id())
            && let Some(index) = controls
                .positions
                .iter()
                .position(|(handle, _)| *handle == drag.handle)
        {
            let captured = controls.positions.remove(index);
            controls.positions.push(captured);
        }
        for (handle, position) in controls.positions {
            let clip_id = clip.id();
            let hover = Hover {
                generation: self.project_generation,
                track,
                clip: clip_id,
                handle,
            };
            let display = self.displayed_fade_drag();
            let paint_state =
                if display.is_some_and(|drag| drag.clip.id() == clip_id && drag.handle == handle) {
                    theme::FadeHandleState::Active
                } else if display.is_none() && self.hovered_fade_handle == Some(hover) {
                    theme::FadeHandleState::Hover
                } else {
                    theme::FadeHandleState::Idle
                };
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
                        paint_state,
                    )])
                    .cursor(gpui::CursorStyle::Arrow)
                    .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                        if hover.generation != view.project_generation {
                            return;
                        }
                        let next = if *hovered {
                            Some(hover)
                        } else {
                            view.hovered_fade_handle.filter(|current| *current != hover)
                        };
                        if next != view.hovered_fade_handle {
                            view.hovered_fade_handle = next;
                            cx.notify();
                        }
                    }))
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::waveform::ClipTooltip {
                            kind: crate::tooltips::Kind::control(handle.name().replace('-', " ")),
                            name: format!(
                                "{}{}{} · hold f to show all fade handles",
                                handle.name().replace('-', " "),
                                if handle.curve_side().is_some_and(|out| {
                                    local_fade.curve_laws()[usize::from(out)]
                                        == ClipFade::DEFAULT_S_CURVE
                                }) {
                                    " · double click to straighten"
                                } else {
                                    ""
                                },
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
                                let straighten = handle.curve_side().filter(|&out| {
                                    event.click_count == 2
                                        && drag.original.curve_laws()[usize::from(out)]
                                            == ClipFade::DEFAULT_S_CURVE
                                });
                                view.active_fade_drag = Some(FadeDrag {
                                    pointer: event.position,
                                    preview: straighten.map_or(drag.original, |out| {
                                        drag.original.straighten_ramp(out)
                                    }),
                                    moved: straighten.is_some(),
                                    ..drag.clone()
                                });
                                if straighten.is_some() {
                                    view.finish_fade_gesture(window, cx);
                                } else {
                                    cx.notify();
                                }
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
mod tests {
    use super::*;
    #[test]
    fn fade_control_geometry_requires_room_in_both_dimensions() {
        let height = super::super::clip_name::CLIP_NAME_HEIGHT + MIN_FADE_CONTROL_EXTENT;
        assert!(!fade_controls_fit(MIN_FADE_CONTROL_EXTENT - 0.01, height));
        assert!(!fade_controls_fit(MIN_FADE_CONTROL_EXTENT, height - 0.01));
        assert!(fade_controls_fit(MIN_FADE_CONTROL_EXTENT, height));
        assert!(!fade_controls_fit(f32::NAN, height));
        assert!(!fade_controls_fit(MIN_FADE_CONTROL_EXTENT, f32::INFINITY));
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "fade_handles_ui_test.rs"]
mod ui_tests;
