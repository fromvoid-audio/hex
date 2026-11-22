//! Pure clip-drag geometry, snapping, preview, and command planning.
use super::super::musical_time::SnapGrid;
use super::super::viewport::Viewport;
#[cfg(all(test, feature = "ui-unit-tier"))]
use super::geometry::grab_offset_from_pointer;
use super::geometry::{ClipSnapEdge, LaneBounds, drop_start};
use hex_project::{BeatTime, ClipId, Command, Project, TrackId, TrimEdge};
use hex_widgets::{RectangleCapture, RectanglePart};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Default, Debug)]
pub(crate) struct ClipDragGroup {
    pub(crate) selection_epoch: Option<u64>,
    pub(crate) ids: Vec<ClipId>,
    pub(super) capture: Option<hex_project::ClipGroupMovePreview>,
}

impl ClipDragGroup {
    pub(crate) fn previews(&self) -> &[hex_project::ClipGroupPreview] {
        self.capture
            .as_ref()
            .map_or(&[], |capture| capture.previews())
    }
    pub(crate) fn invalidate(&mut self) {
        if let Some(capture) = self.capture.as_mut() {
            capture.invalidate();
        }
    }
    pub(crate) fn original_audio<'a>(
        &self,
        project: &'a std::sync::Arc<Project>,
        index: usize,
    ) -> Option<&'a hex_project::Clip> {
        self.capture.as_ref()?.original_audio(project, index)
    }
}

/// Control-side custody for a frozen selection and its latest held target.
pub(crate) struct MoveAudition {
    pub(super) clips: std::sync::Arc<[ClipId]>,
    pub(super) gesture: u64,
    pub(super) sequence: u64,
    pub(super) new_track_defaults: Option<(std::sync::Arc<str>, f32)>,
    pub(crate) target: Option<hex_session::service::ArrangementMoveAudition>,
}

/// Bounded, metadata-only payload for one native pointer drag.
#[derive(Clone, Debug)]
pub(crate) struct ClipDrag {
    pub(crate) project_generation: u64,
    pub(crate) revision: u64,
    pub(crate) source_track: TrackId,
    pub(crate) clip: ClipId,
    pub(crate) group: Rc<RefCell<ClipDragGroup>>,
    pub(crate) position: ClipPosition,
    pub(crate) grab_offset: Rc<Cell<Option<u64>>>,
    pub(crate) snap_edge: Rc<Cell<Option<ClipSnapEdge>>>,
    /// Last completed resting layout, before trim handles narrow the move hit
    /// surface. Used only while admitting the original press.
    pub(crate) source_span: Rc<Cell<Option<(f32, f32)>>>,
    pub(crate) capture: Rc<Cell<Option<PointerDown>>>,
    pub(crate) viewport_start: u64,
    pub(crate) viewport_length: u64,
    pub(crate) scroll: super::super::drag_autoscroll::scroll_state::DragScroll,
    pub(crate) snap_grid: SnapGrid,
    /// Command bypasses both original-position capture and the move grid.
    pub(crate) snap_modifier: Rc<Cell<bool>>,
    pub(crate) copy_modifier: Rc<Cell<bool>>,
    pub(crate) preview: Rc<Cell<Option<DropPreview>>>,
    pub(crate) destination: Rc<Cell<Option<ClipDropDestination>>>,
    pub(crate) insertion_line_y: Rc<Cell<Option<f32>>>,
    pub(crate) ghost_label: String,
    pub(crate) ghost_waveform: Option<std::sync::Arc<hex_session::CachedWaveform>>,
    pub(crate) lane_hovered: Rc<Cell<bool>>,
    pub(crate) last_pointer: Rc<Cell<Option<(f32, f32)>>>,
}

impl ClipDrag {
    pub(crate) fn alias_copy(&self) -> bool {
        self.copy_modifier.get() && self.snap_modifier.get()
    }

    /// The grabbed clip supplies the cursor for both single and group drags.
    pub(crate) fn cursor_position(&self) -> Option<(Option<TrackId>, u64)> {
        match self.destination.get()? {
            ClipDropDestination::ExistingTrack(track) => {
                let preview = self.preview.get().filter(|p| p.target_track == track)?;
                Some((Some(track), preview.start_frame))
            }
            ClipDropDestination::NewTrack { start_frame, .. } => Some((None, start_frame)),
        }
    }

    pub(crate) fn retire(&self) {
        self.preview.set(None);
        self.group.borrow_mut().capture = None;
        self.destination.set(None);
        self.insertion_line_y.set(None);
        self.lane_hovered.set(false);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipDropDestination {
    ExistingTrack(TrackId),
    NewTrack {
        before_track: Option<TrackId>,
        position: ClipPosition,
        start_frame: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipPosition {
    Audio { start: u64, duration: u64 },
    Midi { start: BeatTime, length: BeatTime },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DropPreview {
    pub(crate) target_track: TrackId,
    pub(crate) start_frame: u64,
    pub(crate) end_frame: u64,
    pub(crate) position: ClipPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipPointerKey {
    pub(crate) generation: u64,
    pub(crate) clip: ClipId,
}
/// The rectangle owns keyed input; the clip owner adds the edge chosen at the
/// original press. They move/cancel together across render replacement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PointerDown {
    rectangle: RectangleCapture<ClipPointerKey>,
    pub(crate) snap_edge: Option<ClipSnapEdge>,
}
impl PointerDown {
    pub(crate) fn begin(
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
    pub(crate) fn take_matching(
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
    pub(crate) fn origin_x(self) -> f32 {
        self.rectangle.origin_x
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(crate) fn part(self) -> RectanglePart {
        self.rectangle.part
    }
    pub(crate) fn update(
        &mut self,
        x: f32,
    ) -> Option<hex_widgets::RectangleIntent<ClipPointerKey>> {
        self.rectangle.update(x)
    }
    pub(crate) fn displaced(self, threshold: f32) -> bool {
        self.rectangle.displaced(threshold)
    }
    pub(crate) fn pointer(held: &Cell<Option<Self>>, x: f32) -> Option<f32> {
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
pub(crate) fn rectangle_part(edge: Option<TrimEdge>) -> RectanglePart {
    match edge {
        None => RectanglePart::Body,
        Some(TrimEdge::Left) => RectanglePart::Leading,
        Some(TrimEdge::Right) => RectanglePart::Trailing,
    }
}
#[cfg(all(test, feature = "ui-unit-tier"))]
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

pub(super) fn near_original_anchor(
    frame: u64,
    original: u64,
    viewport: Viewport,
    width: f32,
) -> bool {
    width.is_finite()
        && width > 0.0
        && viewport.length > 0
        && frame.abs_diff(original) as f64 / viewport.length as f64 * f64::from(width)
            <= ORIGINAL_POSITION_SNAP_PIXELS
}

pub(super) fn candidate_position(
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

pub(crate) fn preview_for(
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
pub(crate) struct DropState<'a> {
    pub(crate) project_generation: u64,
    pub(crate) revision: u64,
    pub(crate) project: &'a Project,
    pub(crate) target_track: TrackId,
    pub(crate) position: ClipPosition,
    pub(crate) viewport_start: u64,
    pub(crate) viewport_length: u64,
    pub(crate) snap_grid: SnapGrid,
}

pub(super) fn context_is_current(drag: &ClipDrag, state: &DropState<'_>) -> bool {
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

pub(crate) fn is_current(drag: &ClipDrag, state: DropState<'_>) -> bool {
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

pub(crate) fn is_noop(drag: &ClipDrag, state: &DropState<'_>) -> bool {
    drag.source_track == state.target_track && drag.position == state.position
}

pub(crate) fn group_position(position: ClipPosition) -> hex_project::ClipGroupPosition {
    match position {
        ClipPosition::Audio { start, .. } => hex_project::ClipGroupPosition::Audio(start),
        ClipPosition::Midi { start, .. } => hex_project::ClipGroupPosition::Midi(start),
    }
}

pub(super) fn move_command(clip: ClipId, target_track: TrackId, position: ClipPosition) -> Command {
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

pub(super) fn copy_command(clip: ClipId, target_track: TrackId, position: ClipPosition) -> Command {
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

pub(super) fn move_to_new_track_command(
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

pub(super) fn copy_to_new_track_command(
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

impl ClipDrag {
    pub(crate) fn viewport(&self) -> Viewport {
        self.scroll.get().viewport.unwrap_or(Viewport {
            start: self.viewport_start,
            length: self.viewport_length,
        })
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
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
