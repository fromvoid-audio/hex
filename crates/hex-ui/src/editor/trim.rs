//! Native pointer trim gesture state and bounded canonical dispatch.
use super::{EditorView, ViewState, drag::LaneBounds, viewport::Viewport};
use gpui::{Context, Div, MouseButton, Stateful, Window, div, prelude::*, px};
use hex_project::{BeatTime, Clip, Command, MidiClip, Project, TrackId, TrimEdge};
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

#[path = "trim/preview_state.rs"]
mod preview_state;
pub(super) use preview_state::{MidiEdgePreview, TrimPreviewState};
use preview_state::{replace_audio_trim_preview, replace_midi_trim_preview};

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

/// An audio trim surface retains only the source and geometry it rendered.
#[derive(Clone)]
struct AudioTrimSource {
    loop_kind: Option<LoopHandleKind>,
    repeat_available: bool,
    generation: u64,
    revision: u64,
    track: TrackId,
    clip: Rc<Clip>,
    asset_playback_frames: u64,
    edge: TrimEdge,
    lane: LaneBounds,
    viewport: Viewport,
    snap_grid: super::musical_time::SnapGrid,
}

impl AudioTrimSource {
    fn payload(&self) -> TrimDrag {
        TrimDrag {
            loop_kind: self.loop_kind,
            repeat_available: self.repeat_available,
            repeat: Rc::new(Cell::new(false)),
            canceled: Rc::new(Cell::new(false)),
            capture: Rc::new(Cell::new(None)),
            stretch_modifier: Rc::new(Cell::new(false)),
            project_generation: self.generation,
            revision: self.revision,
            source_track: self.track,
            clip: self.clip.as_ref().clone(),
            asset_playback_frames: self.asset_playback_frames,
            edge: self.edge,
            source_lane: self.lane,
            viewport_start: self.viewport.start,
            viewport_length: self.viewport.length,
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

/// One resize owner: terminal core plus validated prospective display metadata.
/// No separate completion key, FIFO counter or viewport owner.
pub(super) struct PendingTrim {
    replayed: bool,
    generation: u64,
    revision: u64,
    pub(super) track: TrackId,
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

#[derive(Clone, Copy)]
pub(super) struct LaneTrimTarget {
    pub(super) track: TrackId,
    pub(super) clip_id: hex_project::ClipId,
    pub(super) edge: TrimEdge,
    pub(super) loop_kind: Option<LoopHandleKind>,
}

pub(super) struct TrimPreview {
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

mod audio_actions;
mod midi_actions;
mod preview_math;

use preview_math::*;
pub(super) use preview_math::{drag_admitted, edge_trim_height, preview_clip};

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests;

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
