mod geometry;
use geometry::lane_contains_pointer;
pub(super) use geometry::{
    ClipSnapEdge, LaneBounds, clip_snap_edge, drop_start, grab_offset_from_pointer,
};

use super::musical_time::SnapGrid;
use super::viewport::Viewport;
use gpui::{Context, Window};
use hex_project::Command;
use hex_project::{BeatTime, ClipId, Project, TrackId, TrimEdge};
use hex_session::service::{AudioPreview, Request};
use hex_widgets::{RectangleCapture, RectanglePart};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Default, Debug)]
pub(super) struct ClipDragGroup {
    pub(super) selection_epoch: Option<u64>,
    pub(super) ids: Vec<ClipId>,
    capture: Option<hex_project::ClipGroupMovePreview>,
}

impl ClipDragGroup {
    pub(super) fn previews(&self) -> &[hex_project::ClipGroupPreview] {
        self.capture
            .as_ref()
            .map_or(&[], |capture| capture.previews())
    }
    pub(super) fn invalidate(&mut self) {
        if let Some(capture) = self.capture.as_mut() {
            capture.invalidate();
        }
    }
    pub(super) fn original_audio<'a>(
        &self,
        project: &'a std::sync::Arc<Project>,
        index: usize,
    ) -> Option<&'a hex_project::Clip> {
        self.capture.as_ref()?.original_audio(project, index)
    }
}

/// Bounded, metadata-only payload for one native pointer drag.
#[derive(Clone, Debug)]
pub(super) struct ClipDrag {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) source_track: TrackId,
    pub(super) clip: ClipId,
    pub(super) group: Rc<RefCell<ClipDragGroup>>,
    pub(super) position: ClipPosition,
    pub(super) grab_offset: Rc<Cell<Option<u64>>>,
    pub(super) snap_edge: Rc<Cell<Option<ClipSnapEdge>>>,
    /// Last completed resting layout, before trim handles narrow the move hit
    /// surface. Used only while admitting the original press.
    pub(super) source_span: Rc<Cell<Option<(f32, f32)>>>,
    pub(super) capture: Rc<Cell<Option<PointerDown>>>,
    pub(super) viewport_start: u64,
    pub(super) viewport_length: u64,
    pub(super) scroll: super::drag_autoscroll::DragScroll,
    pub(super) snap_grid: SnapGrid,
    /// Command bypasses both original-position capture and the move grid.
    pub(super) snap_modifier: Rc<Cell<bool>>,
    pub(super) copy_modifier: Rc<Cell<bool>>,
    pub(super) preview: Rc<Cell<Option<DropPreview>>>,
    pub(super) destination: Rc<Cell<Option<ClipDropDestination>>>,
    pub(super) insertion_line_y: Rc<Cell<Option<f32>>>,
    pub(super) ghost_label: String,
    pub(super) ghost_waveform: Option<std::sync::Arc<hex_session::CachedWaveform>>,
    pub(super) lane_hovered: Rc<Cell<bool>>,
    pub(super) last_pointer: Rc<Cell<Option<(f32, f32)>>>,
}

impl ClipDrag {
    pub(super) fn alias_copy(&self) -> bool {
        self.copy_modifier.get() && self.snap_modifier.get()
    }

    /// The grabbed clip supplies the cursor for both single and group drags.
    pub(super) fn cursor_position(&self) -> Option<(Option<TrackId>, u64)> {
        match self.destination.get()? {
            ClipDropDestination::ExistingTrack(track) => {
                let preview = self.preview.get().filter(|p| p.target_track == track)?;
                Some((Some(track), preview.start_frame))
            }
            ClipDropDestination::NewTrack { start_frame, .. } => Some((None, start_frame)),
        }
    }

    pub(super) fn retire(&self) {
        self.preview.set(None);
        self.group.borrow_mut().capture = None;
        self.destination.set(None);
        self.insertion_line_y.set(None);
        self.lane_hovered.set(false);
    }
}

/// In-window drag payload for a WAV from the library or retained take.
#[derive(Clone)]
pub(super) struct BrowserAudioDrag {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) browser_generation: Option<u64>,
    pub(super) sample_rate: u32,
    pub(super) path: std::path::PathBuf,
    pub(super) label: String,
    pub(super) viewport_start: u64,
    pub(super) viewport_length: u64,
    pub(super) scroll: super::drag_autoscroll::DragScroll,
    pub(super) snap_grid: SnapGrid,
    pub(super) snap_modifier: Rc<Cell<bool>>,
    pub(super) copy_modifier: Rc<Cell<bool>>,
    pub(super) metadata: Rc<std::cell::RefCell<Option<Rc<AudioPreview>>>>,
    pub(super) metadata_error: Rc<std::cell::RefCell<Option<String>>>,
    pub(super) preview: Rc<Cell<Option<DropPreview>>>,
    pub(super) destination: Rc<Cell<Option<BrowserDropDestination>>>,
    pub(super) insertion_line_y: Rc<Cell<Option<f32>>>,
    pub(super) lane_hovered: Rc<Cell<bool>>,
    pub(super) last_pointer: Rc<Cell<Option<(f32, f32)>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NativeDeviceDropTarget {
    pub(super) track: TrackId,
    /// Canonical ordered-chain node before which the device should land.
    /// `None` appends existing devices; new browser instruments default to front.
    pub(super) before: Option<hex_project::DeviceId>,
    /// Explicit replacement intent, set only when a browser instrument is
    /// released over the existing instrument card itself.
    pub(super) replace: Option<hex_project::DeviceId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NativeDeviceDragSource {
    Browser {
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
    },
    PluginEffect(std::sync::Arc<hex_plugin_catalog::PluginSelection>),
    Existing {
        device: hex_project::DeviceId,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReturnDeviceDropTarget {
    pub(super) owner: super::device_panel::EffectBusOwner,
    pub(super) before: Option<hex_project::DeviceId>,
}

/// Metadata-only payload shared by catalog drops and ordered-chain moves.
#[derive(Clone, Debug)]
pub(super) struct NativeDeviceDrag {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) source: NativeDeviceDragSource,
    pub(super) destination: Rc<Cell<Option<NativeDeviceDropTarget>>>,
    pub(super) return_destination: Rc<Cell<Option<ReturnDeviceDropTarget>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BrowserDropDestination {
    Console(hex_session::import_batch::ConsoleDestination),
    ExistingTrack {
        track: TrackId,
        start: u64,
    },
    NewTrack {
        before_track: Option<TrackId>,
        start: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClipDropDestination {
    ExistingTrack(TrackId),
    NewTrack {
        before_track: Option<TrackId>,
        position: ClipPosition,
        start_frame: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClipPosition {
    Audio { start: u64, duration: u64 },
    Midi { start: BeatTime, length: BeatTime },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DropPreview {
    pub(super) target_track: TrackId,
    pub(super) start_frame: u64,
    pub(super) end_frame: u64,
    pub(super) position: ClipPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ClipPointerKey {
    pub(super) generation: u64,
    pub(super) clip: ClipId,
}
/// The rectangle owns keyed input; the clip owner adds the edge chosen at the
/// original press. They move/cancel together across render replacement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PointerDown {
    rectangle: RectangleCapture<ClipPointerKey>,
    pub(super) snap_edge: Option<ClipSnapEdge>,
}
impl PointerDown {
    pub(super) fn begin(
        key: ClipPointerKey,
        part: RectanglePart,
        x: f32,
        enabled: bool,
    ) -> Option<Self> {
        Some(Self {
            rectangle: RectangleCapture::begin(key, part, x, enabled)?,
            snap_edge: None,
        })
    }
    pub(super) fn take_matching(
        held: &mut Option<Self>,
        key: ClipPointerKey,
        part: RectanglePart,
    ) -> Option<Self> {
        if held.is_some_and(|held| held.rectangle.key == key && held.rectangle.part == part) {
            held.take()
        } else {
            None
        }
    }
    pub(super) fn origin_x(self) -> f32 {
        self.rectangle.origin_x
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn part(self) -> RectanglePart {
        self.rectangle.part
    }
    pub(super) fn update(
        &mut self,
        x: f32,
    ) -> Option<hex_widgets::RectangleIntent<ClipPointerKey>> {
        self.rectangle.update(x)
    }
    pub(super) fn displaced(self, threshold: f32) -> bool {
        self.rectangle.displaced(threshold)
    }
    pub(super) fn pointer(held: &Cell<Option<Self>>, x: f32) -> Option<f32> {
        let mut press = held.get();
        let rectangle = Cell::new(press.map(|press| press.rectangle));
        let pointer = hex_widgets::rectangle_pointer(&rectangle, x)?;
        if let Some(press) = press.as_mut() {
            press.rectangle = rectangle.get()?;
        }
        held.set(press);
        Some(pointer)
    }
}
pub(super) fn rectangle_part(edge: Option<TrimEdge>) -> RectanglePart {
    match edge {
        None => RectanglePart::Body,
        Some(TrimEdge::Left) => RectanglePart::Leading,
        Some(TrimEdge::Right) => RectanglePart::Trailing,
    }
}
#[cfg(all(test, not(feature = "ui-test-support")))]
fn take_pointer_down(
    pending: &mut Option<PointerDown>,
    generation: u64,
    clip: ClipId,
    edge: Option<TrimEdge>,
) -> Option<f32> {
    PointerDown::take_matching(
        pending,
        ClipPointerKey { generation, clip },
        rectangle_part(edge),
    )
    .map(PointerDown::origin_x)
}

// Window-space radius, independent of timeline zoom or audio/MIDI units.
const ORIGINAL_POSITION_SNAP_PIXELS: f64 = 8.0;

fn near_original_anchor(frame: u64, original: u64, viewport: Viewport, width: f32) -> bool {
    width.is_finite()
        && width > 0.0
        && viewport.length > 0
        && frame.abs_diff(original) as f64 / viewport.length as f64 * f64::from(width)
            <= ORIGINAL_POSITION_SNAP_PIXELS
}

fn candidate_position(
    project: &Project,
    drag: &ClipDrag,
    target_track: Option<TrackId>,
    pointer_x: f32,
    lane: LaneBounds,
) -> Option<ClipPosition> {
    let viewport = drag.viewport();
    let bypass = drag.snap_modifier.get();
    let grid = if bypass {
        SnapGrid::Off
    } else {
        drag.snap_grid
    };
    match drag.position {
        ClipPosition::Audio {
            start: original,
            duration,
        } => {
            let original_end = original.checked_add(duration)?;
            let start = drop_start(
                pointer_x,
                lane,
                viewport.start,
                viewport.length,
                drag.grab_offset.get()?,
            )?;
            let edge = drag.snap_edge.get()?;
            let anchor = match edge {
                ClipSnapEdge::Leading => start,
                ClipSnapEdge::Trailing => start.checked_add(duration)?,
            };
            let original_anchor = match edge {
                ClipSnapEdge::Leading => original,
                ClipSnapEdge::Trailing => original_end,
            };
            if !bypass && near_original_anchor(anchor, original_anchor, viewport, lane.width) {
                return Some(drag.position);
            }
            let anchor = grid.snap_frame(project, anchor, viewport, lane.width)?;
            let start = match edge {
                ClipSnapEdge::Leading => anchor,
                ClipSnapEdge::Trailing => anchor.saturating_sub(duration),
            };
            start.checked_add(duration)?;
            Some(ClipPosition::Audio { start, duration })
        }
        ClipPosition::Midi {
            start: original,
            length,
        } => {
            let original_end = BeatTime::from_raw(original.raw().checked_add(length.raw())?);
            if !bypass
                && drag.scroll.get().viewport.is_none()
                && target_track == Some(drag.source_track)
                && drag.capture.get().is_some_and(|mut capture| {
                    capture.update(pointer_x).is_some() && !capture.displaced(0.5)
                })
            {
                return Some(drag.position);
            }
            let pointer_frame = drop_start(pointer_x, lane, viewport.start, viewport.length, 0)?;
            let pointer_beat = project.tempo_map().frame_to_beat(pointer_frame).ok()?;
            let offset = drag.grab_offset.get()?;
            let start = BeatTime::from_raw(pointer_beat.raw().saturating_sub(offset));
            let edge = drag.snap_edge.get()?;
            let anchor = match edge {
                ClipSnapEdge::Leading => start,
                ClipSnapEdge::Trailing => {
                    BeatTime::from_raw(start.raw().checked_add(length.raw())?)
                }
            };
            let original_anchor = match edge {
                ClipSnapEdge::Leading => original,
                ClipSnapEdge::Trailing => original_end,
            };
            if !bypass
                && near_original_anchor(
                    project.tempo_map().beat_to_frame(anchor).ok()?,
                    project.tempo_map().beat_to_frame(original_anchor).ok()?,
                    viewport,
                    lane.width,
                )
            {
                // Preserve Q32.32 directly: a frame round trip can erase a
                // subframe original even when the pointer has not moved.
                return Some(drag.position);
            }
            let anchor = grid.snap_beat(project, anchor, viewport, lane.width)?;
            let start = match edge {
                ClipSnapEdge::Leading => anchor,
                ClipSnapEdge::Trailing => {
                    BeatTime::from_raw(anchor.raw().saturating_sub(length.raw()))
                }
            };
            let end = start.raw().checked_add(length.raw())?;
            project.tempo_map().beat_to_frame(start).ok()?;
            project
                .tempo_map()
                .beat_to_frame(BeatTime::from_raw(end))
                .ok()?;
            Some(ClipPosition::Midi { start, length })
        }
    }
}

pub(super) struct BrowserDropCandidate<'a> {
    pub project: &'a Project,
    pub viewport_start: u64,
    pub viewport_length: u64,
    pub snap_grid: SnapGrid,
    pub snap_modifier: bool,
    pub pointer_x: f32,
    pub lane: LaneBounds,
    pub duration: u64,
    pub target_track: TrackId,
}

pub(super) fn browser_candidate(candidate: BrowserDropCandidate<'_>) -> Option<DropPreview> {
    let start = browser_start(
        candidate.project,
        candidate.viewport_start,
        candidate.viewport_length,
        candidate
            .snap_grid
            .with_platform_modifier(candidate.snap_modifier),
        candidate.pointer_x,
        candidate.lane,
    )?;
    preview_for(
        candidate.project,
        candidate.target_track,
        ClipPosition::Audio {
            start,
            duration: candidate.duration,
        },
    )
}

pub(super) fn browser_start(
    project: &Project,
    viewport_start: u64,
    viewport_length: u64,
    snap_grid: SnapGrid,
    pointer_x: f32,
    lane: LaneBounds,
) -> Option<u64> {
    let pointer_frame = drop_start(pointer_x, lane, viewport_start, viewport_length, 0)?;
    snap_grid.snap_frame(
        project,
        pointer_frame,
        Viewport {
            start: viewport_start,
            length: viewport_length,
        },
        lane.width,
    )
}

pub(super) fn preview_for(
    project: &Project,
    target_track: TrackId,
    position: ClipPosition,
) -> Option<DropPreview> {
    let (start_frame, end_frame) = match position {
        ClipPosition::Audio { start, duration } => (start, start.checked_add(duration)?),
        ClipPosition::Midi { start, length } => {
            let end = BeatTime::from_raw(start.raw().checked_add(length.raw())?);
            (
                project.tempo_map().beat_to_frame(start).ok()?,
                project.tempo_map().beat_to_frame(end).ok()?,
            )
        }
    };
    (start_frame < end_frame).then_some(DropPreview {
        target_track,
        start_frame,
        end_frame,
        position,
    })
}

#[derive(Clone, Copy)]
pub(super) struct DropState<'a> {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) project: &'a Project,
    pub(super) target_track: TrackId,
    pub(super) position: ClipPosition,
    pub(super) viewport_start: u64,
    pub(super) viewport_length: u64,
    pub(super) snap_grid: SnapGrid,
}

fn context_is_current(drag: &ClipDrag, state: &DropState<'_>) -> bool {
    if drag.project_generation != state.project_generation
        || drag.revision != state.revision
        || drag.viewport().start != state.viewport_start
        || drag.viewport().length != state.viewport_length
        || drag.snap_grid != state.snap_grid
    {
        return false;
    }
    if state.project.track(state.target_track).is_none() {
        return false;
    }
    true
}

pub(super) fn is_current(drag: &ClipDrag, state: DropState<'_>) -> bool {
    if !context_is_current(drag, &state) {
        return false;
    }
    match drag.position {
        ClipPosition::Audio { start, duration } => state
            .project
            .track(drag.source_track)
            .and_then(|track| track.clips().iter().find(|clip| clip.id() == drag.clip))
            .is_some_and(|clip| clip.start() == start && clip.duration() == duration),
        ClipPosition::Midi { start, length } => state
            .project
            .track(drag.source_track)
            .and_then(|track| {
                track
                    .midi_clips()
                    .iter()
                    .find(|clip| clip.id() == drag.clip)
            })
            .is_some_and(|clip| clip.start() == start && clip.length() == length),
    }
}

pub(super) fn is_noop(drag: &ClipDrag, state: &DropState<'_>) -> bool {
    drag.source_track == state.target_track && drag.position == state.position
}

pub(super) fn group_position(position: ClipPosition) -> hex_project::ClipGroupPosition {
    match position {
        ClipPosition::Audio { start, .. } => hex_project::ClipGroupPosition::Audio(start),
        ClipPosition::Midi { start, .. } => hex_project::ClipGroupPosition::Midi(start),
    }
}

fn move_command(clip: ClipId, target_track: TrackId, position: ClipPosition) -> Command {
    match position {
        ClipPosition::Audio { start, .. } => Command::MoveClipWithOverwrite {
            clip,
            track: target_track,
            start,
        },
        ClipPosition::Midi { start, .. } => Command::MoveMidiClipWithOverwrite {
            clip,
            track: target_track,
            start,
        },
    }
}

fn copy_command(clip: ClipId, target_track: TrackId, position: ClipPosition) -> Command {
    match position {
        ClipPosition::Audio { start, .. } => Command::CopyClipTo {
            clip,
            track: target_track,
            start,
        },
        ClipPosition::Midi { start, .. } => Command::CopyMidiClipTo {
            clip,
            track: target_track,
            start,
        },
    }
}

fn move_to_new_track_command(
    clip: ClipId,
    name: String,
    before_track: Option<TrackId>,
    position: ClipPosition,
    gain: f32,
) -> Command {
    match position {
        ClipPosition::Audio { start, .. } => Command::MoveClipToNewTrack {
            initial_gain: Some(gain),
            clip,
            name,
            before_track,
            start,
        },
        ClipPosition::Midi { start, .. } => Command::MoveMidiClipToNewTrack {
            initial_gain: Some(gain),
            clip,
            name,
            before_track,
            start,
        },
    }
}

fn copy_to_new_track_command(
    clip: ClipId,
    name: String,
    before_track: Option<TrackId>,
    position: ClipPosition,
    gain: f32,
) -> Command {
    match position {
        ClipPosition::Audio { start, .. } => Command::CopyClipToNewTrack {
            initial_gain: Some(gain),
            clip,
            name,
            before_track,
            start,
        },
        ClipPosition::Midi { start, .. } => Command::CopyMidiClipToNewTrack {
            initial_gain: Some(gain),
            clip,
            name,
            before_track,
            start,
        },
    }
}

impl super::EditorView {
    pub(super) fn remember_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
        x: f32,
    ) {
        self.pointer_down = PointerDown::begin(
            ClipPointerKey { generation, clip },
            rectangle_part(edge),
            x,
            true,
        );
    }

    pub(super) fn take_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
    ) -> Option<PointerDown> {
        PointerDown::take_matching(
            &mut self.pointer_down,
            ClipPointerKey { generation, clip },
            rectangle_part(edge),
        )
    }

    pub(super) fn clear_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
    ) {
        PointerDown::take_matching(
            &mut self.pointer_down,
            ClipPointerKey { generation, clip },
            rectangle_part(edge),
        );
    }

    pub(super) fn drag_start_is_current(&self, drag: &ClipDrag) -> bool {
        let current = (|| {
            if drag.group.borrow().ids.len() > 1
                && drag.group.borrow().selection_epoch != Some(self.clip_selection.epoch)
            {
                return false;
            }
            // Source validity follows the installed canonical state. Harmless
            // worker work does not retire a held gesture; existing-track release
            // also validates this captured revision on the worker before editing.
            if self.opening_project || self.close_prompt_open {
                return false;
            }
            let Some(state) = self.state.as_ref() else {
                return false;
            };
            let viewport = self.visible_viewport(state.timeline_frames);
            let drop_state = DropState {
                project_generation: self.project_generation,
                revision: state.revision,
                project: &state.project,
                target_track: drag.source_track,
                position: drag.position,
                viewport_start: viewport.start,
                viewport_length: viewport.length,
                snap_grid: self.snap_grid,
            };
            let group = drag.group.borrow();
            if group.ids.len() > 1 {
                let length = match drag.position {
                    ClipPosition::Audio { duration, .. } => duration,
                    ClipPosition::Midi { length, .. } => length.raw(),
                };
                context_is_current(drag, &drop_state)
                    && group.capture.as_ref().is_some_and(|capture| {
                        capture.anchor_matches(
                            &state.project,
                            drag.clip,
                            drag.source_track,
                            group_position(drag.position),
                            length,
                        )
                    })
            } else {
                is_current(drag, drop_state)
            }
        })();
        if !current {
            // Context retirement happens inside the shared payload owner; a
            // GPUI payload clone cannot keep a usable source capture alive.
            drag.group.borrow_mut().capture = None;
        }
        current
    }

    pub(super) fn drag_candidate(
        &self,
        drag: &ClipDrag,
        target_track: TrackId,
        pointer_x: f32,
        lane: LaneBounds,
    ) -> Option<DropPreview> {
        // Every unsuccessful projection invalidates ALL row destinations,
        // including early stale/no-state/position failures and final bounds.
        let preview = (|| {
            if !self.drag_start_is_current(drag) {
                return None;
            }
            let state = self.state.as_ref()?;
            let position =
                candidate_position(&state.project, drag, Some(target_track), pointer_x, lane)?;
            if drag.group.borrow().ids.len() > 1 {
                let mut group = drag.group.borrow_mut();
                group
                    .capture
                    .as_mut()?
                    .update(&state.project, target_track, group_position(position))
                    .ok()?;
            }
            let position = if drag.group.borrow().ids.len() > 1 {
                let group = drag.group.borrow();
                match &group
                    .previews()
                    .iter()
                    .find(|p| p.id() == drag.clip)?
                    .content
                {
                    hex_project::ClipGroupContent::Audio(c) => ClipPosition::Audio {
                        start: c.start(),
                        duration: c.duration(),
                    },
                    hex_project::ClipGroupContent::Midi(c) => ClipPosition::Midi {
                        start: c.start(),
                        length: c.length(),
                    },
                }
            } else {
                position
            };
            preview_for(&state.project, target_track, position)
        })();
        if preview.is_none() {
            drag.group.borrow_mut().invalidate();
        }
        preview
    }

    pub(super) fn clip_drag_drop_position(
        &self,
        drag: &ClipDrag,
        pointer_x: f32,
        pointer_y: f32,
    ) -> (Option<DropPreview>, bool) {
        drag.group.borrow_mut().invalidate();
        let Some(pointer_x) = PointerDown::pointer(&drag.capture, pointer_x) else {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, false);
        };
        if !self.drag_start_is_current(drag) {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, false);
        }
        let Some(state) = self.state.as_ref() else {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, false);
        };
        if let Some((before_track, timeline, line_y)) =
            self.painted_track_insertion_target(pointer_x, pointer_y)
        {
            if drag.group.borrow().ids.len() > 1 {
                drag.group.borrow_mut().invalidate();
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                return (None, true);
            }
            let preview = candidate_position(&state.project, drag, None, pointer_x, timeline)
                .and_then(|position| preview_for(&state.project, drag.source_track, position));
            let Some(preview) = preview else {
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                return (None, false);
            };
            drag.destination.set(Some(ClipDropDestination::NewTrack {
                before_track,
                position: preview.position,
                start_frame: preview.start_frame,
            }));
            drag.insertion_line_y.set(Some(line_y));
            return (None, true);
        }
        if let Some((track, lane)) = self.hovered_drag_target(pointer_x, pointer_y) {
            let preview = self.drag_candidate(drag, track, pointer_x, lane);
            drag.destination
                .set(preview.map(|_| ClipDropDestination::ExistingTrack(track)));
            drag.insertion_line_y.set(None);
            return (preview, true);
        }
        drag.destination.set(None);
        drag.insertion_line_y.set(None);
        (None, false)
    }

    /// Track destinations include attached automation rows, while clip editing
    /// continues to use the separately measured body bounds in `drag_lanes`.
    /// Both bounds publish together under the current painted body's epoch.
    pub(super) fn painted_track_drop_lane(&self, track: TrackId) -> Option<LaneBounds> {
        if !self.arrangement_track_visible(track)
            || self.drag_lane_epochs.get(&track) != Some(&self.drag_layout_generation)
        {
            return None;
        }
        self.track_drop_lanes.get(&track).copied()
    }
    pub(super) fn hovered_track_drop_target(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(TrackId, LaneBounds)> {
        self.drag_lanes.keys().find_map(|track| {
            let lane = self.painted_track_drop_lane(*track)?;
            lane_contains_pointer(x, y, lane).then_some((*track, lane))
        })
    }

    pub(super) fn hovered_drag_target(
        &self,
        pointer_x: f32,
        pointer_y: f32,
    ) -> Option<(TrackId, LaneBounds)> {
        self.drag_lanes
            .iter()
            .filter(|(track, _)| {
                self.arrangement_track_visible(**track)
                    && self.drag_lane_epochs.get(track) == Some(&self.drag_layout_generation)
            })
            .find_map(|(track, lane)| {
                lane_contains_pointer(pointer_x, pointer_y, *lane).then_some((*track, *lane))
            })
    }

    pub(super) fn retire_active_clip_drag(&mut self) -> bool {
        self.drag_bounce_held = false;
        let Some(drag) = self.active_clip_drag.take() else {
            return false;
        };
        drag.retire();
        true
    }

    pub(super) fn clear_clip_drag(&mut self, generation: u64, clip: ClipId) {
        if self
            .active_clip_drag
            .as_ref()
            .is_some_and(|drag| drag.project_generation == generation && drag.clip == clip)
        {
            self.retire_active_clip_drag();
        }
    }

    pub(super) fn begin_clip_drag(&mut self, drag: &ClipDrag) {
        if drag.group.borrow().ids.is_empty() {
            drag.group.borrow_mut().ids = if self.clip_is_selected(drag.clip) {
                self.selected_clip_ids()
            } else {
                vec![drag.clip]
            };
            drag.group.borrow_mut().selection_epoch = Some(self.clip_selection.epoch);
        }
        if self
            .active_clip_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.group, &drag.group))
        {
            return;
        }
        self.retire_active_clip_drag();
        if drag.group.borrow().ids.len() > 1 {
            let capture = self.state.as_ref().and_then(|state| {
                hex_project::ClipGroupMovePreview::capture(
                    &state.project,
                    &drag.group.borrow().ids,
                    drag.clip,
                )
                .ok()
            });
            drag.group.borrow_mut().capture = capture;
        }
        self.active_native_device_drag = None;
        self.active_clip_drag = Some(drag.clone());
        self.active_browser_audio_drag = None;
    }

    pub(super) fn drop_clip_to_new_track(
        &mut self,
        drag: &ClipDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.group.borrow().ids.len() > 1 {
            self.error =
                Some("clip group move canceled · choose existing destination tracks".into());
            self.clear_clip_drag(drag.project_generation, drag.clip);
            cx.notify();
            return;
        }
        drag.snap_modifier.set(window.modifiers().platform);
        drag.copy_modifier.set(window.modifiers().alt);
        let owns_gesture = self.active_clip_drag.as_ref().is_some_and(|active| {
            active.project_generation == drag.project_generation
                && active.clip == drag.clip
                && Rc::ptr_eq(&active.destination, &drag.destination)
        });
        if !owns_gesture {
            return;
        }
        let current = self.drag_start_is_current(drag);
        let pointer = window.mouse_position();
        let pointer = (f32::from(pointer.x), f32::from(pointer.y));
        drag.last_pointer.set(Some(pointer));
        let (preview, lane_hovered) = self.clip_drag_drop_position(drag, pointer.0, pointer.1);
        drag.preview.set(preview);
        drag.lane_hovered.set(lane_hovered);
        let destination = drag.destination.get();
        if !current {
            self.clear_clip_drag(drag.project_generation, drag.clip);
            return;
        }
        let Some(ClipDropDestination::NewTrack {
            before_track,
            position,
            ..
        }) = destination
        else {
            self.clear_clip_drag(drag.project_generation, drag.clip);
            return;
        };
        if self.opening_project
            || self.close_prompt_open
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
        {
            self.error = Some("clip move canceled · project work pending".into());
            self.clear_clip_drag(drag.project_generation, drag.clip);
            cx.notify();
            return;
        }
        let gain = match self.track_defaults.captured_gain() {
            Ok(gain) => gain,
            Err(error) => {
                self.error = Some(error.into());
                self.clear_clip_drag(drag.project_generation, drag.clip);
                cx.notify();
                return;
            }
        };
        let count = self
            .state
            .as_ref()
            .map_or(0, |state| state.project.tracks().len());
        let name = format!("track {}", count + 1);
        if drag.alias_copy() {
            let command = Command::DuplicateAliasClipToNewTrack {
                initial_gain: Some(gain),
                clip: drag.clip,
                name,
                before_track,
                position: group_position(position),
            };
            let copy_intent = super::group_copy::Pending::capture(self, &command);
            let Some(source) = self
                .state
                .as_ref()
                .map(|state| std::sync::Arc::clone(&state.project))
            else {
                self.clear_clip_drag(drag.project_generation, drag.clip);
                return;
            };
            self.pending_clip_move = self.active_clip_drag.take().map(|drag| {
                super::pending_clip_move::Pending::new(
                    drag,
                    command.clone(),
                    source,
                    self.selection_intent_epoch,
                )
            });
            let before_pending = self.worker_requests_pending;
            if self.enqueue(Request::MoveExistingClips {
                project_generation: drag.project_generation,
                revision: drag.revision,
                command,
            }) {
                if let Some(mut intent) = copy_intent {
                    if self.worker_requests_pending > before_pending {
                        intent.submitted(self.worker_requests_pending);
                    }
                    self.pending_group_copy = Some(intent);
                }
            } else {
                self.retire_pending_clip_move();
                self.clear_clip_drag(drag.project_generation, drag.clip);
            }
        } else {
            let command = if drag.copy_modifier.get() {
                copy_to_new_track_command(drag.clip, name, before_track, position, gain)
            } else {
                move_to_new_track_command(drag.clip, name, before_track, position, gain)
            };
            let Some(source) = self
                .state
                .as_ref()
                .map(|state| std::sync::Arc::clone(&state.project))
            else {
                self.clear_clip_drag(drag.project_generation, drag.clip);
                return;
            };
            self.pending_clip_move = self.active_clip_drag.take().map(|drag| {
                super::pending_clip_move::Pending::new(
                    drag,
                    command.clone(),
                    source,
                    self.selection_intent_epoch,
                )
            });
            if !self.enqueue(Request::Edit(command)) {
                self.retire_pending_clip_move();
                self.clear_clip_drag(drag.project_generation, drag.clip);
            }
        }
        cx.notify();
    }

    pub(super) fn audio_file_drag_payload(
        &self,
        path: std::path::PathBuf,
        label: String,
        browser_generation: Option<u64>,
        metadata: Rc<RefCell<Option<Rc<AudioPreview>>>>,
        metadata_error: Rc<RefCell<Option<String>>>,
    ) -> Option<BrowserAudioDrag> {
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        Some(BrowserAudioDrag {
            project_generation: self.project_generation,
            revision: state.revision,
            browser_generation,
            sample_rate: state.project.sample_rate(),
            path,
            label,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            copy_modifier: Rc::new(Cell::new(false)),
            metadata,
            metadata_error,
            preview: Rc::new(Cell::new(None)),
            destination: Rc::new(Cell::new(None)),
            insertion_line_y: Rc::new(Cell::new(None)),
            lane_hovered: Rc::new(Cell::new(false)),
            last_pointer: Rc::new(Cell::new(None)),
        })
    }

    pub(super) fn begin_browser_audio_drag(&mut self, drag: &BrowserAudioDrag) {
        self.retire_active_clip_drag();
        self.active_native_device_drag = None;
        self.active_browser_audio_drag = Some(drag.clone());
    }

    pub(super) fn begin_native_device_drag(&mut self, drag: &NativeDeviceDrag) {
        self.arrangement_device_preview = None;
        self.retire_active_clip_drag();
        self.active_browser_audio_drag = None;
        if matches!(
            drag.source,
            NativeDeviceDragSource::Browser { .. } | NativeDeviceDragSource::PluginEffect(_)
        ) {
            // Make the native-device receiver visible as soon as a catalog drag
            // becomes active. Keep the selected track/clip untouched: a missing
            // target remains an empty device pane and the eventual drop can refuse.
            self.device_panel_active = true;
            self.clip_view_open = true;
        }
        self.active_native_device_drag = Some(drag.clone());
    }

    pub(super) fn native_device_drag_is_current(&self, drag: &NativeDeviceDrag) -> bool {
        if self.opening_project || self.close_prompt_open {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.project_generation != drag.project_generation || state.revision != drag.revision {
            return false;
        }
        match drag.source {
            NativeDeviceDragSource::Browser { .. } | NativeDeviceDragSource::PluginEffect(_) => {
                true
            }
            NativeDeviceDragSource::Existing {
                device,
                class,
                type_id,
            } => {
                state.project.tracks().iter().any(|track| {
                    track.ordered_device_chain().any(|node| {
                        let (candidate, candidate_class) = match node {
                            hex_project::TrackDeviceRef::Instrument(device) => {
                                (device, hex_engine::NativeDeviceClass::Instrument)
                            }
                            hex_project::TrackDeviceRef::Effect(device) => {
                                (device, hex_engine::NativeDeviceClass::Effect)
                            }
                        };
                        candidate.id() == device
                            && candidate_class == class
                            && candidate.state().type_id() == type_id
                    })
                }) || (class == hex_engine::NativeDeviceClass::Effect
                    && state.project.main_devices().iter().any(|candidate| {
                        candidate.id() == device && candidate.state().type_id() == type_id
                    }))
                    || state.project.return_tracks().iter().any(|track| {
                        class == hex_engine::NativeDeviceClass::Effect
                            && track.devices().iter().any(|candidate| {
                                candidate.id() == device && candidate.state().type_id() == type_id
                            })
                    })
            }
        }
    }

    pub(super) fn clear_native_device_drag(&mut self, drag: &NativeDeviceDrag) {
        if self
            .active_native_device_drag
            .as_ref()
            .is_some_and(|active| {
                active.project_generation == drag.project_generation
                    && active.revision == drag.revision
                    && active.source == drag.source
                    && Rc::ptr_eq(&active.destination, &drag.destination)
            })
        {
            self.arrangement_device_preview = None;
            self.active_native_device_drag = None;
            self.device_panel.drag_copy = None;
        }
        drag.destination.set(None);
        drag.return_destination.set(None);
    }

    pub(super) fn cancel_active_native_device_drag(&mut self) -> bool {
        self.arrangement_device_preview = None;
        let Some(drag) = self.active_native_device_drag.take() else {
            return false;
        };
        self.device_panel.drag_copy = None;
        drag.destination.set(None);
        drag.return_destination.set(None);
        true
    }

    pub(super) fn browser_audio_drag_is_current(&self, drag: &BrowserAudioDrag) -> bool {
        if !self
            .active_browser_audio_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.destination, &drag.destination))
            || self.opening_project
            || self.close_prompt_open
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || drag
                .browser_generation
                .is_some_and(|generation| self.browser.scan_generation() != generation)
        {
            return false;
        }
        self.state.as_ref().is_some_and(|state| {
            let viewport = self.visible_viewport(state.timeline_frames);
            self.project_generation == drag.project_generation
                && state.revision == drag.revision
                && state.project.sample_rate() == drag.sample_rate
                && viewport.start == drag.viewport().start
                && viewport.length == drag.viewport().length
                && self.snap_grid == drag.snap_grid
        })
    }

    fn painted_arrangement_timeline_at(&self, x: f32, y: f32) -> Option<LaneBounds> {
        let body = self
            .arrangement_bounds
            .filter(|_| self.arrangement_bounds_epoch == self.drag_layout_generation)?;
        let timeline = LaneBounds {
            left: body.left,
            width: body.width - self.track_header_width(),
            top: body.top,
            height: body.height,
        };
        lane_contains_pointer(x, y, timeline).then_some(timeline)
    }

    // Use the last painted full row, including attached automation lanes.
    // A virtualized or stale last row cannot claim the trailing hit region.
    fn painted_arrangement_tail_boundary(&self, pointer_y: f32) -> Option<f32> {
        let state = self.state.as_ref()?;
        let Some(index) = self.arrangement_track_indices().last() else {
            return Some(pointer_y);
        };
        let last = state.project.tracks().get(*index)?;
        let row = self.painted_track_drop_lane(last.id())?;
        let bottom = row.top + row.height;
        (pointer_y >= bottom).then_some(bottom)
    }

    pub(super) fn painted_arrangement_tail(&self, x: f32, y: f32) -> Option<LaneBounds> {
        let timeline = self.painted_arrangement_timeline_at(x, y)?;
        self.painted_arrangement_tail_boundary(y)?;
        Some(timeline)
    }

    pub(super) fn painted_track_insertion_target(
        &self,
        pointer_x: f32,
        pointer_y: f32,
    ) -> Option<(Option<TrackId>, LaneBounds, f32)> {
        let timeline = self.painted_arrangement_timeline_at(pointer_x, pointer_y)?;
        let state = self.state.as_ref()?;
        let tracks = self.arrangement_track_indices();
        if tracks.is_empty() {
            return Some((None, timeline, pointer_y));
        }

        let first = state.project.tracks().get(*tracks.first()?)?;
        if self.drag_lane_epochs.get(&first.id()) == Some(&self.drag_layout_generation) {
            let first_lane = self.painted_track_drop_lane(first.id())?;
            // Reserve the same narrow insertion band at the first row's top,
            // without adding empty layout space above the tracks.
            if first_lane.top >= timeline.top && pointer_y <= first_lane.top + 3.0 {
                return Some((Some(first.id()), timeline, first_lane.top));
            }
        }

        for index in 1..tracks.len() {
            let previous = state.project.tracks().get(*tracks.get(index - 1)?)?;
            let next = state.project.tracks().get(*tracks.get(index)?)?;
            if self.drag_lane_epochs.get(&previous.id()) != Some(&self.drag_layout_generation)
                || self.drag_lane_epochs.get(&next.id()) != Some(&self.drag_layout_generation)
            {
                continue;
            }
            let previous_lane = self.painted_track_drop_lane(previous.id())?;
            let next_lane = self.painted_track_drop_lane(next.id())?;
            let boundary = (previous_lane.top + previous_lane.height + next_lane.top) * 0.5;
            if (pointer_y - boundary).abs() <= 3.0 {
                return Some((Some(next.id()), timeline, boundary));
            }
        }

        self.painted_arrangement_tail_boundary(pointer_y)
            .map(|bottom| (None, timeline, bottom))
    }

    pub(super) fn browser_audio_drop_position(
        &self,
        drag: &BrowserAudioDrag,
        pointer_x: f32,
        pointer_y: f32,
    ) -> (Option<DropPreview>, bool) {
        if !self.browser_audio_drag_is_current(drag) {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, false);
        }
        if self.mode == super::EditorMode::Session {
            // Console receivers own their stable cell/scene target. Browser
            // row moves must not project into retained Arrangement geometry.
            drag.insertion_line_y.set(None);
            return (None, false);
        }
        let Some(state) = self.state.as_ref() else {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, true);
        };
        if let Some((before_track, timeline, line_y)) =
            self.painted_track_insertion_target(pointer_x, pointer_y)
        {
            let Some(start) = browser_start(
                &state.project,
                drag.viewport().start,
                drag.viewport().length,
                drag.snap_grid
                    .with_platform_modifier(drag.snap_modifier.get()),
                pointer_x,
                timeline,
            ) else {
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                return (None, true);
            };
            drag.destination.set(Some(BrowserDropDestination::NewTrack {
                before_track,
                start,
            }));
            drag.insertion_line_y.set(Some(line_y));
            return (None, true);
        }
        if let Some((target, lane)) = self.hovered_drag_target(pointer_x, pointer_y) {
            let Some(start) = browser_start(
                &state.project,
                drag.viewport().start,
                drag.viewport().length,
                drag.snap_grid
                    .with_platform_modifier(drag.snap_modifier.get()),
                pointer_x,
                lane,
            ) else {
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                return (None, true);
            };
            drag.destination
                .set(Some(BrowserDropDestination::ExistingTrack {
                    track: target,
                    start,
                }));
            drag.insertion_line_y.set(None);
            let preview = drag
                .metadata
                .borrow()
                .clone()
                .filter(|metadata| metadata.sample_rate == state.project.sample_rate())
                .and_then(|metadata| {
                    browser_candidate(BrowserDropCandidate {
                        project: &state.project,
                        viewport_start: drag.viewport().start,
                        viewport_length: drag.viewport().length,
                        snap_grid: drag.snap_grid,
                        snap_modifier: drag.snap_modifier.get(),
                        pointer_x,
                        lane,
                        duration: metadata.playback_frames,
                        target_track: target,
                    })
                });
            return (preview, true);
        }
        drag.destination.set(None);
        drag.insertion_line_y.set(None);
        (None, false)
    }

    pub(super) fn refresh_browser_audio_drag(&mut self, drag: &BrowserAudioDrag) {
        if !self.browser_audio_drag_is_current(drag) {
            drag.preview.set(None);
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            drag.lane_hovered.set(false);
            return;
        }
        if self.mode == super::EditorMode::Session {
            if let Some(BrowserDropDestination::Console(target)) = drag.destination.get()
                && !self.console_target_exists(target)
            {
                drag.destination.set(None);
            }
            return;
        }
        let Some((pointer_x, pointer_y)) = drag.last_pointer.get() else {
            return;
        };
        let (preview, lane_hovered) = self.browser_audio_drop_position(drag, pointer_x, pointer_y);
        drag.preview.set(preview);
        drag.lane_hovered.set(lane_hovered);
    }

    pub(super) fn clear_browser_audio_drag(&mut self, drag: &BrowserAudioDrag) {
        if self
            .active_browser_audio_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.destination, &drag.destination))
        {
            self.active_browser_audio_drag = None;
        }
        drag.preview.set(None);
        drag.destination.set(None);
        drag.insertion_line_y.set(None);
        drag.lane_hovered.set(false);
        drag.last_pointer.set(None);
        drag.scroll.set(Default::default());
    }

    pub(super) fn cancel_active_browser_audio_drag(&mut self) -> bool {
        let Some(drag) = self.active_browser_audio_drag.clone() else {
            return false;
        };
        self.clear_browser_audio_drag(&drag);
        true
    }

    pub(super) fn drop_browser_audio(
        &mut self,
        drag: &BrowserAudioDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        drag.snap_modifier.set(window.modifiers().platform);
        drag.copy_modifier.set(window.modifiers().alt);
        let current = self.browser_audio_drag_is_current(drag);
        let (preview, lane_hovered) = self.browser_audio_drop_position(
            drag,
            f32::from(window.mouse_position().x),
            f32::from(window.mouse_position().y),
        );
        drag.preview.set(preview);
        drag.lane_hovered.set(lane_hovered);
        if !current {
            self.active_browser_audio_drag = None;
            cx.notify();
            return;
        }
        let destination = drag.destination.get();
        if destination.is_none() {
            self.active_browser_audio_drag = None;
            cx.notify();
            return;
        }
        let metadata_valid = drag.metadata.borrow().as_ref().is_none_or(|metadata| {
            metadata.sample_rate == drag.sample_rate
                && match destination {
                    Some(BrowserDropDestination::ExistingTrack { .. }) => preview.is_some(),
                    Some(BrowserDropDestination::NewTrack { start, .. }) => {
                        metadata.playback_frames > 0
                            && start.checked_add(metadata.playback_frames).is_some()
                    }
                    Some(BrowserDropDestination::Console(_)) | None => false,
                }
        });
        if !metadata_valid {
            self.active_browser_audio_drag = None;
            self.error = Some("audio drop canceled · preview duration is invalid".into());
            cx.notify();
            return;
        }
        let Some(destination) = destination else {
            self.active_browser_audio_drag = None;
            cx.notify();
            return;
        };
        let request = match destination {
            BrowserDropDestination::Console(_) => {
                self.clear_browser_audio_drag(drag);
                self.error = Some("sample drop canceled · choose a console cell".into());
                cx.notify();
                return;
            }
            BrowserDropDestination::ExistingTrack { track, start } => {
                Request::ImportToTrackWithOverwrite {
                    path: drag.path.clone(),
                    track,
                    start,
                }
            }
            BrowserDropDestination::NewTrack {
                before_track,
                start,
            } => Request::ImportToNewTrack {
                path: drag.path.clone(),
                before_track,
                start,
            },
        };
        if self.enqueue_sample_drop(request, drag, destination) {
            self.stop_browser_preview_for_drop();
            if self.recovery.deferred_edit.is_none() {
                self.status = "importing sample to arrangement".into();
            }
        }
        self.clear_browser_audio_drag(drag);
        cx.notify();
    }

    pub(super) fn drop_clip(
        &mut self,
        drag: &ClipDrag,
        target_track: TrackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        drag.snap_modifier.set(window.modifiers().platform);
        drag.copy_modifier.set(window.modifiers().alt);
        if !self
            .active_clip_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.group, &drag.group))
        {
            return;
        }
        let clear = |view: &mut Self| view.clear_clip_drag(drag.project_generation, drag.clip);
        if self.opening_project || self.close_prompt_open {
            clear(self);
            return;
        }
        let pointer = window.mouse_position();
        let pointer = (f32::from(pointer.x), f32::from(pointer.y));
        if self
            .painted_track_insertion_target(pointer.0, pointer.1)
            .is_some()
        {
            if self.worker_requests_pending != 0 || !self.pending_requests.is_empty() {
                self.error = Some("clip move canceled · project work pending".into());
                clear(self);
                cx.notify();
                return;
            }
            if self.drag_bounce_held {
                self.error = Some("bounce drop needs an existing track".into());
                clear(self);
                cx.notify();
                return;
            }
            self.drop_clip_to_new_track(drag, window, cx);
            return;
        }
        if self.arrangement_bounds_epoch != self.drag_layout_generation
            || self.drag_lane_epochs.get(&target_track) != Some(&self.drag_layout_generation)
        {
            clear(self);
            cx.notify();
            return;
        }
        let Some(state) = self.state.as_ref() else {
            clear(self);
            return;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        if drag.grab_offset.get().is_none() {
            self.error = Some("clip move canceled · pointer position unavailable".into());
            clear(self);
            cx.notify();
            return;
        }
        let Some(lane) = self.drag_lanes.get(&target_track).copied() else {
            self.error = Some("clip move canceled · target track unavailable".into());
            clear(self);
            cx.notify();
            return;
        };
        let Some(preview) = self.drag_candidate(
            drag,
            target_track,
            f32::from(window.mouse_position().x),
            lane,
        ) else {
            self.error = Some("clip move canceled · invalid timeline position".into());
            clear(self);
            cx.notify();
            return;
        };
        let drop_state = DropState {
            project_generation: self.project_generation,
            revision: state.revision,
            project: &state.project,
            target_track,
            position: preview.position,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            snap_grid: self.snap_grid,
        };
        if !(if drag.group.borrow().ids.len() > 1 {
            self.drag_start_is_current(drag) && context_is_current(drag, &drop_state)
        } else {
            is_current(drag, drop_state)
        }) {
            self.error = Some("clip move canceled · project changed".into());
            clear(self);
            cx.notify();
            return;
        }
        drag.destination
            .set(Some(ClipDropDestination::ExistingTrack(target_track)));
        if std::mem::take(&mut self.drag_bounce_held) {
            self.drop_clip_bounce(drag, target_track, preview);
            cx.notify();
            return;
        }
        if is_noop(drag, &drop_state) {
            clear(self);
            return;
        }
        let command = if drag.alias_copy() {
            Command::DuplicateAliasClipsAt {
                clips: drag.group.borrow().ids.clone(),
                anchor: drag.clip,
                track: target_track,
                position: group_position(preview.position),
            }
        } else if drag.group.borrow().ids.len() > 1 {
            Command::MoveClipsWithOverwrite {
                clips: drag.group.borrow().ids.clone(),
                anchor: drag.clip,
                track: target_track,
                position: group_position(preview.position),
                copy: drag.copy_modifier.get(),
            }
        } else if drag.copy_modifier.get() {
            copy_command(drag.clip, target_track, preview.position)
        } else {
            move_command(drag.clip, target_track, preview.position)
        };
        let copy_intent = super::group_copy::Pending::capture(self, &command);
        // Input capture ends here. Keep the frozen destination for moves AND
        // copies until the shared receipt observes this edit's real terminal.
        drag.preview.set(Some(preview));
        self.pending_clip_move = self.active_clip_drag.take().map(|drag| {
            super::pending_clip_move::Pending::new(
                drag,
                command.clone(),
                std::sync::Arc::clone(&state.project),
                self.selection_intent_epoch,
            )
        });
        let before_pending = self.worker_requests_pending;
        if self.enqueue(Request::MoveExistingClips {
            project_generation: drag.project_generation,
            revision: drag.revision,
            command,
        }) {
            if let Some(mut intent) = copy_intent {
                if self.worker_requests_pending > before_pending {
                    intent.submitted(self.worker_requests_pending);
                }
                self.pending_group_copy = Some(intent);
            }
        } else {
            self.retire_pending_clip_move();
            clear(self);
        }
        cx.notify();
    }
}

impl super::EditorView {
    pub(super) fn refresh_drag_modifiers(
        &mut self,
        held: bool,
        copy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.active_clip_drag.clone() {
            let snap_changed = drag.snap_modifier.replace(held) != held;
            let copy_changed = drag.copy_modifier.replace(copy) != copy;
            cx.set_active_drag_cursor_style(copy_cursor(copy), window);
            if (snap_changed || copy_changed)
                && let Some((x, y)) = drag.last_pointer.get()
            {
                let (preview, lane_hovered) = self.clip_drag_drop_position(&drag, x, y);
                drag.preview.set(preview);
                drag.lane_hovered.set(lane_hovered);
                cx.notify();
            }
        }
        if let Some(drag) = self.active_browser_audio_drag.clone() {
            let snap_changed = drag.snap_modifier.replace(held) != held;
            let copy_changed = drag.copy_modifier.replace(copy) != copy;
            cx.set_active_drag_cursor_style(copy_cursor(copy), window);
            if (snap_changed || copy_changed)
                && let Some((x, y)) = drag.last_pointer.get()
            {
                let (preview, lane_hovered) = self.browser_audio_drop_position(&drag, x, y);
                drag.preview.set(preview);
                drag.lane_hovered.set(lane_hovered);
                cx.notify();
            }
        }
    }
}

pub(super) fn copy_cursor(copy: bool) -> gpui::CursorStyle {
    if copy {
        gpui::CursorStyle::DragCopy
    } else {
        gpui::CursorStyle::Arrow
    }
}

impl ClipDrag {
    pub(super) fn viewport(&self) -> Viewport {
        self.scroll.get().viewport.unwrap_or(Viewport {
            start: self.viewport_start,
            length: self.viewport_length,
        })
    }
}

impl BrowserAudioDrag {
    pub(super) fn viewport(&self) -> Viewport {
        self.scroll.get().viewport.unwrap_or(Viewport {
            start: self.viewport_start,
            length: self.viewport_length,
        })
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::{
        ClipDrag, ClipPosition, DropState, LaneBounds, PointerDown, SnapGrid, candidate_position,
        drop_start, grab_offset_from_pointer, is_current, is_noop, move_command, preview_for,
        take_pointer_down,
    };
    use hex_project::{BeatTime, Command, Created, Editor, Project, Tempo, TrackId, TrimEdge};
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn off_grid_original_anchor_precedes_grid_for_cross_track_moves() {
        let lane = LaneBounds {
            left: 0.0,
            width: 480.0,
            top: 0.0,
            height: 44.0,
        };
        for edge in [super::ClipSnapEdge::Leading, super::ClipSnapEdge::Trailing] {
            let (mut editor, source, target, clip) = fixture();
            editor
                .execute(Command::MoveClip {
                    clip,
                    track: source,
                    start: 15_137,
                })
                .unwrap();
            let mut payload = drag(source, clip);
            payload.position = ClipPosition::Audio {
                start: 15_137,
                duration: 100,
            };
            payload.viewport_length = 48_000;
            payload.grab_offset.set(Some(0));
            payload.snap_grid = SnapGrid::Quarter;
            payload.snap_edge.set(Some(edge));
            assert_eq!(
                candidate_position(editor.project(), &payload, Some(target), 153.37, lane),
                Some(payload.position)
            );

            let (mut editor, source, target, clip) = midi_fixture();
            let start = BeatTime::from_raw(BeatTime::ONE.raw() * 4 + 137);
            editor
                .execute(Command::MoveMidiClip {
                    clip,
                    track: source,
                    start,
                })
                .unwrap();
            let rounded = editor
                .project()
                .tempo_map()
                .frame_to_beat(editor.project().tempo_map().beat_to_frame(start).unwrap())
                .unwrap();
            assert_ne!(rounded, start, "fixture contains a subframe beat position");
            let mut payload = midi_drag(source, clip);
            payload.position = ClipPosition::Midi {
                start,
                length: BeatTime::from_quarters(4).unwrap(),
            };
            payload.grab_offset.set(Some(0));
            payload.snap_edge.set(Some(edge));
            assert_eq!(
                candidate_position(editor.project(), &payload, Some(target), 161.0, lane),
                Some(payload.position)
            );
        }
    }

    #[test]
    fn origin_capture_refuses_overflowing_captured_extents() {
        let lane = LaneBounds {
            left: 0.0,
            width: 480.0,
            top: 0.0,
            height: 44.0,
        };
        let (editor, source, target, clip) = fixture();
        let mut payload = drag(source, clip);
        payload.position = ClipPosition::Audio {
            start: u64::MAX,
            duration: 100,
        };
        payload.viewport_start = u64::MAX;
        payload.viewport_length = 1;
        payload.grab_offset.set(Some(0));
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(target), 0.0, lane),
            None
        );
        let (editor, source, target, clip) = midi_fixture();
        let mut payload = midi_drag(source, clip);
        payload.position = ClipPosition::Midi {
            start: BeatTime::from_raw(u64::MAX),
            length: BeatTime::ONE,
        };
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(target), 0.0, lane),
            None
        );
    }

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

    fn midi_fixture() -> (Editor, TrackId, TrackId, hex_project::ClipId) {
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
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(4).expect("tempo beat"),
                tempo: Tempo::new(60_000_000).expect("tempo"),
            })
            .expect("tempo point");
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: source,
                start: BeatTime::from_quarters(4).expect("clip beat"),
                length: BeatTime::from_quarters(4).expect("clip length"),
            })
            .expect("midi clip")
        else {
            panic!("midi clip id");
        };
        (editor, source, target, clip)
    }

    fn drag(source: TrackId, clip: hex_project::ClipId) -> ClipDrag {
        ClipDrag {
            group: Default::default(),
            project_generation: 3,
            revision: 9,
            source_track: source,
            clip,
            position: ClipPosition::Audio {
                start: 40,
                duration: 100,
            },
            grab_offset: Rc::new(Cell::new(Some(50))),
            snap_edge: Rc::new(Cell::new(Some(super::ClipSnapEdge::Leading))),
            source_span: Default::default(),
            capture: Rc::new(Cell::new(None)),
            viewport_start: 0,
            viewport_length: 1_000,
            scroll: Default::default(),
            snap_grid: SnapGrid::Off,
            snap_modifier: Rc::new(Cell::new(false)),
            copy_modifier: Rc::new(Cell::new(false)),
            preview: Rc::new(Cell::new(None)),
            destination: Rc::new(Cell::new(None)),
            insertion_line_y: Rc::new(Cell::new(None)),
            ghost_label: "audio clip".into(),
            ghost_waveform: None,
            lane_hovered: Rc::new(Cell::new(false)),
            last_pointer: Rc::new(Cell::new(None)),
        }
    }

    fn midi_drag(source: TrackId, clip: hex_project::ClipId) -> ClipDrag {
        ClipDrag {
            group: Default::default(),
            project_generation: 3,
            revision: 9,
            source_track: source,
            clip,
            position: ClipPosition::Midi {
                start: BeatTime::from_quarters(4).expect("clip beat"),
                length: BeatTime::from_quarters(4).expect("clip length"),
            },
            grab_offset: Rc::new(Cell::new(Some(BeatTime::ONE.raw()))),
            snap_edge: Rc::new(Cell::new(Some(super::ClipSnapEdge::Leading))),
            source_span: Default::default(),
            capture: Rc::new(Cell::new(None)),
            viewport_start: 0,
            viewport_length: 288_000,
            scroll: Default::default(),
            snap_grid: SnapGrid::Quarter,
            snap_modifier: Rc::new(Cell::new(false)),
            copy_modifier: Rc::new(Cell::new(false)),
            preview: Rc::new(Cell::new(None)),
            destination: Rc::new(Cell::new(None)),
            insertion_line_y: Rc::new(Cell::new(None)),
            ghost_label: "midi clip".into(),
            ghost_waveform: None,
            lane_hovered: Rc::new(Cell::new(false)),
            last_pointer: Rc::new(Cell::new(None)),
        }
    }

    fn drop_state<'a>(
        project: &'a Project,
        target_track: TrackId,
        position: ClipPosition,
    ) -> DropState<'a> {
        DropState {
            project_generation: 3,
            revision: 9,
            project,
            target_track,
            position,
            viewport_start: 0,
            viewport_length: 1_000,
            snap_grid: SnapGrid::Off,
        }
    }

    #[test]
    fn move_grab_offset_uses_mouse_down_instead_of_threshold_crossing_position() {
        let (_, _, _, clip) = fixture();
        let mut pending = PointerDown::begin(
            super::ClipPointerKey {
                generation: 3,
                clip,
            },
            hex_widgets::RectanglePart::Body,
            790.0,
            true,
        );
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
                    top: 0.0,
                    height: 44.0,
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
        let mut pending = PointerDown::begin(
            super::ClipPointerKey {
                generation: 3,
                clip,
            },
            hex_widgets::RectanglePart::Trailing,
            790.0,
            true,
        );
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
    fn trailing_move_snaps_the_end_without_changing_audio_or_midi_length() {
        let (editor, source, _, clip) = fixture();
        let mut payload = drag(source, clip);
        payload.viewport_length = 48_000;
        payload.grab_offset.set(Some(0));
        payload.snap_grid = SnapGrid::Quarter;
        payload.snap_edge.set(Some(super::ClipSnapEdge::Trailing));
        let lane = LaneBounds {
            left: 0.0,
            width: 480.0,
            top: 0.0,
            height: 44.0,
        };
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(source), 150.0, lane),
            Some(ClipPosition::Audio {
                start: 23_900,
                duration: 100
            })
        );
        payload.snap_modifier.set(true);
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(source), 150.0, lane),
            Some(ClipPosition::Audio {
                start: 15_000,
                duration: 100
            })
        );
        // The original is only 0.4 px from zero and now outranks the grid.
        payload.snap_modifier.set(false);
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(source), 0.0, lane),
            Some(ClipPosition::Audio {
                start: 40,
                duration: 100
            })
        );
        payload.snap_modifier.set(true);
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(source), 0.0, lane),
            Some(ClipPosition::Audio {
                start: 0,
                duration: 100
            })
        );
        let (editor, source, _, clip) = midi_fixture();
        let mut payload = midi_drag(source, clip);
        payload.viewport_length = 480_000;
        payload.grab_offset.set(Some(0));
        payload.snap_grid = SnapGrid::Quarter;
        payload.snap_edge.set(Some(super::ClipSnapEdge::Trailing));
        let length = BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 2);
        payload.position = ClipPosition::Midi {
            start: BeatTime::from_quarters(4).unwrap(),
            length,
        };
        for (pointer, start) in [
            (120.0, BeatTime::ONE.raw() * 9 / 2),
            (72.0, BeatTime::ONE.raw() * 7 / 2),
        ] {
            assert_eq!(
                candidate_position(editor.project(), &payload, Some(source), pointer, lane),
                Some(ClipPosition::Midi {
                    start: BeatTime::from_raw(start),
                    length
                })
            );
        }
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
            top: 0.0,
            height: 44.0,
        };
        // 150 px maps to frame 15,000 (0.625 quarter at 120 bpm); the
        // canonical quarter grid maps that candidate to 24,000.
        let preview = candidate_position(editor.project(), &payload, Some(source), 150.0, lane);
        let release = candidate_position(editor.project(), &payload, Some(source), 150.0, lane);
        assert_eq!(
            preview,
            Some(ClipPosition::Audio {
                start: 24_000,
                duration: 100
            })
        );
        assert_eq!(release, preview);
        assert_eq!(
            preview.and_then(|position| preview_for(editor.project(), source, position)),
            Some(super::DropPreview {
                target_track: source,
                start_frame: 24_000,
                end_frame: 24_100,
                position: ClipPosition::Audio {
                    start: 24_000,
                    duration: 100
                },
            })
        );

        payload.snap_grid = SnapGrid::Off;
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(source), 150.0, lane),
            Some(ClipPosition::Audio {
                start: 15_000,
                duration: 100
            })
        );
        payload.snap_grid = SnapGrid::Quarter;
        payload.viewport_start = u64::MAX;
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(source), 480.0, lane),
            None
        );
    }

    #[test]
    fn midi_preview_and_drop_keep_beat_anchor_and_resolve_tempo_mapped_width() {
        let (editor, source, target, clip) = midi_fixture();
        let mut payload = midi_drag(source, clip);
        let lane = LaneBounds {
            left: 0.0,
            width: 480.0,
            top: 100.0,
            height: 44.0,
        };
        // The center is beat five in the slower second tempo section. Keeping
        // the one-beat original grab offset places the clip at beat four.
        let preview_position =
            candidate_position(editor.project(), &payload, Some(target), 240.0, lane);
        let release_position =
            candidate_position(editor.project(), &payload, Some(target), 240.0, lane);
        let expected = ClipPosition::Midi {
            start: BeatTime::from_quarters(4).expect("start"),
            length: BeatTime::from_quarters(4).expect("length"),
        };
        assert_eq!(preview_position, Some(expected));
        assert_eq!(release_position, preview_position);
        assert_eq!(
            preview_for(editor.project(), target, expected),
            Some(super::DropPreview {
                target_track: target,
                start_frame: 96_000,
                end_frame: 288_000,
                position: expected,
            })
        );
        assert!(matches!(
            move_command(clip, target, expected),
            Command::MoveMidiClipWithOverwrite {
                clip: moved_clip,
                track,
                start
            } if moved_clip == clip && track == target && start == BeatTime::from_quarters(4).expect("start")
        ));
        assert!(is_current(
            &payload,
            DropState {
                viewport_length: 288_000,
                snap_grid: SnapGrid::Quarter,
                ..drop_state(editor.project(), target, expected)
            }
        ));
        payload.snap_grid = SnapGrid::Off;
        payload.capture.set(PointerDown::begin(
            super::ClipPointerKey {
                generation: payload.project_generation,
                clip: payload.clip,
            },
            hex_widgets::RectanglePart::Body,
            120.0,
            true,
        ));
        assert_eq!(
            candidate_position(editor.project(), &payload, Some(source), 120.25, lane),
            Some(expected),
            "returning to the original grab point preserves the exact beat anchor"
        );
    }

    #[test]
    fn stale_project_revision_viewport_and_missing_ids_reject_a_drop() {
        let (mut editor, source, target, clip) = fixture();
        let payload = drag(source, clip);
        let valid = drop_state(
            editor.project(),
            target,
            ClipPosition::Audio {
                start: 200,
                duration: 100,
            },
        );
        assert!(is_current(&payload, valid));

        let mut stale_generation = drop_state(
            editor.project(),
            target,
            ClipPosition::Audio {
                start: 200,
                duration: 100,
            },
        );
        stale_generation.project_generation += 1;
        assert!(!is_current(&payload, stale_generation));

        let mut stale_revision = drop_state(
            editor.project(),
            target,
            ClipPosition::Audio {
                start: 200,
                duration: 100,
            },
        );
        stale_revision.revision += 1;
        assert!(!is_current(&payload, stale_revision));

        let mut stale_snap = drop_state(
            editor.project(),
            target,
            ClipPosition::Audio {
                start: 200,
                duration: 100,
            },
        );
        stale_snap.snap_grid = SnapGrid::Quarter;
        assert!(!is_current(&payload, stale_snap));

        let mut stale_viewport = drop_state(
            editor.project(),
            target,
            ClipPosition::Audio {
                start: 200,
                duration: 100,
            },
        );
        stale_viewport.viewport_start += 1;
        assert!(!is_current(&payload, stale_viewport));

        editor
            .execute(Command::RemoveTrack(source))
            .expect("remove source");
        assert!(!is_current(
            &payload,
            drop_state(
                editor.project(),
                target,
                ClipPosition::Audio {
                    start: 200,
                    duration: 100
                }
            )
        ));

        let (mut editor, source, target, clip) = fixture();
        let payload = drag(source, clip);
        editor
            .execute(Command::RemoveClip(clip))
            .expect("remove source clip");
        assert!(!is_current(
            &payload,
            drop_state(
                editor.project(),
                target,
                ClipPosition::Audio {
                    start: 200,
                    duration: 100
                }
            )
        ));

        let (mut editor, source, target, clip) = fixture();
        let payload = drag(source, clip);
        editor
            .execute(Command::RemoveTrack(target))
            .expect("remove destination");
        assert!(!is_current(
            &payload,
            drop_state(
                editor.project(),
                target,
                ClipPosition::Audio {
                    start: 200,
                    duration: 100
                }
            )
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
        let mut replaced_project = drop_state(
            second.project(),
            second_target,
            ClipPosition::Audio {
                start: 200,
                duration: 100,
            },
        );
        replaced_project.project_generation += 1;
        assert!(!is_current(&payload, replaced_project));
    }

    #[test]
    fn unchanged_drop_is_classified_as_noop_without_becoming_stale() {
        let (editor, source, _, clip) = fixture();
        let payload = drag(source, clip);
        let state = drop_state(editor.project(), source, payload.position);
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

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "group_drag_ui_test.rs"]
pub(super) mod group_drag_ui_test;
