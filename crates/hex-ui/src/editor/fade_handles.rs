//! Momentary Arrangement fade controls. Pointer previews never compile audio or
//! mutate history; release submits one validated canonical envelope command.
use super::{EditorView, ViewState, drag::LaneBounds, viewport::Viewport};
use crate::theme;
use gpui::{Context, Div, MouseButton, Pixels, Point, Window, div, prelude::*, px, relative, rgb};
use hex_project::{Clip, ClipFade, ClipId, Command, TrackId, TrimEdge};
use hex_session::service::Request;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    InTop,
    InBottom,
    InCurve,
    OutTop,
    OutBottom,
    OutCurve,
}
impl Handle {
    fn name(self) -> &'static str {
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

fn edited_fade(fade: ClipFade, handle: Handle, frames: i128, bend: i16) -> Option<ClipFade> {
    let span = fade.span();
    let start = fade.in_start();
    let end_offset = fade.out_end_offset();
    let mut fade_in = fade.fade_in();
    let mut fade_out = fade.fade_out();
    let mut in_curve = fade.in_curve();
    let mut out_curve = fade.out_curve();
    let shift = |value: u64, upper: u64| {
        i128::from(value)
            .saturating_add(frames)
            .clamp(0, i128::from(upper)) as u64
    };
    match handle {
        Handle::InTop => fade_in = shift(fade_in, span - start),
        Handle::OutTop => {
            fade_out = i128::from(fade_out)
                .saturating_sub(frames)
                .clamp(0, i128::from(span - end_offset)) as u64
        }
        Handle::InBottom | Handle::OutBottom => return None,
        Handle::InCurve => in_curve = in_curve.saturating_add(bend).clamp(-1000, 1000),
        Handle::OutCurve => out_curve = out_curve.saturating_add(bend).clamp(-1000, 1000),
    }
    ClipFade::new(span, 0, fade_in, fade_out)
        .ok()?
        .with_shape(start, end_offset, in_curve, out_curve)
        .ok()
}

fn knots(fade: ClipFade) -> [(Handle, u64, f32); 6] {
    let end = fade.span() - fade.out_end_offset() - 1;
    let in_top = fade.in_start() + fade.fade_in().saturating_sub(1);
    let out_top = (end + 1).saturating_sub(fade.fade_out()).min(end);
    let in_mid = fade.in_start() + in_top.saturating_sub(fade.in_start()) / 2;
    let out_mid = out_top + (end - out_top) / 2;
    [
        (Handle::InTop, in_top, 1.0),
        (Handle::InBottom, fade.in_start(), 0.0),
        (
            Handle::InCurve,
            in_mid,
            fade.gain_at(in_mid.saturating_sub(fade.offset())),
        ),
        (Handle::OutTop, out_top, 1.0),
        (Handle::OutBottom, end, 0.0),
        (
            Handle::OutCurve,
            out_mid,
            fade.gain_at(out_mid.saturating_sub(fade.offset())),
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
                let entity = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                    if phase.capture() {
                        entity.update(cx, |view, cx| {
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
        let frames = (f64::from(dx) / f64::from(drag.lane.width) * drag.viewport.length as f64)
            .round() as i128;
        let bend = (dy * 25.0).round().clamp(-2000.0, 2000.0) as i16;
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
                        super::musical_time::SnapGrid::Off => {
                            super::musical_time::SnapGrid::Adaptive
                        }
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
            let Some(next) = edited_fade(drag.original, drag.handle, frames, bend) else {
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
            self.enqueue(Request::Edit(command));
        }
        cx.notify();
    }
    pub(super) fn fade_preview(&self, clip: ClipId) -> Option<ClipFade> {
        self.active_fade_drag
            .as_ref()
            .filter(|drag| {
                drag.clip.id() == clip
                    && drag.moved
                    && drag.preview != drag.original
                    && self.fade_drag_current(drag)
            })
            .map(|drag| drag.preview)
    }
    pub(super) fn fade_trim_preview(&self, clip: ClipId) -> Option<&Clip> {
        self.active_fade_drag
            .as_ref()
            .filter(|drag| drag.clip.id() == clip && drag.moved && self.fade_drag_current(drag))
            .and_then(|drag| drag.preview_clip.as_ref())
    }
    fn fade_controls(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &Clip,
        visible: (u64, u64),
        viewport: Viewport,
    ) -> Option<FadeControls> {
        let preview = self.fade_preview(clip.id());
        let active = self
            .active_fade_drag
            .as_ref()
            .is_some_and(|drag| drag.clip.id() == clip.id());
        let fade = preview.unwrap_or(clip.retained_fade());
        if !self.fade_key_held && fade.is_neutral() && !fade.has_shape() && !active {
            return None;
        }
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
        let local_fade = fade.reanchor(clip.duration()).ok()?;
        let local_controls = self.fade_key_held || preview.is_some() || active;
        let controls = if local_controls { local_fade } else { fade };
        let reanchors =
            clip.audio_edit().is_some() || fade.offset() != 0 || fade.span() != clip.duration();
        let mut positions = Vec::with_capacity(6);
        for (handle, domain_frame, gain) in knots(controls) {
            if matches!(handle, Handle::InCurve) && controls.fade_in() < 3
                || matches!(handle, Handle::OutCurve) && controls.fade_out() < 3
            {
                continue;
            }
            let local_frame = if local_controls {
                Some(domain_frame)
            } else if let Some(edit) = clip.audio_edit() {
                edit.projected_fade(state.project.tempo_map())
                    .and_then(|projection| projection.frame_at_domain_position(domain_frame))
                    .ok()
                    .flatten()
                    .zip(edit.projected_domain(state.project.tempo_map()).ok())
                    .and_then(|(root, (_, _, start, _))| root.checked_sub(start))
            } else {
                domain_frame.checked_sub(fade.offset())
            };
            let Some(frame) = local_frame.and_then(|frame| clip.start().checked_add(frame)) else {
                continue;
            };
            if frame < visible.0 || frame >= visible.1 {
                continue;
            }
            let fraction = (frame - visible.0) as f32
                / (visible.1 - visible.0).saturating_sub(1).max(1) as f32;
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
            local_fade,
            reanchors,
            positions,
        })
    }
    pub(super) fn fade_handle_hit(
        &self,
        state: &ViewState,
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
        let Some(controls) = self.fade_controls(state, track, clip, visible, viewport) else {
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
        let Some(controls) = self.fade_controls(state, track, clip, visible, viewport) else {
            return Vec::new();
        };
        let lane = controls.lane;
        let local_fade = controls.local_fade;
        let reanchors = controls.reanchors;
        let original_clip = Rc::new(clip.clone());
        let mut handles = Vec::new();
        for (handle, position) in controls.positions {
            let clip_id = clip.id();
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
                    .border_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Background)))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                    .cursor(if matches!(handle, Handle::InCurve | Handle::OutCurve) {
                        gpui::CursorStyle::ResizeUpDown
                    } else {
                        gpui::CursorStyle::ResizeLeftRight
                    })
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::waveform::ClipTooltip {
                            name: if reanchors {
                                format!(
                                    "{} · editing fits fades to this clip",
                                    handle.name().replace('-', " ")
                                )
                            } else {
                                handle.name().replace('-', " ")
                            },
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
        let bent = edited_fade(fade, Handle::InCurve, 0, 700).unwrap();
        assert_eq!(
            (bent.in_start(), bent.fade_in(), bent.fade_out()),
            (10, 40, 30)
        );
        assert!(bent.gain_at(20) < fade.gain_at(20));
        assert_eq!(
            edited_fade(bent, Handle::InTop, i128::MAX - 100, 0)
                .unwrap()
                .fade_in(),
            90
        );
        assert_eq!(
            edited_fade(bent, Handle::OutTop, i128::MIN, 0)
                .unwrap()
                .fade_out(),
            90
        );
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "fade_handles_ui_test.rs"]
mod ui_tests;
