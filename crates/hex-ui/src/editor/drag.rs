use super::musical_time::SnapGrid;
use gpui::{Context, Window};
use hex_project::Command;
use hex_project::{ClipId, Project, TrackId, TrimEdge};
use hex_session::service::Request;
use std::cell::Cell;
use std::rc::Rc;

/// Bounded, metadata-only payload for one native pointer drag.
#[derive(Clone, Debug)]
pub(super) struct ClipDrag {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) source_track: TrackId,
    pub(super) clip: ClipId,
    pub(super) clip_start: u64,
    pub(super) clip_duration: u64,
    pub(super) grab_offset: Rc<Cell<Option<u64>>>,
    pub(super) viewport_start: u64,
    pub(super) viewport_length: u64,
    pub(super) snap_grid: SnapGrid,
    pub(super) preview_start: Rc<Cell<Option<u64>>>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LaneBounds {
    pub(super) left: f32,
    pub(super) width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PointerDown {
    pub(super) generation: u64,
    pub(super) clip: ClipId,
    pub(super) edge: Option<TrimEdge>,
    pub(super) x: f32,
}

fn take_pointer_down(
    pending: &mut Option<PointerDown>,
    generation: u64,
    clip: ClipId,
    edge: Option<TrimEdge>,
) -> Option<f32> {
    let matches = pending.is_some_and(|down| {
        down.generation == generation && down.clip == clip && down.edge == edge
    });
    if matches {
        pending.take().map(|down| down.x)
    } else {
        None
    }
}

/// Convert the pointer's position in the visible part of a clip to its offset
/// from the clip's true start, including any portion cropped by the viewport.
pub(super) fn grab_offset(
    local_x: f32,
    element_width: f32,
    visible_start: u64,
    visible_end: u64,
    clip_start: u64,
) -> Option<u64> {
    let visible_frames = visible_end.checked_sub(visible_start)?;
    if visible_frames == 0 || !local_x.is_finite() {
        return None;
    }
    if !element_width.is_finite() || element_width <= 0.0 {
        return None;
    }
    let fraction = (f64::from(local_x) / f64::from(element_width)).clamp(0.0, 1.0);
    let within_visible = (fraction * visible_frames as f64).floor() as u64;
    visible_start
        .checked_add(within_visible.min(visible_frames))?
        .checked_sub(clip_start)
}

/// Map the original mouse-down point, not GPUI's later threshold-crossing point, into clip frames.
pub(super) fn grab_offset_from_pointer(
    mouse_down_x: f32,
    element_left: f32,
    element_width: f32,
    visible_start: u64,
    visible_end: u64,
    clip_start: u64,
) -> Option<u64> {
    let local_x = f64::from(mouse_down_x) - f64::from(element_left);
    if !local_x.is_finite() || local_x.abs() > f64::from(f32::MAX) {
        return None;
    }
    grab_offset(
        local_x as f32,
        element_width,
        visible_start,
        visible_end,
        clip_start,
    )
}

/// Resolve the drop point to a project frame and clamp a dragged clip at zero.
pub(super) fn drop_start(
    pointer_x: f32,
    lane: LaneBounds,
    viewport_start: u64,
    viewport_length: u64,
    grab_offset: u64,
) -> Option<u64> {
    if !pointer_x.is_finite()
        || !lane.left.is_finite()
        || !lane.width.is_finite()
        || lane.width <= 0.0
        || viewport_length == 0
    {
        return None;
    }
    let fraction =
        ((f64::from(pointer_x) - f64::from(lane.left)) / f64::from(lane.width)).clamp(0.0, 1.0);
    let within_view = (fraction * viewport_length as f64).floor() as u64;
    let pointer_frame = viewport_start.checked_add(within_view.min(viewport_length))?;
    Some(pointer_frame.saturating_sub(grab_offset))
}

fn candidate_start(
    project: &Project,
    drag: &ClipDrag,
    pointer_x: f32,
    lane: LaneBounds,
) -> Option<u64> {
    let pointer_frame = drop_start(
        pointer_x,
        lane,
        drag.viewport_start,
        drag.viewport_length,
        drag.grab_offset.get()?,
    )?;
    drag.snap_grid.snap_frame(project, pointer_frame)
}

#[derive(Clone, Copy)]
pub(super) struct DropState<'a> {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) project: &'a Project,
    pub(super) target_track: TrackId,
    pub(super) start: u64,
    pub(super) viewport_start: u64,
    pub(super) viewport_length: u64,
    pub(super) snap_grid: SnapGrid,
}

pub(super) fn is_current(drag: &ClipDrag, state: DropState<'_>) -> bool {
    if drag.project_generation != state.project_generation
        || drag.revision != state.revision
        || drag.viewport_start != state.viewport_start
        || drag.viewport_length != state.viewport_length
        || drag.snap_grid != state.snap_grid
    {
        return false;
    }
    if state.project.track(state.target_track).is_none() {
        return false;
    }
    state
        .project
        .track(drag.source_track)
        .and_then(|track| track.clips().iter().find(|clip| clip.id() == drag.clip))
        .is_some_and(|clip| {
            clip.start() == drag.clip_start && clip.duration() == drag.clip_duration
        })
}

pub(super) fn is_noop(drag: &ClipDrag, state: &DropState<'_>) -> bool {
    drag.source_track == state.target_track && drag.clip_start == state.start
}

impl super::EditorView {
    pub(super) fn remember_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
        x: f32,
    ) {
        self.pointer_down = x.is_finite().then_some(PointerDown {
            generation,
            clip,
            edge,
            x,
        });
    }

    pub(super) fn take_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
    ) -> Option<f32> {
        take_pointer_down(&mut self.pointer_down, generation, clip, edge)
    }

    pub(super) fn clear_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
    ) {
        if self.pointer_down.is_some_and(|down| {
            down.generation == generation && down.clip == clip && down.edge == edge
        }) {
            self.pointer_down = None;
        }
    }

    pub(super) fn drag_start_is_current(&self, drag: &ClipDrag) -> bool {
        if self.opening_project
            || self.close_prompt_open
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        is_current(
            drag,
            DropState {
                project_generation: self.project_generation,
                revision: state.revision,
                project: &state.project,
                target_track: drag.source_track,
                start: drag.clip_start,
                viewport_start: viewport.start,
                viewport_length: viewport.length,
                snap_grid: self.snap_grid,
            },
        )
    }

    pub(super) fn drag_candidate_start(
        &self,
        drag: &ClipDrag,
        pointer_x: f32,
        lane: LaneBounds,
    ) -> Option<u64> {
        if !self.drag_start_is_current(drag) {
            return None;
        }
        let state = self.state.as_ref()?;
        candidate_start(&state.project, drag, pointer_x, lane)
    }

    pub(super) fn drop_clip(
        &mut self,
        drag: &ClipDrag,
        target_track: TrackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_project || self.close_prompt_open {
            return;
        }
        if self.worker_requests_pending != 0 || !self.pending_requests.is_empty() {
            self.error = Some("clip move canceled · project work pending".into());
            cx.notify();
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        if drag.grab_offset.get().is_none() {
            self.error = Some("clip move canceled · pointer position unavailable".into());
            cx.notify();
            return;
        }
        let Some(lane) = self.drag_lanes.get(&target_track).copied() else {
            self.error = Some("clip move canceled · target track unavailable".into());
            cx.notify();
            return;
        };
        let Some(start) =
            self.drag_candidate_start(drag, f32::from(window.mouse_position().x), lane)
        else {
            self.error = Some("clip move canceled · invalid timeline position".into());
            cx.notify();
            return;
        };
        if start.checked_add(drag.clip_duration).is_none() {
            self.error = Some("clip move canceled · timeline position overflows".into());
            cx.notify();
            return;
        }
        let drop_state = DropState {
            project_generation: self.project_generation,
            revision: state.revision,
            project: &state.project,
            target_track,
            start,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            snap_grid: self.snap_grid,
        };
        if !is_current(drag, drop_state) {
            self.error = Some("clip move canceled · project changed".into());
            cx.notify();
            return;
        }
        if is_noop(drag, &drop_state) {
            return;
        }
        self.enqueue(Request::Edit(Command::MoveClip {
            clip: drag.clip,
            track: target_track,
            start,
        }));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClipDrag, DropState, LaneBounds, PointerDown, SnapGrid, candidate_start, drop_start,
        grab_offset, grab_offset_from_pointer, is_current, is_noop, take_pointer_down,
    };
    use hex_project::{Command, Created, Editor, Project, TrackId, TrimEdge};
    use std::{cell::Cell, rc::Rc};

    fn fixture() -> (Editor, TrackId, TrackId, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
        let Created::Track(source) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .expect("source track")
        else {
            panic!("source track id");
        };
        let Created::Track(target) = editor
            .execute(Command::AddTrack {
                name: "target".into(),
            })
            .expect("target track")
        else {
            panic!("target track id");
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "Media/Original.WAV".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .expect("asset")
        else {
            panic!("asset id");
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceClip {
                track: source,
                asset,
                start: 40,
                duration: 100,
                source_offset: 5,
                source_length: 80,
                looping: true,
            })
            .expect("clip")
        else {
            panic!("clip id");
        };
        (editor, source, target, clip)
    }

    fn drag(source: TrackId, clip: hex_project::ClipId) -> ClipDrag {
        ClipDrag {
            project_generation: 3,
            revision: 9,
            source_track: source,
            clip,
            clip_start: 40,
            clip_duration: 100,
            grab_offset: Rc::new(Cell::new(Some(50))),
            viewport_start: 0,
            viewport_length: 1_000,
            snap_grid: SnapGrid::Off,
            preview_start: Rc::new(Cell::new(None)),
        }
    }

    fn drop_state<'a>(project: &'a Project, target_track: TrackId, start: u64) -> DropState<'a> {
        DropState {
            project_generation: 3,
            revision: 9,
            project,
            target_track,
            start,
            viewport_start: 0,
            viewport_length: 1_000,
            snap_grid: SnapGrid::Off,
        }
    }

    #[test]
    fn pointer_math_preserves_cropped_grab_offset_and_clamps_before_zero() {
        // The cropped leading 80 frames plus half of the visible 100 frames
        // place the grab point 130 frames from the clip's true start.
        assert_eq!(grab_offset(25.0, 50.0, 120, 220, 40), Some(130));
        let lane = LaneBounds {
            left: 10.0,
            width: 100.0,
        };
        assert_eq!(drop_start(10.0, lane, 0, 200, 15), Some(0));
        assert_eq!(drop_start(60.0, lane, 0, 200, 15), Some(85));
        assert_eq!(
            drop_start(
                f32::MAX,
                LaneBounds {
                    left: -f32::MAX,
                    width: 1.0
                },
                0,
                200,
                0
            ),
            Some(200)
        );
    }

    #[test]
    fn move_grab_offset_uses_mouse_down_instead_of_threshold_crossing_position() {
        let (_, _, _, clip) = fixture();
        let mut pending = Some(PointerDown {
            generation: 3,
            clip,
            edge: None,
            x: 790.0,
        });
        let threshold_crossing_x = 725.0;
        let captured_down_x =
            take_pointer_down(&mut pending, 3, clip, None).expect("captured original mouse-down");
        assert_eq!(threshold_crossing_x, 725.0);
        let down_offset = grab_offset_from_pointer(captured_down_x, 700.0, 100.0, 0, 100, 10)
            .expect("original mouse-down frame offset");
        let threshold_offset =
            grab_offset_from_pointer(threshold_crossing_x, 700.0, 100.0, 0, 100, 10)
                .expect("threshold-crossing offset");
        assert_eq!(down_offset, 80);
        assert_eq!(threshold_offset, 15);
        assert!(pending.is_none());
        assert_eq!(
            drop_start(
                660.0,
                LaneBounds {
                    left: 600.0,
                    width: 100.0,
                },
                0,
                200,
                down_offset,
            ),
            Some(40)
        );
    }

    #[test]
    fn pointer_down_is_keyed_to_trim_edge_and_survives_render_replacement() {
        let (_, _, _, clip) = fixture();
        // The pending gesture lives on EditorView; replacing element callbacks on a
        // redraw does not replace this state. A trim handle consumes only its edge.
        let mut pending = Some(PointerDown {
            generation: 3,
            clip,
            edge: Some(TrimEdge::Right),
            x: 790.0,
        });
        assert_eq!(
            take_pointer_down(&mut pending, 3, clip, Some(TrimEdge::Left)),
            None
        );
        assert_eq!(
            take_pointer_down(&mut pending, 3, clip, Some(TrimEdge::Right)),
            Some(790.0)
        );
        assert!(pending.is_none());
    }

    #[test]
    fn pointer_math_rejects_invalid_geometry_and_overflow() {
        assert_eq!(grab_offset(1.0, 0.0, 0, 2, 0), None);
        assert_eq!(grab_offset(f32::NAN, 100.0, 0, 2, 0), None);
        assert_eq!(
            drop_start(
                1.0,
                LaneBounds {
                    left: 0.0,
                    width: 1.0,
                },
                u64::MAX,
                2,
                0,
            ),
            None
        );
    }

    #[test]
    fn move_preview_and_release_share_the_same_snapped_candidate_resolver() {
        let (editor, source, _, clip) = fixture();
        let mut payload = drag(source, clip);
        payload.viewport_length = 48_000;
        payload.grab_offset.set(Some(0));
        payload.snap_grid = SnapGrid::Quarter;
        let lane = LaneBounds {
            left: 0.0,
            width: 480.0,
        };
        // 150 px maps to frame 15,000 (0.625 quarter at 120 bpm); the
        // canonical quarter grid maps that candidate to 24,000.
        let preview = candidate_start(editor.project(), &payload, 150.0, lane);
        let release = candidate_start(editor.project(), &payload, 150.0, lane);
        assert_eq!(preview, Some(24_000));
        assert_eq!(release, preview);
        assert_eq!(
            preview.and_then(|start| start.checked_add(payload.clip_duration)),
            Some(24_100)
        );

        payload.snap_grid = SnapGrid::Off;
        assert_eq!(
            candidate_start(editor.project(), &payload, 150.0, lane),
            Some(15_000)
        );
        payload.snap_grid = SnapGrid::Quarter;
        payload.viewport_start = u64::MAX;
        assert_eq!(
            candidate_start(editor.project(), &payload, 480.0, lane),
            None
        );
    }

    #[test]
    fn stale_project_revision_viewport_and_missing_ids_reject_a_drop() {
        let (mut editor, source, target, clip) = fixture();
        let payload = drag(source, clip);
        let valid = drop_state(editor.project(), target, 200);
        assert!(is_current(&payload, valid));

        let mut stale_generation = drop_state(editor.project(), target, 200);
        stale_generation.project_generation += 1;
        assert!(!is_current(&payload, stale_generation));

        let mut stale_revision = drop_state(editor.project(), target, 200);
        stale_revision.revision += 1;
        assert!(!is_current(&payload, stale_revision));

        let mut stale_snap = drop_state(editor.project(), target, 200);
        stale_snap.snap_grid = SnapGrid::Quarter;
        assert!(!is_current(&payload, stale_snap));

        let mut stale_viewport = drop_state(editor.project(), target, 200);
        stale_viewport.viewport_start += 1;
        assert!(!is_current(&payload, stale_viewport));

        editor
            .execute(Command::RemoveTrack(source))
            .expect("remove source");
        assert!(!is_current(
            &payload,
            drop_state(editor.project(), target, 200)
        ));

        let (mut editor, source, target, clip) = fixture();
        let payload = drag(source, clip);
        editor
            .execute(Command::RemoveClip(clip))
            .expect("remove source clip");
        assert!(!is_current(
            &payload,
            drop_state(editor.project(), target, 200)
        ));

        let (mut editor, source, target, clip) = fixture();
        let payload = drag(source, clip);
        editor
            .execute(Command::RemoveTrack(target))
            .expect("remove destination");
        assert!(!is_current(
            &payload,
            drop_state(editor.project(), target, 200)
        ));
    }

    #[test]
    fn independently_created_projects_with_colliding_ids_are_generation_scoped() {
        let (_first, first_source, first_target, first_clip) = fixture();
        let (second, second_source, second_target, second_clip) = fixture();
        assert_eq!(first_source, second_source);
        assert_eq!(first_target, second_target);
        assert_eq!(first_clip, second_clip);

        let payload = drag(first_source, first_clip);
        let mut replaced_project = drop_state(second.project(), second_target, 200);
        replaced_project.project_generation += 1;
        assert!(!is_current(&payload, replaced_project));
    }

    #[test]
    fn unchanged_drop_is_classified_as_noop_without_becoming_stale() {
        let (editor, source, _, clip) = fixture();
        let payload = drag(source, clip);
        let state = drop_state(editor.project(), source, payload.clip_start);
        assert!(is_current(&payload, state));
        assert!(is_noop(&payload, &state));
    }

    #[test]
    fn one_cross_track_move_undoes_and_redoes_with_the_same_clip_metadata() {
        let (mut editor, source, target, clip) = fixture();
        let before = editor.project().clone();
        let source_meta = {
            let clip = editor.project().clip(clip).expect("clip metadata");
            (
                clip.id(),
                clip.asset(),
                clip.duration(),
                clip.source_offset(),
                clip.source_length(),
                clip.looping(),
            )
        };
        editor
            .execute(Command::MoveClip {
                clip,
                track: target,
                start: 300,
            })
            .expect("move clip");
        let moved = editor.project().clone();
        let moved_clip = moved
            .track(target)
            .expect("target track")
            .clips()
            .first()
            .expect("moved clip");
        assert_eq!(moved_clip.start(), 300);
        assert_eq!(
            (
                moved_clip.id(),
                moved_clip.asset(),
                moved_clip.duration(),
                moved_clip.source_offset(),
                moved_clip.source_length(),
                moved_clip.looping(),
            ),
            source_meta
        );
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        assert!(editor.redo());
        assert_eq!(editor.project(), &moved);
        assert!(editor.project().track(source).unwrap().clips().is_empty());
    }
}
