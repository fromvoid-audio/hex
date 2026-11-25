//! Pointer projection, snap math, and current-state checks for clip trim drags.
use super::*;

/// Use only the canonical model's applied portion of one timer step. Ordinary
/// unconstrained movement stays constant-work. A source/minimum clamp finds
/// its first saturating point; an invalid full step finds its last valid point.
/// Each search halves a u64 interval: at most 64 metadata-only iterations, never
/// per-frame retries, DSP work, grid quantization or relaxed model bounds.
pub(in crate::editor) fn resize_pan_prefix<T: PartialEq>(
    current: Viewport,
    next: Viewport,
    candidate: impl Fn(Viewport) -> Option<(T, bool)>,
) -> Option<Viewport> {
    if current.length != next.length || current.start == next.start {
        return None;
    }
    let before = candidate(current)?.0;
    let distance = current.start.abs_diff(next.start);
    let viewport_at = |distance| Viewport {
        start: if next.start > current.start {
            current.start + distance
        } else {
            current.start - distance
        },
        length: current.length,
    };
    if let Some((after, unclamped)) = candidate(next) {
        if after == before {
            return None;
        }
        if unclamped {
            return Some(next);
        }
        let mut earlier = 0;
        let mut saturated = distance;
        while saturated - earlier > 1 {
            let middle = earlier + (saturated - earlier) / 2;
            if candidate(viewport_at(middle))
                .as_ref()
                .is_some_and(|(clip, _)| clip == &after)
            {
                saturated = middle;
            } else {
                earlier = middle;
            }
        }
        return Some(viewport_at(saturated));
    }
    let mut valid = 0;
    let mut invalid = distance;
    while invalid - valid > 1 {
        let middle = valid + (invalid - valid) / 2;
        if candidate(viewport_at(middle)).is_some() {
            valid = middle;
        } else {
            invalid = middle;
        }
    }
    let last = viewport_at(valid);
    candidate(last)
        .filter(|(after, _)| *after != before)
        .map(|_| last)
}

pub(in crate::editor) fn drag_admitted(view: &EditorView) -> bool {
    view.pending_trim.is_none()
        && trim_drag_admitted(
            view.persistence.opening_project,
            view.persistence.close_prompt_open,
            view.worker_requests_pending,
            view.pending_requests.len(),
        )
}

pub(in crate::editor) fn payload_is_current(
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

pub(in crate::editor) fn midi_payload_is_current(
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

/// Height of a clip's edge-trim strips, from the top of the clip: the header
/// rail, or the whole row when a header-only row has no body below it.
pub(in crate::editor) fn edge_trim_height(row_height: f32) -> f32 {
    if row_height <= super::super::track_layout::HEADER_ONLY_HEIGHT {
        row_height.max(0.0)
    } else {
        super::super::time_selection::header_hit_height(row_height)
    }
}

pub(in crate::editor) fn pointer_in_lane_x(x: f32, lane: LaneBounds) -> bool {
    x.is_finite()
        && lane.left.is_finite()
        && lane.width.is_finite()
        && lane.width > 0.0
        && x >= lane.left
        && x <= lane.left + lane.width
}

pub(in crate::editor) fn retained_timeline_pointer(
    actual: Option<f32>,
    valid: Option<f32>,
    has_preview: bool,
    lane: LaneBounds,
) -> Option<f32> {
    if let Some(x) = actual.filter(|x| pointer_in_lane_x(*x, lane)) {
        Some(x)
    } else {
        valid.filter(|_| has_preview)
    }
}

pub(in crate::editor) fn snapped_edge_frame(
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

pub(in crate::editor) fn preview_for_pointer(
    drag: &TrimDrag,
    project: &hex_project::Project,
    pointer_x: f32,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<TrimPreviewState> {
    let frame = snapped_edge_frame(drag, project, pointer_x, lane, viewport)?;
    resolve_preview(drag, project, frame)
}

/// The right edge of a repeating audio clip extends by whole repeats, like the
/// outer loop corner. A plain trim cannot grow a warped clip past its retained
/// audio, so routing it as a trim would refuse every extension.
pub(in crate::editor) fn effective_loop_kind(drag: &TrimDrag) -> Option<LoopHandleKind> {
    if drag.repeat.get() {
        return Some(LoopHandleKind::Outer);
    }
    drag.loop_kind.or_else(|| {
        (drag.edge == TrimEdge::Right
            && !drag.stretch_modifier.get()
            && loop_handles::audio_repeats(&drag.clip))
        .then_some(LoopHandleKind::Outer)
    })
}

/// The held preview a refused pointer position falls back to, if it was made
/// in the current trim/stretch mode.
pub(in crate::editor) fn retained_preview(drag: &TrimDrag) -> Option<TrimPreviewState> {
    drag.preview
        .try_borrow()
        .ok()?
        .clone()
        .filter(|preview| preview.stretch == drag.stretch_modifier.get())
}

pub(in crate::editor) fn resolve_preview(
    drag: &TrimDrag,
    project: &Project,
    frame: u64,
) -> Option<TrimPreviewState> {
    if effective_loop_kind(drag).is_some() {
        Some(TrimPreviewState {
            clip: preview_clip(drag, project, frame).ok()?,
            command_frame: frame,
            stretch: false,
        })
    } else {
        stretch_preview::resolve(drag, project, frame)
    }
}

pub(in crate::editor) fn audio_grab_frame(drag: &TrimDrag, project: &Project) -> Option<u64> {
    if drag.loop_kind == Some(LoopHandleKind::FirstBoundary) {
        u64::try_from(
            hex_project::AudioViewProjection::new(&drag.clip, project.tempo_map())
                .ok()?
                .logical_first_boundary(),
        )
        .ok()
    } else {
        edge_frame(&drag.clip, drag.edge)
    }
}

pub(in crate::editor) fn midi_grab_beat(drag: &MidiTrimDrag) -> Option<BeatTime> {
    if drag.loop_kind == Some(LoopHandleKind::FirstBoundary) {
        drag.clip.first_repeat_boundary()
    } else {
        midi_edge_beat(&drag.clip, drag.edge)
    }
}

pub(in crate::editor) fn preview_clip(
    drag: &TrimDrag,
    project: &Project,
    frame: u64,
) -> Result<Clip, hex_project::Invalid> {
    if let Some(kind) = effective_loop_kind(drag) {
        hex_project::preview_audio_view_resize(
            &drag.clip,
            project.tempo_map(),
            drag.asset_playback_frames,
            kind.audio_operation(frame),
        )
    } else if drag.stretch_modifier.get() {
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

pub(in crate::editor) fn midi_edge_beat(clip: &MidiClip, edge: TrimEdge) -> Option<BeatTime> {
    match edge {
        TrimEdge::Left => Some(clip.start()),
        TrimEdge::Right => clip
            .start()
            .raw()
            .checked_add(clip.length().raw())
            .map(BeatTime::from_raw),
    }
}

pub(in crate::editor) fn midi_pointer_beat(
    project: &Project,
    pointer_x: f32,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<BeatTime> {
    let frame = frame_at_x(pointer_x, lane, viewport)?;
    project.tempo_map().frame_to_beat(frame).ok()
}

pub(in crate::editor) fn snapped_midi_edge_beat(
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

pub(in crate::editor) fn midi_preview_for_pointer(
    drag: &MidiTrimDrag,
    project: &Project,
    pointer_x: f32,
    lane: LaneBounds,
    viewport: Viewport,
) -> Option<MidiEdgePreview> {
    midi_edge_preview(
        drag,
        snapped_midi_edge_beat(drag, project, pointer_x, lane, viewport)?,
    )
}
