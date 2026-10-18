//! Native pointer trim gesture state and bounded canonical dispatch.
use super::{EditorView, ViewState, drag::LaneBounds, viewport::Viewport};
use gpui::{Context, CursorStyle, Div, MouseButton, Stateful, Window, div, prelude::*, px};
use hex_project::{BeatTime, Clip, Command, MidiClip, MidiClipTrim, Project, TrackId, TrimEdge};
use hex_session::service::Request;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const HANDLE_WIDTH: f32 = 4.0;
const MIN_MOVE_WIDTH: f32 = 1.0;

#[derive(Clone, Debug)]
pub(super) struct TrimPreviewState {
    pub(super) clip: Clip,
}

fn replace_audio_trim_preview(
    preview: &RefCell<Option<TrimPreviewState>>,
    next: Option<TrimPreviewState>,
) -> bool {
    let Ok(mut current) = preview.try_borrow_mut() else {
        return false;
    };
    if current.as_ref().map(|preview| &preview.clip) == next.as_ref().map(|preview| &preview.clip) {
        return false;
    }
    *current = next;
    true
}

fn replace_midi_trim_preview(
    preview: &RefCell<Option<MidiClipTrim>>,
    next: Option<MidiClipTrim>,
) -> bool {
    let Ok(mut current) = preview.try_borrow_mut() else {
        return false;
    };
    if *current == next {
        return false;
    }
    *current = next;
    true
}

/// Bounded metadata-only payload; the model helper builds previews without project or PCM copies.
#[derive(Clone, Debug)]
pub(super) struct TrimDrag {
    canceled: Rc<Cell<bool>>,
    pub(super) stretch_modifier: Rc<Cell<bool>>,
    project_generation: u64,
    revision: u64,
    pub(super) source_track: TrackId,
    pub(super) clip: Clip,
    asset_playback_frames: u64,
    edge: TrimEdge,
    source_lane: LaneBounds,
    viewport_start: u64,
    viewport_length: u64,
    pub(super) scroll: super::drag_autoscroll::DragScroll,
    snap_grid: super::musical_time::SnapGrid,
    pub(super) snap_modifier: Rc<Cell<bool>>,
    pub(super) last_pointer_x: Rc<Cell<Option<f32>>>,
    pub(super) grab_delta: Rc<RefCell<Option<i128>>>,
    pub(super) preview: Rc<RefCell<Option<TrimPreviewState>>>,
}

/// Metadata-only Arrangement MIDI edge gesture. The authored notes are shared
/// through one `Rc`; pointer updates and previews never clone the note list.
#[derive(Clone, Debug)]
pub(super) struct MidiTrimDrag {
    canceled: Rc<Cell<bool>>,
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) source_track: TrackId,
    pub(super) clip: Rc<MidiClip>,
    edge: TrimEdge,
    source_lane: LaneBounds,
    viewport_start: u64,
    viewport_length: u64,
    pub(super) scroll: super::drag_autoscroll::DragScroll,
    snap_grid: super::musical_time::SnapGrid,
    pub(super) snap_modifier: Rc<Cell<bool>>,
    pub(super) last_pointer_x: Rc<Cell<Option<f32>>>,
    grab_delta: Rc<RefCell<Option<i128>>>,
    pub(super) preview: Rc<RefCell<Option<MidiClipTrim>>>,
}

fn frame_at_x(x: f32, lane: LaneBounds, viewport: Viewport) -> Option<u64> {
    if !x.is_finite()
        || !lane.left.is_finite()
        || !lane.width.is_finite()
        || lane.width <= 0.0
        || viewport.length == 0
    {
        return None;
    }
    let fraction = ((f64::from(x) - f64::from(lane.left)) / f64::from(lane.width)).clamp(0.0, 1.0);
    let in_view = (fraction * viewport.length as f64).floor() as u64;
    viewport.start.checked_add(in_view.min(viewport.length))
}

fn corrected_edge_frame(pointer_frame: u64, grab_delta: i128) -> u64 {
    (i128::from(pointer_frame) - grab_delta).clamp(0, i128::from(u64::MAX)) as u64
}

fn edge_frame(clip: &Clip, edge: TrimEdge) -> Option<u64> {
    match edge {
        TrimEdge::Left => Some(clip.start()),
        TrimEdge::Right => clip.start().checked_add(clip.duration()),
    }
}

fn drag_admitted(view: &EditorView) -> bool {
    trim_drag_admitted(
        view.opening_project,
        view.close_prompt_open,
        view.worker_requests_pending,
        view.pending_requests.len(),
    )
}

fn trim_drag_admitted(
    opening_project: bool,
    close_prompt_open: bool,
    worker_requests_pending: usize,
    pending_requests: usize,
) -> bool {
    !opening_project && !close_prompt_open && worker_requests_pending == 0 && pending_requests == 0
}

fn payload_is_current(
    drag: &TrimDrag,
    generation: u64,
    state: &ViewState,
    viewport: Viewport,
    source_lane: Option<LaneBounds>,
) -> bool {
    !drag.canceled.get()
        && drag.project_generation == generation
        && drag.revision == state.revision
        && drag.viewport().start == viewport.start
        && drag.viewport().length == viewport.length
        && source_lane.is_some_and(|lane| {
            lane.left == drag.source_lane.left && lane.width == drag.source_lane.width
        })
        && state
            .project
            .track(drag.source_track)
            .and_then(|track| {
                track
                    .clips()
                    .iter()
                    .find(|clip| clip.id() == drag.clip.id())
            })
            .is_some_and(|clip| clip == &drag.clip)
        && state
            .project
            .asset(drag.clip.asset())
            .is_some_and(|asset| asset.playback_frames() == drag.asset_playback_frames)
}

fn midi_payload_is_current(
    drag: &MidiTrimDrag,
    generation: u64,
    state: &ViewState,
    viewport: Viewport,
    source_lane: Option<LaneBounds>,
) -> bool {
    !drag.canceled.get()
        && drag.project_generation == generation
        && drag.revision == state.revision
        && drag.viewport().start == viewport.start
        && drag.viewport().length == viewport.length
        && source_lane.is_some_and(|lane| {
            lane.left == drag.source_lane.left && lane.width == drag.source_lane.width
        })
        && state
            .project
            .track(drag.source_track)
            .and_then(|track| {
                track
                    .midi_clips()
                    .iter()
                    .find(|clip| clip.id() == drag.clip.id())
            })
            .is_some_and(|clip| clip == drag.clip.as_ref())
}

fn preview_is_noop(original: &Clip, preview: &Clip) -> bool {
    original == preview
}

pub(super) fn handle_width(clip_width: f32) -> f32 {
    if !clip_width.is_finite() || clip_width <= 0.0 {
        return 0.0;
    }
    HANDLE_WIDTH.min((clip_width - MIN_MOVE_WIDTH).max(0.0) / 2.0)
}

fn bracket_cursor(edge: TrimEdge) -> CursorStyle {
    match edge {
        TrimEdge::Left => CursorStyle::ResizeLeftBracket,
        TrimEdge::Right => CursorStyle::ResizeRightBracket,
    }
}

pub(super) fn rendered_clip_width(lane_width: f32, width_fraction: f64, minimum: f32) -> f32 {
    let width = lane_width * width_fraction as f32;
    if !width.is_finite() || width <= 0.0 || !minimum.is_finite() || minimum <= 0.0 {
        return 0.0;
    }
    width.max(minimum)
}

fn snapped_edge_frame(
    drag: &TrimDrag,
    project: &hex_project::Project,
    pointer_x: f32,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<u64> {
    let pointer_frame = frame_at_x(pointer_x, lane, viewport)?;
    let delta = (*drag.grab_delta.borrow())?;
    let edge = corrected_edge_frame(pointer_frame, delta);
    drag.snap_grid
        .with_platform_modifier(drag.snap_modifier.get())
        .snap_frame(project, edge, viewport, lane.width)
}

fn preview_for_pointer(
    drag: &TrimDrag,
    project: &hex_project::Project,
    pointer_x: f32,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<TrimPreviewState> {
    let frame = snapped_edge_frame(drag, project, pointer_x, lane, viewport)?;
    let clip = preview_clip(drag, project, frame).ok()?;
    Some(TrimPreviewState { clip })
}

fn preview_clip(
    drag: &TrimDrag,
    project: &Project,
    frame: u64,
) -> Result<Clip, hex_project::Invalid> {
    if drag.stretch_modifier.get() {
        hex_project::preview_audio_clip_stretch(
            &drag.clip,
            drag.edge,
            frame,
            project.tempo_map(),
            drag.asset_playback_frames,
        )
    } else {
        drag.clip.preview_edge_trim_with_tempo(
            drag.edge,
            frame,
            drag.asset_playback_frames,
            project.tempo_map(),
        )
    }
}

fn midi_edge_beat(clip: &MidiClip, edge: TrimEdge) -> Option<BeatTime> {
    match edge {
        TrimEdge::Left => Some(clip.start()),
        TrimEdge::Right => clip
            .start()
            .raw()
            .checked_add(clip.length().raw())
            .map(BeatTime::from_raw),
    }
}

fn midi_pointer_beat(
    project: &Project,
    pointer_x: f32,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<BeatTime> {
    let frame = frame_at_x(pointer_x, lane, viewport)?;
    project.tempo_map().frame_to_beat(frame).ok()
}

fn snapped_midi_edge_beat(
    drag: &MidiTrimDrag,
    project: &Project,
    pointer_x: f32,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<BeatTime> {
    let pointer = midi_pointer_beat(project, pointer_x, lane, viewport)?;
    let delta = (*drag.grab_delta.borrow())?;
    let candidate = BeatTime::from_raw(
        (i128::from(pointer.raw()) - delta).clamp(0, i128::from(u64::MAX)) as u64,
    );
    drag.snap_grid
        .with_platform_modifier(drag.snap_modifier.get())
        .snap_beat(project, candidate, viewport, lane.width)
}

fn midi_preview_for_pointer(
    drag: &MidiTrimDrag,
    project: &Project,
    pointer_x: f32,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<MidiClipTrim> {
    drag.clip
        .preview_edge_trim(
            drag.edge,
            snapped_midi_edge_beat(drag, project, pointer_x, lane, viewport)?,
        )
        .ok()
}

impl EditorView {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trim_handles(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &Clip,
        visible_start: u64,
        visible_end: u64,
        clip_width: f32,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        if !drag_admitted(self) || handle_width(clip_width) <= 0.0 {
            return Vec::new();
        }
        let Some(lane) = self.drag_lanes.get(&track).copied() else {
            return Vec::new();
        };
        let Some(asset) = state.project.asset(clip.asset()) else {
            return Vec::new();
        };
        let mut handles = Vec::with_capacity(2);
        if visible_start == clip.start() {
            handles.push(self.trim_handle(
                state,
                track,
                clip,
                TrimEdge::Left,
                lane,
                clip_width,
                viewport,
                asset.playback_frames(),
                cx,
            ));
        }
        if let Some(end) = clip.start().checked_add(clip.duration())
            && visible_end == end
        {
            handles.push(self.trim_handle(
                state,
                track,
                clip,
                TrimEdge::Right,
                lane,
                clip_width,
                viewport,
                asset.playback_frames(),
                cx,
            ));
        }
        handles
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn midi_trim_handles(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &MidiClip,
        clip_start_frame: u64,
        clip_end_frame: u64,
        visible_start: u64,
        visible_end: u64,
        clip_width: f32,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        if !drag_admitted(self) || handle_width(clip_width) <= 0.0 {
            return Vec::new();
        }
        let Some(lane) = self.drag_lanes.get(&track).copied() else {
            return Vec::new();
        };
        let drag = MidiTrimDrag {
            canceled: Rc::new(Cell::new(false)),
            project_generation: self.project_generation,
            revision: state.revision,
            source_track: track,
            clip: Rc::new(clip.clone()),
            edge: TrimEdge::Left,
            source_lane: lane,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            last_pointer_x: Rc::new(Cell::new(None)),
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
        };
        let mut handles = Vec::with_capacity(2);
        if visible_start == clip_start_frame {
            handles.push(self.midi_trim_handle(
                state,
                drag.clone(),
                TrimEdge::Left,
                clip_width,
                lane,
                viewport,
                cx,
            ));
        }
        if visible_end == clip_end_frame {
            let mut drag = drag;
            drag.edge = TrimEdge::Right;
            handles.push(self.midi_trim_handle(
                state,
                drag,
                TrimEdge::Right,
                clip_width,
                lane,
                viewport,
                cx,
            ));
        }
        handles
    }

    #[allow(clippy::too_many_arguments)]
    fn midi_trim_handle(
        &self,
        state: &ViewState,
        payload: MidiTrimDrag,
        edge: TrimEdge,
        clip_width: f32,
        lane: LaneBounds,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let clip_id = payload.clip.id();
        let clip_generation = payload.project_generation;
        let drag_entity = cx.entity();
        let preview_owner_entity = drag_entity.clone();
        let mouse_down_entity = drag_entity.clone();
        let drag_start_entity = drag_entity.clone();
        let pointer_payload = payload.clone();
        let clear_entity = drag_entity.clone();
        let project = Rc::clone(&payload.clip);
        let project_tempo = state.project.clone();
        div()
            .id((
                match edge {
                    TrimEdge::Left => "midi-trim-left",
                    TrimEdge::Right => "midi-trim-right",
                },
                clip_id.value(),
            ))
            .debug_selector(|| {
                format!(
                    "midi-clip-trim-{}-{}",
                    match edge {
                        TrimEdge::Left => "left",
                        TrimEdge::Right => "right",
                    },
                    clip_id.value()
                )
            })
            .absolute()
            .top(px(-1.0))
            .bottom(px(-1.0))
            .w(px(handle_width(clip_width)))
            .when(edge == TrimEdge::Left, |this| this.left(px(-1.0)))
            .when(edge == TrimEdge::Right, |this| this.right(px(-1.0)))
            .cursor(bracket_cursor(edge))
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                mouse_down_entity.update(cx, |view, _| {
                    if view.midi_trim_drag_current(&pointer_payload) {
                        view.remember_pointer_down(
                            clip_generation,
                            clip_id,
                            Some(edge),
                            f32::from(event.position.x),
                        );
                    } else {
                        view.clear_pointer_down(clip_generation, clip_id, Some(edge));
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                clear_entity.update(cx, |view, _| {
                    view.clear_pointer_down(clip_generation, clip_id, Some(edge));
                });
            })
            .on_drag(payload, move |payload, _, window, cx| {
                let original_down_x = drag_start_entity.update(cx, |view, _| {
                    if view.midi_trim_drag_current(payload) {
                        view.take_pointer_down(clip_generation, clip_id, Some(edge))
                    } else {
                        view.clear_pointer_down(clip_generation, clip_id, Some(edge));
                        None
                    }
                });
                let pointer_beat = original_down_x
                    .and_then(|x| midi_pointer_beat(&project_tempo, x, lane, viewport));
                let delta =
                    pointer_beat
                        .zip(midi_edge_beat(&project, edge))
                        .map(|(pointer, edge_beat)| {
                            i128::from(pointer.raw()) - i128::from(edge_beat.raw())
                        });
                *payload.grab_delta.borrow_mut() = delta;
                let pointer_x = f32::from(window.mouse_position().x);
                payload.last_pointer_x.set(Some(pointer_x));
                payload.snap_modifier.set(window.modifiers().platform);
                preview_owner_entity.update(cx, |view, cx| {
                    view.begin_midi_trim_drag(payload);
                    view.update_midi_trim_preview(payload, pointer_x, cx);
                });
                cx.new(|_| TrimPreview {
                    data: Rc::new(RefCell::new(None)),
                })
            })
            .on_drag_move::<MidiTrimDrag>(move |event, _, cx| {
                let Some(drag) = event.dragged_item().downcast_ref::<MidiTrimDrag>() else {
                    return;
                };
                let current = drag_entity.update(cx, |view, _| view.midi_trim_drag_current(drag));
                if !current {
                    replace_midi_trim_preview(&drag.preview, None);
                    drag_entity.update(cx, |view, cx| {
                        view.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
                    });
                    return;
                }
                let pointer_x = f32::from(event.event.position.x);
                drag.last_pointer_x.set(Some(pointer_x));
                drag.snap_modifier.set(event.event.modifiers.platform);
                drag_entity.update(cx, |view, cx| {
                    view.update_midi_trim_preview(drag, pointer_x, cx)
                });
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn trim_handle(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &Clip,
        edge: TrimEdge,
        lane: LaneBounds,
        clip_width: f32,
        viewport: Viewport,
        asset_playback_frames: u64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let clip_id = clip.id();
        let payload = TrimDrag {
            canceled: Rc::new(Cell::new(false)),
            stretch_modifier: Rc::new(Cell::new(false)),
            project_generation: self.project_generation,
            revision: state.revision,
            source_track: track,
            clip: clip.clone(),
            asset_playback_frames,
            edge,
            source_lane: lane,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            last_pointer_x: Rc::new(Cell::new(None)),
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
        };
        let drag_entity = cx.entity();
        let preview_owner_entity = drag_entity.clone();
        let mouse_down_entity = drag_entity.clone();
        let drag_start_entity = drag_entity.clone();
        let pointer_payload = payload.clone();
        let clear_payload = payload.clone();
        let clear_entity = drag_entity.clone();
        div()
            .id((
                match edge {
                    TrimEdge::Left => "trim-left",
                    TrimEdge::Right => "trim-right",
                },
                clip_id.value(),
            ))
            .debug_selector(|| {
                format!(
                    "clip-trim-{}-{}",
                    match edge {
                        TrimEdge::Left => "left",
                        TrimEdge::Right => "right",
                    },
                    clip_id.value()
                )
            })
            .absolute()
            .top(px(-1.0))
            .bottom(px(-1.0))
            .w(px(handle_width(clip_width)))
            .when(edge == TrimEdge::Left, |this| this.left(px(-1.0)))
            .when(edge == TrimEdge::Right, |this| this.right(px(-1.0)))
            .cursor(bracket_cursor(edge))
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                mouse_down_entity.update(cx, |view, _| {
                    if view.trim_drag_current(&pointer_payload) {
                        view.focus.focus(window);
                        view.remember_pointer_down(
                            pointer_payload.project_generation,
                            clip_id,
                            Some(edge),
                            f32::from(event.position.x),
                        );
                    } else {
                        view.clear_pointer_down(
                            pointer_payload.project_generation,
                            clip_id,
                            Some(edge),
                        );
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                clear_entity.update(cx, |view, _| {
                    view.clear_pointer_down(clear_payload.project_generation, clip_id, Some(edge));
                });
            })
            .on_drag(payload, move |payload, _cursor_offset, window, cx| {
                let original_down_x = drag_start_entity.update(cx, |view, _| {
                    if view.trim_drag_current(payload) {
                        view.take_pointer_down(
                            payload.project_generation,
                            payload.clip.id(),
                            Some(payload.edge),
                        )
                    } else {
                        view.clear_pointer_down(
                            payload.project_generation,
                            payload.clip.id(),
                            Some(payload.edge),
                        );
                        None
                    }
                });
                let pointer_frame = original_down_x.and_then(|x| frame_at_x(x, lane, viewport));
                let delta = pointer_frame
                    .zip(edge_frame(&payload.clip, payload.edge))
                    .map(|(pointer, edge)| i128::from(pointer) - i128::from(edge));
                *payload.grab_delta.borrow_mut() = delta;
                let pointer_x = f32::from(window.mouse_position().x);
                payload.last_pointer_x.set(Some(pointer_x));
                payload.snap_modifier.set(window.modifiers().platform);
                payload.stretch_modifier.set(window.modifiers().shift);
                preview_owner_entity.update(cx, |view, cx| {
                    view.begin_trim_drag(payload);
                    view.update_trim_preview(payload, pointer_x, cx);
                });
                cx.new(|_| TrimPreview {
                    data: payload.preview.clone(),
                })
            })
            .on_drag_move::<TrimDrag>(move |event, _, cx| {
                let Some(drag) = event.dragged_item().downcast_ref::<TrimDrag>() else {
                    return;
                };
                let current = drag_entity.update(cx, |view, _| view.trim_drag_current(drag));
                if !current {
                    replace_audio_trim_preview(&drag.preview, None);
                    drag_entity.update(cx, |view, cx| {
                        view.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
                    });
                    return;
                }
                let pointer_x = f32::from(event.event.position.x);
                drag.last_pointer_x.set(Some(pointer_x));
                drag.snap_modifier.set(event.event.modifiers.platform);
                drag.stretch_modifier.set(event.event.modifiers.shift);
                drag_entity.update(cx, |view, cx| view.update_trim_preview(drag, pointer_x, cx));
            })
    }

    pub(super) fn cancel_trim_gesture(&mut self, cx: &mut Context<Self>) -> bool {
        self.retire_trim_gesture(true, cx)
    }

    fn normalize_canceled_trim_viewport(&mut self, generation: u64, owned: Option<Viewport>) {
        if generation == self.project_generation
            && owned.is_some()
            && self.viewport == owned
            && self.active_trim_drag.is_none()
            && self.active_midi_trim_drag.is_none()
            && let Some(horizon) = self.state.as_ref().map(|state| state.timeline_frames)
        {
            // The active display-horizon owner is already retired. Persist the
            // actual canonical clamp so later unrelated growth cannot resurrect
            // an offset that was concealed by visible_viewport's current clamp.
            self.viewport = Some(self.visible_viewport(horizon));
        }
    }

    fn retire_trim_gesture(&mut self, canceled_view: bool, cx: &mut Context<Self>) -> bool {
        let audio = self.active_trim_drag.take();
        let midi = self.active_midi_trim_drag.take();
        let canceled = audio.is_some() || midi.is_some();
        if let Some(drag) = audio {
            drag.canceled.set(true);
            drag.preview.borrow_mut().take();
            if canceled_view {
                self.normalize_canceled_trim_viewport(
                    drag.project_generation,
                    drag.scroll.get().viewport,
                );
            }
        }
        if let Some(drag) = midi {
            drag.canceled.set(true);
            drag.preview.borrow_mut().take();
            if canceled_view {
                self.normalize_canceled_trim_viewport(
                    drag.project_generation,
                    drag.scroll.get().viewport,
                );
            }
        }
        if canceled {
            cx.notify();
        }
        canceled
    }

    pub(super) fn begin_trim_drag(&mut self, drag: &TrimDrag) {
        self.active_trim_drag = Some(drag.clone());
    }

    pub(super) fn clear_trim_drag(
        &mut self,
        generation: u64,
        clip: hex_project::ClipId,
        cx: &mut Context<Self>,
    ) {
        if self.active_trim_drag.as_ref().is_some_and(|active| {
            active.project_generation == generation && active.clip.id() == clip
        }) {
            self.cancel_trim_gesture(cx);
        }
    }

    pub(super) fn update_trim_preview(
        &mut self,
        drag: &TrimDrag,
        pointer_x: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(lane) = self.drag_lanes.get(&drag.source_track).copied() else {
            return false;
        };
        if !payload_is_current(drag, self.project_generation, state, viewport, Some(lane)) {
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return false;
        }
        let changed = replace_audio_trim_preview(
            &drag.preview,
            preview_for_pointer(drag, &state.project, pointer_x, lane, viewport),
        );
        self.active_trim_drag = Some(drag.clone());
        if changed {
            cx.notify();
        }
        changed
    }

    pub(super) fn refresh_trim_snap_modifier(
        &mut self,
        held: bool,
        stretch: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.active_trim_drag.clone()
            && {
                let snap_changed = drag.snap_modifier.replace(held) != held;
                let stretch_changed = drag.stretch_modifier.replace(stretch) != stretch;
                snap_changed || stretch_changed
            }
            && let Some(pointer_x) = drag.last_pointer_x.get()
        {
            self.update_trim_preview(&drag, pointer_x, cx);
        }
    }

    pub(super) fn begin_midi_trim_drag(&mut self, drag: &MidiTrimDrag) {
        self.active_midi_trim_drag = Some(drag.clone());
    }

    pub(super) fn clear_midi_trim_drag(
        &mut self,
        generation: u64,
        clip: hex_project::ClipId,
        cx: &mut Context<Self>,
    ) {
        if self.active_midi_trim_drag.as_ref().is_some_and(|active| {
            active.project_generation == generation && active.clip.id() == clip
        }) {
            if let Some(drag) = self.active_midi_trim_drag.take() {
                drag.canceled.set(true);
                drag.preview.borrow_mut().take();
                self.normalize_canceled_trim_viewport(
                    drag.project_generation,
                    drag.scroll.get().viewport,
                );
            }
            cx.notify();
        }
    }

    pub(super) fn update_midi_trim_preview(
        &mut self,
        drag: &MidiTrimDrag,
        pointer_x: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(lane) = self.drag_lanes.get(&drag.source_track).copied() else {
            return false;
        };
        if !midi_payload_is_current(drag, self.project_generation, state, viewport, Some(lane)) {
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return false;
        }
        let changed = replace_midi_trim_preview(
            &drag.preview,
            midi_preview_for_pointer(drag, &state.project, pointer_x, lane, viewport),
        );
        self.active_midi_trim_drag = Some(drag.clone());
        if changed {
            cx.notify();
        }
        changed
    }

    pub(super) fn refresh_midi_trim_snap_modifier(&mut self, held: bool, cx: &mut Context<Self>) {
        if let Some(drag) = self.active_midi_trim_drag.clone()
            && drag.snap_modifier.replace(held) != held
            && let Some(pointer_x) = drag.last_pointer_x.get()
        {
            self.update_midi_trim_preview(&drag, pointer_x, cx);
        }
    }

    pub(super) fn midi_trim_drag_current(&self, drag: &MidiTrimDrag) -> bool {
        if !drag_admitted(self) {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        midi_payload_is_current(
            drag,
            self.project_generation,
            state,
            self.visible_viewport(state.timeline_frames),
            self.drag_lanes.get(&drag.source_track).copied(),
        )
    }

    pub(super) fn drop_midi_trim(
        &mut self,
        drag: &MidiTrimDrag,
        target_track: TrackId,
        cx: &mut Context<Self>,
    ) {
        if target_track != drag.source_track || !drag_admitted(self) {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(lane) = self.drag_lanes.get(&target_track).copied() else {
            return;
        };
        if !midi_payload_is_current(drag, self.project_generation, state, viewport, Some(lane)) {
            self.error = Some("midi clip trim canceled · project changed".into());
            cx.notify();
            return;
        }
        let Some(pointer_x) = drag.last_pointer_x.get() else {
            return;
        };
        let Some(beat) = snapped_midi_edge_beat(drag, &state.project, pointer_x, lane, viewport)
        else {
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        };
        let Ok(preview) = drag.clip.preview_edge_trim(drag.edge, beat) else {
            return;
        };
        if preview.start() == drag.clip.start()
            && preview.length() == drag.clip.length()
            && preview.source_start() == drag.clip.source_start()
        {
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let accepted = self.enqueue(Request::Edit(Command::TrimMidiClipEdge {
            clip: drag.clip.id(),
            edge: drag.edge,
            beat,
        }));
        self.retire_trim_gesture(!accepted, cx);
        cx.notify();
    }

    pub(super) fn trim_pan_viewport(
        &self,
        drag: &TrimDrag,
        x: f32,
        next: Viewport,
    ) -> Option<Viewport> {
        let state = self.state.as_ref()?;
        let lane = self.drag_lanes.get(&drag.source_track).copied()?;
        let delta = (*drag.grab_delta.borrow())?;
        let candidate = |viewport| {
            let frame = corrected_edge_frame(frame_at_x(x, lane, viewport)?, delta);
            preview_clip(drag, &state.project, frame).ok()
        };
        let current = drag.viewport();
        let before = candidate(current)?;
        if let Some(after) = candidate(next) {
            return (before != after).then_some(next);
        }
        // A full timer step can cross the opposite edge during inward stretch.
        // Search only this checked step's valid prefix, with the canonical model
        // still deciding legality. The maximum is 64 probes, never unbounded
        // one-frame retries or a relaxed minimum/source interval.
        let mut valid = 0;
        let mut invalid = current.start.abs_diff(next.start);
        let viewport_at = |distance| Viewport {
            start: if next.start > current.start {
                current.start + distance
            } else {
                current.start - distance
            },
            length: current.length,
        };
        while invalid.saturating_sub(valid) > 1 {
            let middle = valid + (invalid - valid) / 2;
            if candidate(viewport_at(middle)).is_some() {
                valid = middle;
            } else {
                invalid = middle;
            }
        }
        let last = viewport_at(valid);
        candidate(last)
            .filter(|after| *after != before)
            .map(|_| last)
    }

    pub(super) fn midi_trim_pan_advances(
        &self,
        drag: &MidiTrimDrag,
        x: f32,
        next: Viewport,
    ) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        let Some(lane) = self.drag_lanes.get(&drag.source_track).copied() else {
            return false;
        };
        let Some(delta) = *drag.grab_delta.borrow() else {
            return false;
        };
        let candidate = |viewport| {
            let beat = midi_pointer_beat(&state.project, x, lane, viewport)?;
            let beat = BeatTime::from_raw(
                (i128::from(beat.raw()) - delta).clamp(0, i128::from(u64::MAX)) as u64,
            );
            drag.clip.preview_edge_trim(drag.edge, beat).ok()
        };
        match (candidate(drag.viewport()), candidate(next)) {
            (Some(before), Some(after)) => before != after,
            _ => false,
        }
    }

    /// A private resize may navigate beyond the current display extent, never
    /// beyond its checked viewport or into a stale owner's project/geometry.
    pub(super) fn trim_display_end(&self) -> Option<u64> {
        if !drag_admitted(self) {
            return None;
        }
        let state = self.state.as_ref()?;
        if let Some(drag) = &self.active_trim_drag {
            let viewport = drag.scroll.get().viewport?;
            if self.viewport == Some(viewport)
                && payload_is_current(
                    drag,
                    self.project_generation,
                    state,
                    viewport,
                    self.drag_lanes.get(&drag.source_track).copied(),
                )
            {
                return viewport.end();
            }
        }
        if let Some(drag) = &self.active_midi_trim_drag {
            let viewport = drag.scroll.get().viewport?;
            if self.viewport == Some(viewport)
                && midi_payload_is_current(
                    drag,
                    self.project_generation,
                    state,
                    viewport,
                    self.drag_lanes.get(&drag.source_track).copied(),
                )
            {
                return viewport.end();
            }
        }
        None
    }

    pub(super) fn trim_drag_current(&self, drag: &TrimDrag) -> bool {
        if !drag_admitted(self) {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        payload_is_current(
            drag,
            self.project_generation,
            state,
            self.visible_viewport(state.timeline_frames),
            self.drag_lanes.get(&drag.source_track).copied(),
        )
    }

    pub(super) fn drop_trim(
        &mut self,
        drag: &TrimDrag,
        target_track: TrackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            drag.canceled.set(true);
            self.cancel_trim_gesture(cx);
            return;
        }
        if target_track != drag.source_track || !drag_admitted(self) {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(lane) = self.drag_lanes.get(&target_track).copied() else {
            return;
        };
        if !payload_is_current(drag, self.project_generation, state, viewport, Some(lane)) {
            self.error = Some("clip trim canceled · project changed".into());
            cx.notify();
            return;
        }
        // GPUI's MouseUpEvent carries the authoritative release point. Window's
        // cached pointer can still be the previous move-event location.
        let pointer_x = drag
            .last_pointer_x
            .get()
            .unwrap_or_else(|| f32::from(window.mouse_position().x));
        drag.last_pointer_x.set(Some(pointer_x));
        #[cfg(test)]
        {
            self.last_trim_release_snap = Some(drag.snap_modifier.get());
        }
        let Some(frame) = snapped_edge_frame(drag, &state.project, pointer_x, lane, viewport)
        else {
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        };
        let Ok(preview) = preview_clip(drag, &state.project, frame) else {
            return;
        };
        if preview_is_noop(&drag.clip, &preview) {
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let accepted = self.enqueue(Request::Edit(if drag.stretch_modifier.get() {
            Command::StretchAudioClipEdge {
                clip: drag.clip.id(),
                edge: drag.edge,
                frame,
            }
        } else {
            Command::TrimClipEdge {
                clip: drag.clip.id(),
                edge: drag.edge,
                frame,
            }
        }));
        self.retire_trim_gesture(!accepted, cx);
        cx.notify();
    }
}

struct TrimPreview {
    data: Rc<RefCell<Option<TrimPreviewState>>>,
}

impl gpui::Render for TrimPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let _ = &self.data;
        div().w(px(0.0)).h(px(0.0))
    }
}

impl TrimDrag {
    pub(super) fn viewport(&self) -> Viewport {
        self.scroll.get().viewport.unwrap_or(Viewport {
            start: self.viewport_start,
            length: self.viewport_length,
        })
    }
}

impl MidiTrimDrag {
    pub(super) fn viewport(&self) -> Viewport {
        self.scroll.get().viewport.unwrap_or(Viewport {
            start: self.viewport_start,
            length: self.viewport_length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TrimDrag, corrected_edge_frame, edge_frame, frame_at_x, handle_width, payload_is_current,
        preview_is_noop, rendered_clip_width, replace_audio_trim_preview,
        replace_midi_trim_preview, trim_drag_admitted,
    };
    use crate::editor::{drag::LaneBounds, viewport::Viewport};
    use hex_project::{Clip, Command, Created, Editor, Project, TrackId, TrimEdge};
    use hex_session::service::ViewState;
    use std::{
        cell::{Cell, RefCell},
        collections::BTreeMap,
        rc::Rc,
        sync::Arc,
    };

    #[test]
    fn pointer_mapping_preserves_handle_offset_and_clamps_before_frame_zero() {
        let lane = LaneBounds {
            left: 10.0,
            width: 100.0,
            top: 0.0,
            height: 44.0,
        };
        let viewport = Viewport {
            start: 100,
            length: 1_000,
        };
        let pointer = frame_at_x(30.0, lane, viewport).expect("pointer frame");
        let delta = i128::from(pointer) - 250;
        assert_eq!(corrected_edge_frame(pointer, delta), 250);
        assert_eq!(corrected_edge_frame(0, 25), 0);
        assert_eq!(corrected_edge_frame(u64::MAX, -1), u64::MAX);
        assert_eq!(frame_at_x(f32::NAN, lane, viewport), None);
    }

    #[test]
    fn edge_frame_uses_inclusive_left_and_exclusive_right_bounds() {
        let clip = test_clip();
        assert_eq!(edge_frame(&clip, TrimEdge::Left), Some(100));
        assert_eq!(edge_frame(&clip, TrimEdge::Right), Some(200));
        let unchanged = clip
            .preview_edge_trim(TrimEdge::Right, 200, 300)
            .expect("unchanged preview");
        assert!(preview_is_noop(&clip, &unchanged));
    }

    #[test]
    fn identical_audio_and_midi_trim_candidates_do_not_request_redraws() {
        let clip = test_clip();
        let audio = RefCell::new(None);
        let candidate = super::TrimPreviewState { clip: clip.clone() };
        assert!(replace_audio_trim_preview(&audio, Some(candidate.clone())));
        assert!(!replace_audio_trim_preview(&audio, Some(candidate)));
        assert!(replace_audio_trim_preview(&audio, None));
        assert!(!replace_audio_trim_preview(&audio, None));

        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "midi".into(),
            })
            .expect("track")
        else {
            panic!("track result")
        };
        let Created::Clip(clip_id) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::ONE,
            })
            .expect("midi clip")
        else {
            panic!("clip result")
        };
        let midi_clip = editor.project().midi_clip(clip_id).expect("midi clip");
        let midi_candidate = midi_clip
            .preview_edge_trim(TrimEdge::Right, hex_project::BeatTime::ONE)
            .expect("midi trim candidate");
        let midi = RefCell::new(None);
        assert!(replace_midi_trim_preview(&midi, Some(midi_candidate)));
        assert!(!replace_midi_trim_preview(&midi, Some(midi_candidate)));
    }

    #[test]
    fn resize_hit_width_stays_inside_clip_and_move_body_remains_available_at_small_sizes() {
        for (clip_width, expected) in [(3.0, 1.0), (10.0, 4.0), (20.0, 4.0), (240.0, 4.0)] {
            let left = handle_width(clip_width);
            let right = handle_width(clip_width);
            assert_eq!(left, expected);
            assert_eq!(right, expected);
            assert!(left + right <= clip_width);
            // Full-height edges must leave the central body available for moves.
            assert!(clip_width - left - right >= 1.0);
        }
        let narrow_audio = rendered_clip_width(720.0, 0.0005, 3.0);
        let narrow_midi = rendered_clip_width(720.0, 0.0005, 4.0);
        assert_eq!(narrow_audio, 3.0);
        assert_eq!(handle_width(narrow_audio), 1.0);
        assert_eq!(narrow_midi, 4.0);
        assert_eq!(handle_width(narrow_midi), 1.5);
        assert_eq!(handle_width(f32::NAN), 0.0);
        assert_eq!(handle_width(0.0), 0.0);
    }

    #[test]
    fn trim_admission_waits_for_open_close_and_canonical_work() {
        assert!(trim_drag_admitted(false, false, 0, 0));
        assert!(!trim_drag_admitted(true, false, 0, 0));
        assert!(!trim_drag_admitted(false, true, 0, 0));
        assert!(!trim_drag_admitted(false, false, 1, 0));
        assert!(!trim_drag_admitted(false, false, 0, 1));
    }

    #[test]
    fn trim_payload_rejects_stale_generation_revision_viewport_owner_and_asset() {
        let (mut editor, state, track, clip_id) = state_fixture(300);
        let clip = state.project.clip(clip_id).expect("clip").clone();
        let payload = trim_drag(track, clip, 300);
        let viewport = Viewport {
            start: 0,
            length: 500,
        };
        assert!(payload_is_current(
            &payload,
            4,
            &state,
            viewport,
            Some(payload.source_lane)
        ));
        assert!(!payload_is_current(
            &payload,
            5,
            &state,
            viewport,
            Some(payload.source_lane)
        ));

        let stale_revision = ViewState {
            canonical_audio_source: None,
            project: Arc::clone(&state.project),
            waveforms: BTreeMap::new(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::clone(&state.midi_preview),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: false,
            can_redo: false,
            revision: state.revision + 1,
            unsaved: false,
            document_path: None,
        };
        assert!(!payload_is_current(
            &payload,
            4,
            &stale_revision,
            viewport,
            Some(payload.source_lane)
        ));
        let shifted_viewport = Viewport {
            start: 1,
            length: 500,
        };
        assert!(!payload_is_current(
            &payload,
            4,
            &state,
            shifted_viewport,
            Some(payload.source_lane)
        ));
        assert!(!payload_is_current(&payload, 4, &state, viewport, None));
        let changed_lane = LaneBounds {
            left: payload.source_lane.left,
            width: payload.source_lane.width + 1.0,
            top: payload.source_lane.top,
            height: payload.source_lane.height,
        };
        assert!(!payload_is_current(
            &payload,
            4,
            &state,
            viewport,
            Some(changed_lane)
        ));

        let (_, wrong_asset, _, _) = state_fixture(301);
        assert!(!payload_is_current(
            &payload,
            4,
            &wrong_asset,
            viewport,
            Some(payload.source_lane)
        ));

        editor
            .execute(Command::RemoveClip(clip_id))
            .expect("remove clip");
        let removed = ViewState {
            canonical_audio_source: None,
            project: Arc::new(editor.project().clone()),
            waveforms: BTreeMap::new(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: 500,
            can_undo: false,
            can_redo: false,
            revision: state.revision,
            unsaved: false,
            document_path: None,
        };
        assert!(!payload_is_current(
            &payload,
            4,
            &removed,
            viewport,
            Some(payload.source_lane)
        ));

        let (mut moved_editor, moved_state, old_owner, moved_clip) = state_fixture(300);
        let Created::Track(new_owner) = moved_editor
            .execute(Command::AddTrack {
                name: "new owner".into(),
            })
            .expect("new owner")
        else {
            panic!("track result")
        };
        moved_editor
            .execute(Command::MoveClip {
                clip: moved_clip,
                track: new_owner,
                start: 300,
            })
            .expect("move clip");
        let moved_payload = trim_drag(
            old_owner,
            moved_state
                .project
                .clip(moved_clip)
                .expect("old clip")
                .clone(),
            300,
        );
        let moved = ViewState {
            canonical_audio_source: None,
            project: Arc::new(moved_editor.project().clone()),
            waveforms: BTreeMap::new(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: 500,
            can_undo: false,
            can_redo: false,
            revision: moved_state.revision,
            unsaved: false,
            document_path: None,
        };
        assert!(!payload_is_current(
            &moved_payload,
            4,
            &moved,
            viewport,
            Some(moved_payload.source_lane)
        ));
    }

    fn state_fixture(frames: u64) -> (Editor, ViewState, TrackId, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "track".into(),
                path: "trim.wav".into(),
                sample_rate: 48_000,
                frames,
            })
            .expect("import")
        else {
            panic!("import result")
        };
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 100,
                duration: 100,
                source_offset: 20,
                source_length: 100,
                looping: false,
            })
            .expect("place clip")
        else {
            panic!("clip result")
        };
        let state = ViewState {
            canonical_audio_source: None,
            project: Arc::new(editor.project().clone()),
            waveforms: BTreeMap::new(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: 500,
            can_undo: false,
            can_redo: false,
            revision: 7,
            unsaved: false,
            document_path: None,
        };
        (editor, state, track, id)
    }

    fn trim_drag(track: TrackId, clip: Clip, asset_playback_frames: u64) -> TrimDrag {
        TrimDrag {
            canceled: Rc::new(Cell::new(false)),
            stretch_modifier: Rc::new(Cell::new(false)),
            project_generation: 4,
            revision: 7,
            source_track: track,
            clip,
            asset_playback_frames,
            edge: TrimEdge::Left,
            source_lane: LaneBounds {
                left: 10.0,
                width: 100.0,
                top: 0.0,
                height: 44.0,
            },
            viewport_start: 0,
            viewport_length: 500,
            scroll: Default::default(),
            snap_grid: super::super::musical_time::SnapGrid::Off,
            snap_modifier: Rc::new(Cell::new(false)),
            last_pointer_x: Rc::new(Cell::new(None)),
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
        }
    }

    fn test_clip() -> Clip {
        let (_, state, _, id) = state_fixture(300);
        state.project.clip(id).expect("clip").clone()
    }
}
