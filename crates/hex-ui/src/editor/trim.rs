//! Native pointer trim gesture state and bounded canonical dispatch.
use super::{EditorView, ViewState, drag::LaneBounds, viewport::Viewport};
use gpui::{Context, Div, MouseButton, Stateful, Window, div, prelude::*, px};
use hex_project::{
    BeatTime, Clip, Command, MidiClip, MidiClipLoopResize, MidiClipStretch, MidiClipTrim,
    MidiSourcePlayback, Project, TrackId, TrimEdge,
};
use hex_session::service::Request;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[path = "clip_loop_handles.rs"]
pub(super) mod loop_handles;
#[path = "stretch_preview.rs"]
mod stretch_preview;

#[path = "trim/admission.rs"]
mod admission;
use admission::trim_drag_admitted;

#[path = "trim/geometry.rs"]
mod geometry;
use geometry::{corrected_edge_frame, edge_frame, frame_at_x, preview_is_noop};
pub(super) use geometry::{handle_width, rendered_clip_width};

#[derive(Clone, Debug)]
pub(super) struct TrimPreviewState {
    pub(super) clip: Clip,
    command_frame: u64,
    stretch: bool,
}

#[cfg(all(test, feature = "ui-test-support"))]
impl TrimPreviewState {
    pub(super) fn command_frame_for_test(&self) -> u64 {
        self.command_frame
    }
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

/// One immutable source plus metadata projection survives capture and the exact
/// worker terminal. Pointer updates never materialize a stretched note array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MidiEdgePreview {
    Trim(MidiClipTrim),
    Stretch(MidiClipStretch),
    Loop(MidiClipLoopResize),
}
impl From<MidiClipTrim> for MidiEdgePreview {
    fn from(value: MidiClipTrim) -> Self {
        Self::Trim(value)
    }
}
impl MidiEdgePreview {
    pub(super) fn start(self) -> BeatTime {
        match self {
            Self::Trim(p) => p.start(),
            Self::Stretch(p) => p.start(),
            Self::Loop(p) => p.start(),
        }
    }
    pub(super) fn length(self) -> BeatTime {
        match self {
            Self::Trim(p) => p.length(),
            Self::Stretch(p) => p.length(),
            Self::Loop(p) => p.length(),
        }
    }
    pub(super) fn source_start(self) -> BeatTime {
        match self {
            Self::Trim(p) => p.source_start(),
            Self::Stretch(p) => p.source_start(),
            Self::Loop(p) => p.source_start(),
        }
    }
    pub(super) fn playback_offset(self) -> BeatTime {
        match self {
            Self::Trim(p) => p.playback_offset(),
            Self::Stretch(p) => p.playback_offset(),
            Self::Loop(p) => p.playback_offset(),
        }
    }
    pub(super) fn note_interval(
        self,
        note: &hex_project::MidiNote,
    ) -> Option<(BeatTime, BeatTime)> {
        let start = self.source_beat(note.start())?;
        let end = self.source_beat(BeatTime::from_raw(
            note.start().raw().checked_add(note.duration().raw())?,
        ))?;
        let duration = end.raw().checked_sub(start.raw()).filter(|n| *n > 0)?;
        Some((start, BeatTime::from_raw(duration)))
    }
    /// The same endpoint projection used by the canonical stretch command.
    pub(super) fn source_beat(self, beat: BeatTime) -> Option<BeatTime> {
        match self {
            Self::Trim(p) => beat
                .raw()
                .checked_add(p.source_prefix().raw())
                .map(BeatTime::from_raw),
            Self::Stretch(p) => p.scale(beat).ok(),
            Self::Loop(_) => Some(beat),
        }
    }
    pub(super) fn looping(self, clip: &MidiClip) -> bool {
        matches!(self, Self::Loop(_)) || clip.looping()
    }
    pub(super) fn source_playback(
        self,
        clip: &MidiClip,
    ) -> Result<MidiSourcePlayback, hex_project::Invalid> {
        if let Self::Loop(p) = self {
            return p.source_playback();
        }
        let shifted = |beat| {
            self.source_beat(beat).ok_or(hex_project::Invalid(
                "midi preview source position overflow",
            ))
        };
        let loop_range = if clip.looping() {
            Some((shifted(clip.loop_start())?, shifted(clip.loop_end())?))
        } else {
            None
        };
        MidiSourcePlayback::new_with_initial_end(
            shifted(clip.source_length())?,
            self.source_start(),
            loop_range,
            self.length(),
            shifted(clip.source().effective_end())?,
        )?
        .with_playback_offset(self.playback_offset())
    }
    fn boundary(self, clip: &MidiClip) -> Option<BeatTime> {
        self.start()
            .raw()
            .checked_add(
                self.source_playback(clip)
                    .ok()?
                    .first_repeat_boundary()?
                    .raw(),
            )
            .map(BeatTime::from_raw)
    }
}
/// Option-dragging the right header edge repeat-extends; see [`TrimDrag::repeat`].
fn midi_loop_kind(drag: &MidiTrimDrag) -> Option<LoopHandleKind> {
    if drag.repeat.get() {
        Some(LoopHandleKind::Outer)
    } else {
        drag.loop_kind
    }
}

fn midi_edge_preview(drag: &MidiTrimDrag, beat: BeatTime) -> Option<MidiEdgePreview> {
    if let Some(kind) = midi_loop_kind(drag) {
        drag.clip
            .preview_loop_resize(kind.midi_operation(beat))
            .ok()
            .map(MidiEdgePreview::Loop)
    } else if drag.stretch_modifier.get() {
        let preview = drag.clip.preview_edge_stretch(drag.edge, beat).ok()?;
        // Match the bounded thumbnail budget. Full source validation remains
        // the worker command's responsibility; a sub-tick unsampled interval
        // may refuse atomically at its terminal rather than stall pointer input.
        let candidate = MidiEdgePreview::Stretch(preview);
        for note in drag
            .clip
            .notes()
            .iter()
            .take(hex_engine::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP)
        {
            candidate.note_interval(note)?;
        }
        Some(candidate)
    } else {
        drag.clip
            .preview_edge_trim(drag.edge, beat)
            .ok()
            .map(MidiEdgePreview::Trim)
    }
}

fn replace_midi_trim_preview(
    preview: &RefCell<Option<MidiEdgePreview>>,
    next: Option<MidiEdgePreview>,
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
    loop_kind: Option<LoopHandleKind>,
    /// The right header edge can repeat-extend this clip with option.
    repeat_available: bool,
    /// Option was held when this gesture started: repeat-extend for its whole
    /// duration, whatever modifiers change afterwards.
    repeat: Rc<Cell<bool>>,
    canceled: Rc<Cell<bool>>,
    capture: Rc<Cell<Option<super::drag::PointerDown>>>,
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
    // Last successfully projected timeline point; foreign panel positions only stop edge-pan.
    valid_pointer_x: Rc<Cell<Option<f32>>>,
    pub(super) grab_delta: Rc<RefCell<Option<i128>>>,
    pub(super) preview: Rc<RefCell<Option<TrimPreviewState>>>,
    resolution: Rc<RefCell<stretch_preview::Resolution>>,
}

/// Metadata-only Arrangement MIDI edge gesture. The authored notes are shared
/// through one `Rc`; pointer updates and previews never clone the note list.
#[derive(Clone, Debug)]
pub(super) struct MidiTrimDrag {
    loop_kind: Option<LoopHandleKind>,
    /// See [`TrimDrag::repeat_available`].
    repeat_available: bool,
    /// See [`TrimDrag::repeat`].
    repeat: Rc<Cell<bool>>,
    canceled: Rc<Cell<bool>>,
    capture: Rc<Cell<Option<super::drag::PointerDown>>>,
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
    pub(super) stretch_modifier: Rc<Cell<bool>>,
    pub(super) snap_modifier: Rc<Cell<bool>>,
    pub(super) last_pointer_x: Rc<Cell<Option<f32>>>,
    // Last successfully projected timeline point; foreign panel positions only stop edge-pan.
    valid_pointer_x: Rc<Cell<Option<f32>>>,
    grab_delta: Rc<RefCell<Option<i128>>>,
    pub(super) preview: Rc<RefCell<Option<MidiEdgePreview>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoopHandleKind {
    Outer,
    FirstBoundary,
}
impl LoopHandleKind {
    fn audio_operation(self, project_frame: u64) -> hex_project::AudioViewResizeOperation {
        match self {
            Self::Outer => hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame },
            Self::FirstBoundary => {
                hex_project::AudioViewResizeOperation::CycleEnd { project_frame }
            }
        }
    }
    fn midi_operation(self, beat: BeatTime) -> hex_project::MidiViewResizeOperation {
        match self {
            Self::Outer => hex_project::MidiViewResizeOperation::RepeatOuterEnd { beat },
            Self::FirstBoundary => hex_project::MidiViewResizeOperation::CycleEnd { beat },
        }
    }
}

enum PendingTrimDisplay {
    Audio {
        original: Clip,
        prospective: Box<Clip>,
        asset_frames: u64,
    },
    Midi {
        original: Rc<MidiClip>,
        prospective: MidiEdgePreview,
    },
}

impl EditorView {
    fn pending_trim_current(&self) -> Option<&PendingTrim> {
        let pending = self.pending_trim.as_ref()?;
        let state = self.state.as_ref()?;
        if pending.generation != self.project_generation || pending.revision > state.revision {
            return None;
        }
        // Independent revisions may complete ahead of this queued request.
        // Its exact stable source owner, not revision equality alone, decides
        // whether the accepted prospective geometry still belongs here.
        let track = state.project.track(pending.track)?;
        let current = match &pending.display {
            PendingTrimDisplay::Audio {
                original,
                asset_frames,
                ..
            } => {
                track.clips().iter().any(|clip| clip == original)
                    && state.project.asset(original.asset())?.playback_frames() == *asset_frames
            }
            PendingTrimDisplay::Midi { original, .. } => track
                .midi_clips()
                .iter()
                .any(|clip| clip == original.as_ref()),
        };
        current.then_some(pending)
    }

    pub(super) fn audio_trim_display(
        &self,
        track: TrackId,
        only: Option<hex_project::ClipId>,
    ) -> Option<(Clip, Clip)> {
        if let Some(drag) = self.active_trim_drag.as_ref()
            && drag.source_track == track
            && only.is_none_or(|id| id == drag.clip.id())
            && self.trim_drag_current(drag)
            && let Some(preview) = drag.preview.borrow().as_ref()
        {
            return Some((drag.clip.clone(), preview.clip.clone()));
        }
        let pending = self.pending_trim.as_ref()?;
        if pending.track != track {
            return None;
        }
        let PendingTrimDisplay::Audio {
            original,
            prospective,
            ..
        } = &pending.display
        else {
            return None;
        };
        if only.is_some_and(|id| id != original.id()) {
            return None;
        }
        self.pending_trim_current()?;
        Some((original.clone(), prospective.as_ref().clone()))
    }

    pub(super) fn midi_trim_display(
        &self,
        track: TrackId,
        only: Option<hex_project::ClipId>,
    ) -> Option<(Rc<MidiClip>, MidiEdgePreview)> {
        if let Some(drag) = self.active_midi_trim_drag.as_ref()
            && drag.source_track == track
            && only.is_none_or(|id| id == drag.clip.id())
            && self.midi_trim_drag_current(drag)
            && let Some(preview) = *drag.preview.borrow()
        {
            return Some((Rc::clone(&drag.clip), preview));
        }
        let pending = self.pending_trim.as_ref()?;
        if pending.track != track {
            return None;
        }
        let PendingTrimDisplay::Midi {
            original,
            prospective,
        } = &pending.display
        else {
            return None;
        };
        if only.is_some_and(|id| id != original.id()) {
            return None;
        }
        self.pending_trim_current()?;
        Some((Rc::clone(original), *prospective))
    }
}

/// One resize owner: terminal core plus validated prospective display metadata.
/// No separate completion key, FIFO counter or viewport owner.
pub(super) struct PendingTrim {
    generation: u64,
    revision: u64,
    track: TrackId,
    command: ResizeCommand,
    remaining: Option<usize>,
    scroll_viewport: Option<Viewport>,
    display: PendingTrimDisplay,
}
pub(super) struct RetiredTrimViewport {
    generation: u64,
    viewport: Option<Viewport>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeCommand {
    AudioLoop {
        clip: hex_project::ClipId,
        operation: hex_project::AudioViewResizeOperation,
    },
    MidiLoop {
        clip: hex_project::ClipId,
        operation: hex_project::MidiViewResizeOperation,
    },
    Audio {
        clip: hex_project::ClipId,
        edge: TrimEdge,
        frame: u64,
        stretch: bool,
    },
    Midi {
        clip: hex_project::ClipId,
        edge: TrimEdge,
        beat: BeatTime,
        stretch: bool,
    },
}

impl ResizeCommand {
    fn from_request(request: &Request) -> Option<Self> {
        match request {
            Request::Edit(Command::ResizeAudioClipView { clip, operation }) => {
                Some(Self::AudioLoop {
                    clip: *clip,
                    operation: *operation,
                })
            }
            Request::Edit(Command::ResizeMidiClipView { clip, operation }) => {
                Some(Self::MidiLoop {
                    clip: *clip,
                    operation: *operation,
                })
            }
            Request::Edit(Command::StretchAudioClipEdge { clip, edge, frame }) => {
                Some(Self::Audio {
                    clip: *clip,
                    edge: *edge,
                    frame: *frame,
                    stretch: true,
                })
            }
            Request::Edit(Command::TrimClipEdge { clip, edge, frame }) => Some(Self::Audio {
                clip: *clip,
                edge: *edge,
                frame: *frame,
                stretch: false,
            }),
            Request::Edit(Command::TrimMidiClipEdge { clip, edge, beat }) => Some(Self::Midi {
                clip: *clip,
                edge: *edge,
                beat: *beat,
                stretch: false,
            }),
            Request::Edit(Command::StretchMidiClipEdge { clip, edge, beat }) => Some(Self::Midi {
                clip: *clip,
                edge: *edge,
                beat: *beat,
                stretch: true,
            }),
            _ => None,
        }
    }
}

impl PendingTrim {
    fn into_viewport(self) -> RetiredTrimViewport {
        RetiredTrimViewport {
            generation: self.generation,
            viewport: self.scroll_viewport,
        }
    }
}
impl EditorView {
    pub(super) fn pending_trim_matches(&self, request: &Request) -> bool {
        self.pending_trim.as_ref().is_some_and(|pending| {
            pending.generation == self.project_generation
                && pending.remaining.is_none()
                && ResizeCommand::from_request(request) == Some(pending.command)
        })
    }
    pub(super) fn trim_request_submitted(&mut self) {
        if let Some(pending) = self.pending_trim.as_mut() {
            pending.remaining = Some(self.worker_requests_pending);
            if let Some(state) = &self.state {
                pending.revision = state.revision;
            }
        }
    }
    /// Take only our own FIFO terminal before receive installs its canonical
    /// state. Earlier completions, including typed gestures, count but do not
    /// normalize. Replacement discards the old document's owner entirely.
    pub(super) fn trim_request_complete(
        &mut self,
        event: &hex_session::service::Event,
    ) -> Option<RetiredTrimViewport> {
        let pending = self.pending_trim.as_mut()?;
        if event.project_replaced || pending.generation != self.project_generation {
            self.pending_trim = None;
            return None;
        }
        if event.request_complete
            && let Some(remaining) = pending.remaining.as_mut()
        {
            *remaining = remaining.saturating_sub(1);
            if *remaining == 0 {
                return self.pending_trim.take().map(PendingTrim::into_viewport);
            }
        }
        None
    }
    pub(super) fn normalize_completed_trim_viewport(
        &mut self,
        terminal: Option<RetiredTrimViewport>,
    ) {
        if let Some(terminal) = terminal {
            self.normalize_canceled_trim_viewport(terminal.generation, terminal.viewport);
        }
    }
    pub(super) fn retire_pending_trim(&mut self) -> bool {
        let terminal = self.pending_trim.take().map(PendingTrim::into_viewport);
        let retired = terminal.is_some();
        self.normalize_completed_trim_viewport(terminal);
        retired
    }
}

/// Use only the canonical model's applied portion of one timer step. Ordinary
/// unconstrained movement stays constant-work. A source/minimum clamp finds
/// its first saturating point; an invalid full step finds its last valid point.
/// Each search halves a u64 interval: at most 64 metadata-only iterations, never
/// per-frame retries, DSP work, grid quantization or relaxed model bounds.
fn resize_pan_prefix<T: PartialEq>(
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

fn drag_admitted(view: &EditorView) -> bool {
    view.pending_trim.is_none()
        && trim_drag_admitted(
            view.opening_project,
            view.close_prompt_open,
            view.worker_requests_pending,
            view.pending_requests.len(),
        )
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

/// Height of a clip's edge-trim strips, from the top of the clip: the header
/// rail, or the whole row when a header-only row has no body below it.
pub(super) fn edge_trim_height(row_height: f32) -> f32 {
    if row_height <= super::track_layout::HEADER_ONLY_HEIGHT {
        row_height.max(0.0)
    } else {
        super::time_selection::header_hit_height(row_height)
    }
}

fn pointer_in_lane_x(x: f32, lane: LaneBounds) -> bool {
    x.is_finite()
        && lane.left.is_finite()
        && lane.width.is_finite()
        && lane.width > 0.0
        && x >= lane.left
        && x <= lane.left + lane.width
}

fn retained_timeline_pointer(
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
    resolve_preview(drag, project, frame)
}

/// The right edge of a repeating audio clip extends by whole repeats, like the
/// outer loop corner. A plain trim cannot grow a warped clip past its retained
/// audio, so routing it as a trim would refuse every extension.
fn effective_loop_kind(drag: &TrimDrag) -> Option<LoopHandleKind> {
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
fn retained_preview(drag: &TrimDrag) -> Option<TrimPreviewState> {
    drag.preview
        .try_borrow()
        .ok()?
        .clone()
        .filter(|preview| preview.stretch == drag.stretch_modifier.get())
}

fn resolve_preview(drag: &TrimDrag, project: &Project, frame: u64) -> Option<TrimPreviewState> {
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

fn audio_grab_frame(drag: &TrimDrag, project: &Project) -> Option<u64> {
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

fn midi_grab_beat(drag: &MidiTrimDrag) -> Option<BeatTime> {
    if drag.loop_kind == Some(LoopHandleKind::FirstBoundary) {
        drag.clip.first_repeat_boundary()
    } else {
        midi_edge_beat(&drag.clip, drag.edge)
    }
}

fn preview_clip(
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
) -> Option<MidiEdgePreview> {
    midi_edge_preview(
        drag,
        snapped_midi_edge_beat(drag, project, pointer_x, lane, viewport)?,
    )
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
        scale: f32,
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
        let mut handles = Vec::with_capacity(4);
        let payload =
            self.audio_trim_payload(state, track, clip, lane, viewport, asset.playback_frames());
        if visible_start == clip.start() {
            handles.push(self.trim_handle(state, payload.clone(), clip_width, None, scale, cx));
        }
        if let Some(end) = clip.start().checked_add(clip.duration())
            && visible_end == end
        {
            let mut right = payload.clone();
            right.edge = TrimEdge::Right;
            right.repeat_available = loop_handles::audio_repeat_available(clip, &state.project);
            handles.push(self.trim_handle(state, right, clip_width, None, scale, cx));
        }
        if let Some(controls) = loop_handles::audio_controls(clip, &state.project, lane, viewport) {
            for control in controls.handles.into_iter().flatten() {
                let mut drag = payload.clone();
                drag.edge = TrimEdge::Right;
                drag.loop_kind = Some(control.kind);
                handles.push(self.trim_handle(state, drag, clip_width, Some(control), scale, cx));
            }
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
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        if !drag_admitted(self) || handle_width(clip_width) <= 0.0 {
            return Vec::new();
        }
        let Some(lane) = self.drag_lanes.get(&track).copied() else {
            return Vec::new();
        };
        let drag = MidiTrimDrag {
            loop_kind: None,
            repeat_available: false,
            repeat: Rc::new(Cell::new(false)),
            canceled: Rc::new(Cell::new(false)),
            capture: Rc::new(Cell::new(None)),
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
            stretch_modifier: Rc::new(Cell::new(false)),
            snap_modifier: Rc::new(Cell::new(false)),
            last_pointer_x: Rc::new(Cell::new(None)),
            valid_pointer_x: Rc::new(Cell::new(None)),
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
        };
        let mut handles = Vec::with_capacity(4);
        if visible_start == clip_start_frame {
            handles.push(self.midi_trim_handle(
                state,
                drag.clone(),
                TrimEdge::Left,
                clip_width,
                lane,
                viewport,
                None,
                scale,
                cx,
            ));
        }
        if visible_end == clip_end_frame {
            let mut right = drag.clone();
            right.edge = TrimEdge::Right;
            right.repeat_available = loop_handles::midi_repeat_available(clip);
            handles.push(self.midi_trim_handle(
                state,
                right,
                TrimEdge::Right,
                clip_width,
                lane,
                viewport,
                None,
                scale,
                cx,
            ));
        }
        if let Some(controls) = loop_handles::midi_controls(clip, &state.project, lane, viewport) {
            for control in controls.handles.into_iter().flatten() {
                let mut payload = drag.clone();
                payload.edge = TrimEdge::Right;
                payload.loop_kind = Some(control.kind);
                handles.push(self.midi_trim_handle(
                    state,
                    payload,
                    TrimEdge::Right,
                    clip_width,
                    lane,
                    viewport,
                    Some(control),
                    scale,
                    cx,
                ));
            }
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
        control: Option<loop_handles::LoopControl>,
        scale: f32,
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
        let project_tempo = state.project.clone();
        let (element_prefix, debug_prefix) = match payload.loop_kind {
            Some(LoopHandleKind::Outer) => ("midi-loop-outer", "midi-clip-loop-outer"),
            Some(LoopHandleKind::FirstBoundary) => ("midi-loop-cycle", "midi-clip-loop-cycle"),
            None => match edge {
                TrimEdge::Left => ("midi-trim-left", "midi-clip-trim-left"),
                TrimEdge::Right => ("midi-trim-right", "midi-clip-trim-right"),
            },
        };
        let repeat_hover = control.is_none()
            && edge == TrimEdge::Right
            && payload.repeat_available
            && self.option_held;
        let repeat_available = payload.repeat_available;
        loop_handles::base(edge, clip_width, lane.height, control, repeat_hover, scale)
            .id((element_prefix, clip_id.value()))
            .debug_selector(move || format!("{debug_prefix}-{}", clip_id.value()))
            .map(|surface| loop_handles::decorate(surface, control, edge, repeat_available))
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                mouse_down_entity.update(cx, |view, _| {
                    if view.midi_trim_drag_current(&pointer_payload) {
                        view.focus.focus(window);
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
                let capture = drag_start_entity.update(cx, |view, _| {
                    if view.midi_trim_drag_current(payload) {
                        view.take_pointer_down(clip_generation, clip_id, Some(edge))
                    } else {
                        view.clear_pointer_down(clip_generation, clip_id, Some(edge));
                        None
                    }
                });
                payload.capture.set(capture);
                let original_down_x = capture.map(|capture| capture.origin_x());
                let pointer_beat = original_down_x
                    .and_then(|x| midi_pointer_beat(&project_tempo, x, lane, viewport));
                let delta =
                    pointer_beat
                        .zip(midi_grab_beat(payload))
                        .map(|(pointer, edge_beat)| {
                            i128::from(pointer.raw()) - i128::from(edge_beat.raw())
                        });
                *payload.grab_delta.borrow_mut() = delta;
                let pointer_x = f32::from(window.mouse_position().x);
                payload.last_pointer_x.set(Some(pointer_x));
                payload.snap_modifier.set(window.modifiers().platform);
                payload.stretch_modifier.set(window.modifiers().shift);
                // The mode is fixed here: option repeats, ahead of shift-stretch.
                payload
                    .repeat
                    .set(payload.repeat_available && window.modifiers().alt);
                if payload.repeat.get() {
                    // GPUI installs the drag (with the edge's cursor) after
                    // this callback, so switch its cursor once it exists.
                    window.defer(cx, |window, cx| {
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::Loop, window);
                    });
                }
                preview_owner_entity.update(cx, |view, cx| {
                    view.begin_midi_trim_drag(payload);
                    view.update_midi_trim_preview(payload, pointer_x, cx);
                });
                cx.new(|_| TrimPreview {
                    data: Rc::new(RefCell::new(None)),
                })
            })
    }

    fn audio_trim_payload(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &Clip,
        lane: LaneBounds,
        viewport: Viewport,
        asset_playback_frames: u64,
    ) -> TrimDrag {
        TrimDrag {
            loop_kind: None,
            repeat_available: false,
            repeat: Rc::new(Cell::new(false)),
            canceled: Rc::new(Cell::new(false)),
            capture: Rc::new(Cell::new(None)),
            stretch_modifier: Rc::new(Cell::new(false)),
            project_generation: self.project_generation,
            revision: state.revision,
            source_track: track,
            clip: clip.clone(),
            asset_playback_frames,
            edge: TrimEdge::Left,
            source_lane: lane,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            last_pointer_x: Rc::new(Cell::new(None)),
            valid_pointer_x: Rc::new(Cell::new(None)),
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
            resolution: Default::default(),
        }
    }

    fn trim_handle(
        &self,
        state: &ViewState,
        payload: TrimDrag,
        clip_width: f32,
        control: Option<loop_handles::LoopControl>,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let clip_id = payload.clip.id();
        let edge = payload.edge;
        let lane = payload.source_lane;
        let viewport = payload.viewport();
        let project = state.project.clone();
        let drag_entity = cx.entity();
        let preview_owner_entity = drag_entity.clone();
        let mouse_down_entity = drag_entity.clone();
        let drag_start_entity = drag_entity.clone();
        let pointer_payload = payload.clone();
        let clear_payload = payload.clone();
        let clear_entity = drag_entity.clone();
        let (element_prefix, debug_prefix) = match payload.loop_kind {
            Some(LoopHandleKind::Outer) => ("loop-outer", "clip-loop-outer"),
            Some(LoopHandleKind::FirstBoundary) => ("loop-cycle", "clip-loop-cycle"),
            None => match edge {
                TrimEdge::Left => ("trim-left", "clip-trim-left"),
                TrimEdge::Right => ("trim-right", "clip-trim-right"),
            },
        };
        let repeat_hover = control.is_none()
            && edge == TrimEdge::Right
            && payload.repeat_available
            && self.option_held;
        let repeat_available = payload.repeat_available;
        loop_handles::base(edge, clip_width, lane.height, control, repeat_hover, scale)
            .id((element_prefix, clip_id.value()))
            .debug_selector(move || format!("{debug_prefix}-{}", clip_id.value()))
            .map(|surface| loop_handles::decorate(surface, control, edge, repeat_available))
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
                let capture = drag_start_entity.update(cx, |view, _| {
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
                payload.capture.set(capture);
                let original_down_x = capture.map(|capture| capture.origin_x());
                let pointer_frame = original_down_x.and_then(|x| frame_at_x(x, lane, viewport));
                let delta = pointer_frame
                    .zip(audio_grab_frame(payload, &project))
                    .map(|(pointer, edge)| i128::from(pointer) - i128::from(edge));
                *payload.grab_delta.borrow_mut() = delta;
                let pointer_x = f32::from(window.mouse_position().x);
                payload.last_pointer_x.set(Some(pointer_x));
                payload.snap_modifier.set(window.modifiers().platform);
                payload.stretch_modifier.set(window.modifiers().shift);
                // The mode is fixed here: option repeats, ahead of shift-stretch.
                payload
                    .repeat
                    .set(payload.repeat_available && window.modifiers().alt);
                if payload.repeat.get() {
                    // GPUI installs the drag (with the edge's cursor) after
                    // this callback, so switch its cursor once it exists.
                    window.defer(cx, |window, cx| {
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::Loop, window);
                    });
                }
                preview_owner_entity.update(cx, |view, cx| {
                    view.begin_trim_drag(payload);
                    view.update_trim_preview(payload, pointer_x, cx);
                });
                cx.new(|_| TrimPreview {
                    data: payload.preview.clone(),
                })
            })
    }

    pub(super) fn refresh_trim_pointer(
        &mut self,
        drag: &TrimDrag,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.trim_drag_current(drag) {
            replace_audio_trim_preview(&drag.preview, None);
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let pointer_x = f32::from(event.position.x);
        drag.last_pointer_x.set(Some(pointer_x));
        drag.snap_modifier.set(event.modifiers.platform);
        drag.stretch_modifier.set(event.modifiers.shift);
        self.update_trim_preview(drag, pointer_x, cx);
    }

    pub(super) fn refresh_midi_trim_pointer(
        &mut self,
        drag: &MidiTrimDrag,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.midi_trim_drag_current(drag) {
            replace_midi_trim_preview(&drag.preview, None);
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let pointer_x = f32::from(event.position.x);
        drag.last_pointer_x.set(Some(pointer_x));
        drag.snap_modifier.set(event.modifiers.platform);
        drag.stretch_modifier.set(event.modifiers.shift);
        self.update_midi_trim_preview(drag, pointer_x, cx);
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
            *drag.resolution.borrow_mut() = Default::default();
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
        let Some(pointer_x) = super::drag::PointerDown::pointer(&drag.capture, pointer_x) else {
            return false;
        };
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
        if !pointer_in_lane_x(pointer_x, lane) {
            return false;
        }
        let next = preview_for_pointer(drag, &state.project, pointer_x, lane, viewport);
        if next.is_some() {
            drag.valid_pointer_x.set(Some(pointer_x));
        } else {
            let reason = snapped_edge_frame(drag, &state.project, pointer_x, lane, viewport)
                .and_then(|frame| preview_clip(drag, &state.project, frame).err());
            let held = retained_preview(drag).is_some();
            self.status = match (held, reason) {
                (true, Some(hex_project::Invalid(reason))) => {
                    format!("resize held at the last valid length · {reason}")
                }
                (true, None) => "resize held at the last valid length".into(),
                (false, Some(hex_project::Invalid(reason))) => format!("resize refused · {reason}"),
                (false, None) => self.status.clone(),
            };
            if held {
                // Hold the last valid extent rather than dropping the gesture.
                self.active_trim_drag = Some(drag.clone());
                cx.notify();
                return false;
            }
        }
        let changed = replace_audio_trim_preview(&drag.preview, next);
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
        {
            // End the temporary preview borrow before the mutable update.
            let pointer = retained_timeline_pointer(
                drag.last_pointer_x.get(),
                drag.valid_pointer_x.get(),
                drag.preview.borrow().is_some(),
                drag.source_lane,
            );
            if let Some(pointer_x) = pointer {
                self.update_trim_preview(&drag, pointer_x, cx);
            }
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
        let Some(pointer_x) = super::drag::PointerDown::pointer(&drag.capture, pointer_x) else {
            return false;
        };
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
        if !pointer_in_lane_x(pointer_x, lane) {
            return false;
        }
        let next = midi_preview_for_pointer(drag, &state.project, pointer_x, lane, viewport);
        if next.is_some() {
            drag.valid_pointer_x.set(Some(pointer_x));
        }
        let changed = replace_midi_trim_preview(&drag.preview, next);
        self.active_midi_trim_drag = Some(drag.clone());
        if changed {
            cx.notify();
        }
        changed
    }

    pub(super) fn refresh_midi_trim_snap_modifier(
        &mut self,
        held: bool,
        stretch: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.active_midi_trim_drag.clone() {
            let snap_changed = drag.snap_modifier.replace(held) != held;
            let stretch_changed = drag.stretch_modifier.replace(stretch) != stretch;
            if snap_changed || stretch_changed {
                let pointer = retained_timeline_pointer(
                    drag.last_pointer_x.get(),
                    drag.valid_pointer_x.get(),
                    drag.preview.borrow().is_some(),
                    drag.source_lane,
                );
                if let Some(pointer_x) = pointer {
                    self.update_midi_trim_preview(&drag, pointer_x, cx);
                }
            }
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
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
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
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
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
        let Some(preview) = midi_edge_preview(drag, beat) else {
            return;
        };
        if (midi_loop_kind(drag).is_some() && midi_grab_beat(drag) == Some(beat))
            || (midi_loop_kind(drag).is_none()
                && preview.start() == drag.clip.start()
                && preview.length() == drag.clip.length()
                && preview.source_start() == drag.clip.source_start()
                && preview.playback_offset() == drag.clip.playback_offset())
        {
            self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let stretch = drag.stretch_modifier.get();
        let request = Request::Edit(if let Some(kind) = midi_loop_kind(drag) {
            Command::ResizeMidiClipView {
                clip: drag.clip.id(),
                operation: kind.midi_operation(beat),
            }
        } else if stretch {
            Command::StretchMidiClipEdge {
                clip: drag.clip.id(),
                edge: drag.edge,
                beat,
            }
        } else {
            Command::TrimMidiClipEdge {
                clip: drag.clip.id(),
                edge: drag.edge,
                beat,
            }
        });
        let Some(command) = ResizeCommand::from_request(&request) else {
            return;
        };
        self.pending_trim = Some(PendingTrim {
            generation: drag.project_generation,
            revision: drag.revision,
            track: drag.source_track,
            command,
            remaining: None,
            scroll_viewport: drag.scroll.get().viewport,
            display: PendingTrimDisplay::Midi {
                original: Rc::clone(&drag.clip),
                prospective: preview,
            },
        });
        let accepted = self.enqueue(request);
        if !accepted {
            self.pending_trim = None;
        }
        self.retire_trim_gesture(!accepted, cx);
        cx.notify();
    }

    /// Finalize the captured source, regardless of the release's hovered panel.
    /// Foreign horizontal coordinates retain the last valid timeline point;
    /// canonical validation and the existing pending-trim owner stay authoritative.
    pub(super) fn finish_trim_pointer_release(
        &mut self,
        event: &gpui::MouseUpEvent,
        _browser_release: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != MouseButton::Left
            || (self.active_trim_drag.is_none() && self.active_midi_trim_drag.is_none())
        {
            return false;
        }
        if self.mode != super::EditorMode::Arrangement || !self.focus.is_focused(window) {
            self.cancel_trim_gesture(cx);
            return true;
        }
        let x = f32::from(event.position.x);
        if !x.is_finite() || !f32::from(event.position.y).is_finite() {
            self.cancel_trim_gesture(cx);
            return true;
        }
        if let Some(drag) = self.active_trim_drag.clone() {
            drag.snap_modifier.set(event.modifiers.platform);
            drag.stretch_modifier.set(event.modifiers.shift);
            let pointer = retained_timeline_pointer(
                Some(x),
                drag.valid_pointer_x.get(),
                drag.preview.borrow().is_some(),
                drag.source_lane,
            );
            if let Some(pointer) = pointer {
                drag.last_pointer_x.set(Some(pointer));
                self.update_trim_preview(&drag, pointer, cx);
                if drag.preview.borrow().is_some() {
                    self.drop_trim(&drag, drag.source_track, window, cx);
                }
            }
        } else if let Some(drag) = self.active_midi_trim_drag.clone() {
            drag.snap_modifier.set(event.modifiers.platform);
            drag.stretch_modifier.set(event.modifiers.shift);
            let pointer = retained_timeline_pointer(
                Some(x),
                drag.valid_pointer_x.get(),
                drag.preview.borrow().is_some(),
                drag.source_lane,
            );
            if let Some(pointer) = pointer {
                drag.last_pointer_x.set(Some(pointer));
                self.update_midi_trim_preview(&drag, pointer, cx);
                if drag.preview.borrow().is_some() {
                    self.drop_midi_trim(&drag, drag.source_track, cx);
                }
            }
        }
        // A refused/stale/invalid release retires its own capture; an accepted
        // release already transferred display ownership to PendingTrim.
        self.cancel_trim_gesture(cx);
        true
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
            let pointer_frame = frame_at_x(x, lane, viewport)?;
            let requested = i128::from(pointer_frame) - delta;
            let frame = corrected_edge_frame(pointer_frame, delta);
            let clip = preview_clip(drag, &state.project, frame).ok()?;
            let endpoint = if drag.loop_kind == Some(LoopHandleKind::FirstBoundary) {
                u64::try_from(
                    hex_project::AudioViewProjection::new(&clip, state.project.tempo_map())
                        .ok()?
                        .logical_first_boundary(),
                )
                .ok()?
            } else {
                edge_frame(&clip, drag.edge)?
            };
            let unclamped = requested == i128::from(frame) && endpoint == frame;
            Some((clip, unclamped))
        };
        resize_pan_prefix(drag.viewport(), next, candidate)
    }

    pub(super) fn midi_trim_pan_viewport(
        &self,
        drag: &MidiTrimDrag,
        x: f32,
        next: Viewport,
    ) -> Option<Viewport> {
        let state = self.state.as_ref()?;
        let lane = self.drag_lanes.get(&drag.source_track).copied()?;
        let delta = (*drag.grab_delta.borrow())?;
        let candidate = |viewport| {
            let beat = midi_pointer_beat(&state.project, x, lane, viewport)?;
            let requested = i128::from(beat.raw()) - delta;
            let beat = BeatTime::from_raw(requested.clamp(0, i128::from(u64::MAX)) as u64);
            let clip = midi_edge_preview(drag, beat)?;
            let edge = if drag.loop_kind == Some(LoopHandleKind::FirstBoundary) {
                clip.boundary(&drag.clip)?.raw()
            } else {
                match drag.edge {
                    TrimEdge::Left => clip.start().raw(),
                    TrimEdge::Right => clip.start().raw().checked_add(clip.length().raw())?,
                }
            };
            Some((
                clip,
                requested == i128::from(beat.raw()) && edge == beat.raw(),
            ))
        };
        resize_pan_prefix(drag.viewport(), next, candidate)
    }

    /// A private resize may navigate beyond the current display extent, never
    /// beyond its checked viewport or into a stale owner's project/geometry.
    pub(super) fn trim_display_end(&self) -> Option<u64> {
        if let Some(pending) = self.pending_trim_current()
            && let Some(viewport) = pending.scroll_viewport
            && self.viewport == Some(viewport)
        {
            return viewport.end();
        }
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
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
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
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
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
        let Some(resolved) =
            resolve_preview(drag, &state.project, frame).or_else(|| retained_preview(drag))
        else {
            return;
        };
        let frame = resolved.command_frame;
        let preview = resolved.clip;
        if preview_is_noop(&drag.clip, &preview)
            || (effective_loop_kind(drag).is_some()
                && audio_grab_frame(drag, &state.project) == Some(frame))
        {
            self.clear_trim_drag(drag.project_generation, drag.clip.id(), cx);
            return;
        }
        let stretch = drag.stretch_modifier.get();
        let request = Request::Edit(if let Some(kind) = effective_loop_kind(drag) {
            Command::ResizeAudioClipView {
                clip: drag.clip.id(),
                operation: kind.audio_operation(frame),
            }
        } else if stretch {
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
        });
        let Some(command) = ResizeCommand::from_request(&request) else {
            return;
        };
        self.pending_trim = Some(PendingTrim {
            generation: drag.project_generation,
            revision: drag.revision,
            track: drag.source_track,
            command,
            remaining: None,
            scroll_viewport: drag.scroll.get().viewport,
            display: PendingTrimDisplay::Audio {
                original: drag.clip.clone(),
                prospective: Box::new(preview),
                asset_frames: drag.asset_playback_frames,
            },
        });
        let accepted = self.enqueue(request);
        if !accepted {
            self.pending_trim = None;
        }
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
        TrimDrag, payload_is_current, replace_audio_trim_preview, replace_midi_trim_preview,
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
    fn identical_audio_and_midi_trim_candidates_do_not_request_redraws() {
        let clip = test_clip();
        let audio = RefCell::new(None);
        let candidate = super::TrimPreviewState {
            clip: clip.clone(),
            command_frame: clip.start(),
            stretch: false,
        };
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
        assert!(replace_midi_trim_preview(
            &midi,
            Some(midi_candidate.into())
        ));
        assert!(!replace_midi_trim_preview(
            &midi,
            Some(midi_candidate.into())
        ));
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
            comparison_audio: None,
            project: Arc::clone(&state.project),
            waveforms: BTreeMap::new(),
            source_overviews: Default::default(),
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
            comparison_audio: None,
            project: Arc::new(editor.project().clone()),
            waveforms: BTreeMap::new(),
            source_overviews: Default::default(),
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
            comparison_audio: None,
            project: Arc::new(moved_editor.project().clone()),
            waveforms: BTreeMap::new(),
            source_overviews: Default::default(),
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
        let (editor, track, id) = super::geometry::clip_fixture(frames);
        let state = ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(editor.project().clone()),
            waveforms: BTreeMap::new(),
            source_overviews: Default::default(),
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

    pub(super) fn trim_drag(track: TrackId, clip: Clip, asset_playback_frames: u64) -> TrimDrag {
        TrimDrag {
            loop_kind: None,
            repeat_available: false,
            repeat: Rc::new(Cell::new(false)),
            canceled: Rc::new(Cell::new(false)),
            capture: Rc::new(Cell::new(None)),
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
            valid_pointer_x: Rc::new(Cell::new(None)),
            grab_delta: Rc::new(RefCell::new(None)),
            preview: Rc::new(RefCell::new(None)),
            resolution: Default::default(),
        }
    }

    fn test_clip() -> Clip {
        let (_, state, _, id) = state_fixture(300);
        state.project.clip(id).expect("clip").clone()
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "stretch_limit_tests.rs"]
mod stretch_limit_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "resize_release_tests.rs"]
mod resize_release_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_loop_ui_tests.rs"]
mod loop_ui_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "resize_terminal_tests.rs"]
mod resize_terminal_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_left_extension_ui_tests.rs"]
mod midi_left_extension_ui_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_stretch_ui_tests.rs"]
mod midi_stretch_ui_tests;
