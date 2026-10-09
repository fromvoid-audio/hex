//! Native pointer trim gesture state and bounded canonical dispatch.
use super::{EditorView, ViewState, drag::LaneBounds, viewport::Viewport};
use crate::theme::Palette;
use gpui::{Context, CursorStyle, Div, MouseButton, Stateful, Window, div, prelude::*, px, rgb};
use hex_project::{Clip, Command, TrackId, TrimEdge};
use hex_session::service::Request;
use std::{cell::RefCell, rc::Rc};

const HANDLE_WIDTH: f32 = 8.0;
const MIN_CLIP_WIDTH_FOR_HANDLES: f32 = 20.0;

#[derive(Clone, Debug)]
pub(super) struct TrimPreviewState {
    pub(super) clip: Clip,
    edge: TrimEdge,
}

/// Bounded metadata-only payload; the model helper builds previews without project or PCM copies.
#[derive(Clone, Debug)]
pub(super) struct TrimDrag {
    project_generation: u64,
    revision: u64,
    pub(super) source_track: TrackId,
    clip: Clip,
    asset_playback_frames: u64,
    edge: TrimEdge,
    source_lane: LaneBounds,
    viewport_start: u64,
    viewport_length: u64,
    grab_delta: Rc<RefCell<Option<i128>>>,
    preview: Rc<RefCell<Option<TrimPreviewState>>>,
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
    drag.project_generation == generation
        && drag.revision == state.revision
        && drag.viewport_start == viewport.start
        && drag.viewport_length == viewport.length
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

fn preview_is_noop(original: &Clip, preview: &Clip) -> bool {
    original == preview
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
        if !drag_admitted(self) || clip_width < MIN_CLIP_WIDTH_FOR_HANDLES {
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
                viewport,
                asset.playback_frames(),
                cx,
            ));
        }
        handles
    }

    #[allow(clippy::too_many_arguments)]
    fn trim_handle(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &Clip,
        edge: TrimEdge,
        lane: LaneBounds,
        viewport: Viewport,
        asset_playback_frames: u64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let clip_id = clip.id();
        let payload = TrimDrag {
            project_generation: self.project_generation,
            revision: state.revision,
            source_track: track,
            clip: clip.clone(),
            asset_playback_frames,
            edge,
            source_lane: lane,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
        };
        let preview_entity = cx.entity();
        let drag_entity = cx.entity();
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
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(HANDLE_WIDTH))
            .when(edge == TrimEdge::Left, |this| this.left_0())
            .when(edge == TrimEdge::Right, |this| this.right_0())
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(rgb(Palette::CORAL))
            .opacity(0.85)
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                mouse_down_entity.update(cx, |view, _| {
                    if view.trim_drag_current(&pointer_payload) {
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
            .on_drag(payload, move |payload, _cursor_offset, _, cx| {
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
                if let Some(pointer) = pointer_frame
                    && let Some(delta) = delta
                {
                    update_preview(payload, corrected_edge_frame(pointer, delta));
                }
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
                    *drag.preview.borrow_mut() = None;
                    preview_entity.update(cx, |_, cx| cx.notify());
                    return;
                }
                let Some(pointer) = frame_at_x(f32::from(event.event.position.x), lane, viewport)
                else {
                    return;
                };
                let Some(delta) = *drag.grab_delta.borrow() else {
                    return;
                };
                let frame = corrected_edge_frame(pointer, delta);
                update_preview(drag, frame);
                preview_entity.update(cx, |_, cx| cx.notify());
            })
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
        let Some(pointer) = frame_at_x(f32::from(window.mouse_position().x), lane, viewport) else {
            return;
        };
        let Some(delta) = *drag.grab_delta.borrow() else {
            return;
        };
        let frame = corrected_edge_frame(pointer, delta);
        let Ok(preview) = drag
            .clip
            .preview_edge_trim(drag.edge, frame, drag.asset_playback_frames)
        else {
            return;
        };
        if preview_is_noop(&drag.clip, &preview) {
            return;
        }
        self.enqueue(Request::Edit(Command::TrimClipEdge {
            clip: drag.clip.id(),
            edge: drag.edge,
            frame,
        }));
        cx.notify();
    }
}

fn update_preview(drag: &TrimDrag, frame: u64) {
    let preview = drag
        .clip
        .preview_edge_trim(drag.edge, frame, drag.asset_playback_frames)
        .ok()
        .map(|clip| TrimPreviewState {
            clip,
            edge: drag.edge,
        });
    *drag.preview.borrow_mut() = preview;
}

struct TrimPreview {
    data: Rc<RefCell<Option<TrimPreviewState>>>,
}

impl gpui::Render for TrimPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let bounds = self.data.borrow().as_ref().map(|preview| {
            let start = preview.clip.start();
            let end = start.saturating_add(preview.clip.duration());
            let edge = match preview.edge {
                TrimEdge::Left => "←",
                TrimEdge::Right => "→",
            };
            format!("{edge} {start}–{end}f")
        });
        div()
            .px_1()
            .py_0p5()
            .border_1()
            .border_color(rgb(if bounds.is_some() {
                Palette::CYAN
            } else {
                Palette::CORAL
            }))
            .bg(rgb(Palette::SURFACE))
            .text_xs()
            .text_color(rgb(Palette::TEXT))
            .child(crate::theme::text(
                bounds.unwrap_or_else(|| "trim unavailable".into()),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TrimDrag, corrected_edge_frame, edge_frame, frame_at_x, payload_is_current,
        preview_is_noop, trim_drag_admitted,
    };
    use crate::editor::{drag::LaneBounds, viewport::Viewport};
    use hex_project::{Clip, Command, Created, Editor, Project, TrackId, TrimEdge};
    use hex_session::service::ViewState;
    use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};

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
            project: Arc::clone(&state.project),
            waveforms: BTreeMap::new(),
            midi_preview: Arc::clone(&state.midi_preview),
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
            project: Arc::new(editor.project().clone()),
            waveforms: BTreeMap::new(),
            midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
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
            project: Arc::new(moved_editor.project().clone()),
            waveforms: BTreeMap::new(),
            midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
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
            project: Arc::new(editor.project().clone()),
            waveforms: BTreeMap::new(),
            midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
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
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
        }
    }

    fn test_clip() -> Clip {
        let (_, state, _, id) = state_fixture(300);
        state.project.clip(id).expect("clip").clone()
    }
}
