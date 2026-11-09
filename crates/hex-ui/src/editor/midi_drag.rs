//! Piano-roll note gesture geometry and immutable preview state.
use hex_project::{BeatTime, ClipId, NoteId};

const FREE_GATE_RAW: u64 = BeatTime::ONE.raw() / 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum MidiGrid {
    Quarter,
    Eighth,
    #[default]
    Sixteenth,
    ThirtySecond,
    Off,
}

impl MidiGrid {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Quarter => Self::Eighth,
            Self::Eighth => Self::Sixteenth,
            Self::Sixteenth => Self::ThirtySecond,
            Self::ThirtySecond => Self::Off,
            Self::Off => Self::Quarter,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::Off => "off",
        }
    }

    pub(super) const fn step_raw(self) -> Option<u64> {
        let beat = BeatTime::ONE.raw();
        match self {
            Self::Quarter => Some(beat),
            Self::Eighth => Some(beat / 2),
            Self::Sixteenth => Some(beat / 4),
            Self::ThirtySecond => Some(beat / 8),
            Self::Off => None,
        }
    }

    pub(super) const fn movement_step(self) -> u64 {
        match self.step_raw() {
            Some(step) => step,
            None => FREE_GATE_RAW,
        }
    }

    pub(super) const fn minimum_gate(self) -> u64 {
        match self.step_raw() {
            Some(step) => step,
            None => 1,
        }
    }
}

pub(super) const fn effective_grid(grid: MidiGrid, invert_snap: bool) -> MidiGrid {
    if !invert_snap {
        return grid;
    }
    match grid {
        MidiGrid::Off => MidiGrid::Sixteenth,
        _ => MidiGrid::Off,
    }
}

/// A dragged note end (held creation or trailing resize) is free by default;
/// the snap modifier temporarily snaps it to the grid (1/16 when the grid is
/// off). Starts, moves and leading resizes keep `effective_grid`'s inversion.
pub(super) const fn note_end_grid(grid: MidiGrid, snap_modifier: bool) -> MidiGrid {
    match (snap_modifier, grid) {
        (false, _) => MidiGrid::Off,
        (true, MidiGrid::Off) => MidiGrid::Sixteenth,
        (true, grid) => grid,
    }
}

pub(super) const fn default_gate_raw(remaining: u64, grid: MidiGrid) -> u64 {
    let desired = match grid.step_raw() {
        Some(step) => step,
        None => FREE_GATE_RAW,
    };
    if remaining < desired {
        remaining
    } else {
        desired
    }
}

pub(super) fn snap_raw(raw: u64, grid: MidiGrid) -> u64 {
    let Some(step) = grid.step_raw() else {
        return raw;
    };
    snap_clamped(i128::from(raw), 0, u64::MAX, step)
}

/// Creation starts at the grid line at or before the pointer. Move/resize
/// deltas retain their existing nearest-grid policy.
pub(super) fn creation_start(raw: u64, grid: MidiGrid) -> u64 {
    grid.step_raw().map_or(raw, |step| raw / step * step)
}

/// Free creation starts one logical pixel wide. The prepared playback minimum
/// prevents a positive beat duration rounding to zero audio frames at deep zoom.
pub(super) fn creation_gate(
    remaining: u64,
    grid: MidiGrid,
    span: u64,
    width: f32,
    minimum: u64,
) -> Option<u64> {
    if remaining == 0 || minimum == 0 || !width.is_finite() || width <= 0.0 || span == 0 {
        return None;
    }
    let desired = if grid.step_raw().is_some() {
        default_gate_raw(remaining, grid)
    } else {
        ((span as f64 / f64::from(width)).ceil() as u64)
            .max(1)
            .max(minimum)
    };
    let duration = desired.min(remaining);
    (duration >= minimum).then_some(duration)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GestureKind {
    Move {
        pointer_beat_raw: u64,
        pointer_pitch: u8,
    },
    ResizeLeading {
        pointer_beat_raw: u64,
        note_end: u64,
    },
    ResizeTrailing {
        pointer_beat_raw: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NotePreview {
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) pitch: u8,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct NoteGesture {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) clip: ClipId,
    pub(super) note: NoteId,
    pub(super) active: bool,
    pub(super) original: NotePreview,
    pub(super) channel: u8,
    pub(super) velocity: u8,
    pub(super) preview: NotePreview,
    /// Positive storage translation; displayed source start is preview.start - prefix.
    /// Only the canonical source-prefix command may publish this coordinate change.
    pub(super) source_prefix: u64,
    pub(super) kind: GestureKind,
    pub(super) bounds: [f32; 4],
    pub(super) beat_start: i128,
    pub(super) beat_span: u64,
    pub(super) clip_length: u64,
    pub(super) pitch_low: u8,
    pub(super) pitch_rows: u8,
    pub(super) grid: MidiGrid,
}

pub(super) struct GestureStart {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) clip: ClipId,
    pub(super) note: NoteId,
    pub(super) active: bool,
    pub(super) original: NotePreview,
    pub(super) channel: u8,
    pub(super) velocity: u8,
    pub(super) kind: HitKind,
    pub(super) pointer: [f32; 2],
    pub(super) bounds: [f32; 4],
    pub(super) beat_start: i128,
    pub(super) beat_span: u64,
    pub(super) clip_length: u64,
    pub(super) pitch_low: u8,
    pub(super) pitch_rows: u8,
    pub(super) grid: MidiGrid,
}

#[derive(Clone, Copy)]
pub(super) struct GestureContext {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) selected_clip: Option<ClipId>,
    pub(super) selected_note: Option<NoteId>,
    pub(super) clip_contains_note: bool,
    pub(super) pending: bool,
    pub(super) beat_start: i128,
    pub(super) beat_span: u64,
    pub(super) pitch_low: u8,
    pub(super) pitch_rows: u8,
    pub(super) grid: MidiGrid,
    pub(super) bounds: [f32; 4],
}

pub(super) fn is_current(gesture: &NoteGesture, context: GestureContext) -> bool {
    gesture.generation == context.generation
        && gesture.revision == context.revision
        && context.selected_clip == Some(gesture.clip)
        && context.selected_note == Some(gesture.note)
        && context.clip_contains_note
        && !context.pending
        && gesture.beat_start == context.beat_start
        && gesture.beat_span == context.beat_span
        && gesture.pitch_low == context.pitch_low
        && gesture.pitch_rows == context.pitch_rows
        && gesture.grid == context.grid
        && gesture.bounds == context.bounds
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HitKind {
    Move,
    ResizeLeading,
    ResizeTrailing,
}

pub(super) fn hit_kind(
    local_x: f32,
    left: f32,
    width: f32,
    leading_edge_visible: bool,
    trailing_edge_visible: bool,
) -> HitKind {
    if !local_x.is_finite() || !left.is_finite() || !width.is_finite() || width < 6.0 {
        return HitKind::Move;
    }
    let handle = 6.0_f32.min(width / 3.0);
    if leading_edge_visible && local_x <= left + handle {
        HitKind::ResizeLeading
    } else if trailing_edge_visible && local_x >= left + width - handle {
        HitKind::ResizeTrailing
    } else {
        HitKind::Move
    }
}

/// Tiny unselected halos stay easy to select/move. Once explicitly selected,
/// their left/right thirds expose the two real, visible note edges while the
/// middle third still moves. Hidden clipped edges never become resize handles.
pub(super) fn selected_hit_kind(
    local_x: f32,
    left: f32,
    width: f32,
    leading: bool,
    trailing: bool,
    selected: bool,
) -> HitKind {
    if selected
        && (3.0..6.0).contains(&width)
        && local_x.is_finite()
        && left.is_finite()
        && (left..=left + width).contains(&local_x)
    {
        let handle = width / 3.0;
        if leading && local_x <= left + handle {
            return HitKind::ResizeLeading;
        }
        if trailing && local_x >= left + width - handle {
            return HitKind::ResizeTrailing;
        }
    }
    hit_kind(local_x, left, width, leading, trailing)
}

pub(super) fn begin(start: GestureStart) -> Option<NoteGesture> {
    let pointer_beat_raw = beat_at_point(
        start.pointer[0],
        start.bounds,
        start.beat_start,
        start.beat_span,
    )?;
    let pointer_pitch = pitch_at_point(
        start.pointer[1],
        start.bounds,
        start.pitch_low,
        start.pitch_rows,
    )?;
    let note_end = start.original.start.checked_add(start.original.duration)?;
    if start.original.duration == 0 || note_end > start.clip_length {
        return None;
    }
    let kind = match start.kind {
        HitKind::Move => GestureKind::Move {
            pointer_beat_raw,
            pointer_pitch,
        },
        HitKind::ResizeLeading => GestureKind::ResizeLeading {
            pointer_beat_raw,
            note_end,
        },
        HitKind::ResizeTrailing => GestureKind::ResizeTrailing { pointer_beat_raw },
    };
    Some(NoteGesture {
        generation: start.generation,
        revision: start.revision,
        clip: start.clip,
        note: start.note,
        active: start.active,
        original: start.original,
        channel: start.channel,
        velocity: start.velocity,
        preview: start.original,
        source_prefix: 0,
        kind,
        bounds: start.bounds,
        beat_start: start.beat_start,
        beat_span: start.beat_span,
        clip_length: start.clip_length,
        pitch_low: start.pitch_low,
        pitch_rows: start.pitch_rows,
        grid: start.grid,
    })
}

#[cfg(all(test, feature = "ui-unit-tier"))]
pub(super) fn update(gesture: &mut NoteGesture, pointer_x: f32, pointer_y: f32) -> bool {
    update_with_modifier(gesture, pointer_x, pointer_y, false)
}

pub(super) fn update_with_modifier(
    gesture: &mut NoteGesture,
    pointer_x: f32,
    pointer_y: f32,
    invert_snap: bool,
) -> bool {
    let Some(pointer_beat_raw) = signed_beat_at_point(
        pointer_x,
        gesture.bounds,
        gesture.beat_start,
        gesture.beat_span,
    ) else {
        return false;
    };
    let Some(pointer_pitch) = pitch_at_point(
        pointer_y,
        gesture.bounds,
        gesture.pitch_low,
        gesture.pitch_rows,
    ) else {
        return false;
    };
    let effective_grid = if matches!(gesture.kind, GestureKind::ResizeTrailing { .. }) {
        note_end_grid(gesture.grid, invert_snap)
    } else {
        effective_grid(gesture.grid, invert_snap)
    };
    let delta_from = |anchor| quantize_delta(pointer_beat_raw - i128::from(anchor), effective_grid);
    let mut source_prefix = 0;
    let candidate = match gesture.kind {
        GestureKind::Move {
            pointer_beat_raw: down_beat,
            pointer_pitch: down_pitch,
        } if delta_from(down_beat) == 0 && pointer_pitch == down_pitch => gesture.original,
        GestureKind::Move {
            pointer_beat_raw: down_beat,
            pointer_pitch: down_pitch,
        } => {
            let delta = delta_from(down_beat);
            // Movement can retain material beyond the current source. The
            // canonical extent-growing update owns storage growth on commit.
            let max_start = u64::MAX - gesture.original.duration;
            let requested = (i128::from(gesture.original.start) + delta).min(i128::from(max_start));
            let Some((signed_start, prefix)) = super::midi_viewport_grid::source_prefix(
                requested,
                u64::MAX - gesture.clip_length,
                gesture.beat_start,
                gesture.beat_span,
                gesture.grid.step_raw().unwrap_or(BeatTime::ONE.raw()),
            ) else {
                return false;
            };
            source_prefix = prefix;
            let start = (signed_start + i128::from(source_prefix)) as u64;
            let pitch = (i16::from(gesture.original.pitch) + i16::from(pointer_pitch)
                - i16::from(down_pitch))
            .clamp(0, 127) as u8;
            NotePreview {
                start,
                duration: gesture.original.duration,
                pitch,
            }
        }
        GestureKind::ResizeLeading {
            pointer_beat_raw: down_beat,
            ..
        } if delta_from(down_beat) == 0 => gesture.original,
        GestureKind::ResizeLeading {
            pointer_beat_raw: down_beat,
            note_end,
        } => {
            let delta = delta_from(down_beat);
            let min_gate = effective_grid.minimum_gate().min(gesture.original.duration);
            let max_start = note_end.saturating_sub(min_gate);
            let start =
                (i128::from(gesture.original.start) + delta).clamp(0, i128::from(max_start)) as u64;
            NotePreview {
                start,
                duration: note_end.saturating_sub(start),
                pitch: gesture.original.pitch,
            }
        }
        GestureKind::ResizeTrailing {
            pointer_beat_raw: down_beat,
        } if delta_from(down_beat) == 0 => gesture.original,
        GestureKind::ResizeTrailing {
            pointer_beat_raw: down_beat,
        } => {
            let delta = delta_from(down_beat);
            let min_gate = effective_grid.minimum_gate().min(gesture.original.duration);
            let min_end = gesture.original.start.saturating_add(min_gate);
            let original_end = gesture
                .original
                .start
                .saturating_add(gesture.original.duration);
            let end = (i128::from(original_end) + delta)
                .clamp(i128::from(min_end), i128::from(gesture.clip_length))
                as u64;
            NotePreview {
                start: gesture.original.start,
                duration: end.saturating_sub(gesture.original.start),
                pitch: gesture.original.pitch,
            }
        }
    };
    let changed = gesture.preview != candidate || gesture.source_prefix != source_prefix;
    gesture.preview = candidate;
    gesture.source_prefix = source_prefix;
    changed
}

pub(super) fn is_noop(gesture: &NoteGesture) -> bool {
    gesture.source_prefix == 0 && gesture.preview == gesture.original
}

pub(super) fn release_is_inside(gesture: &NoteGesture, x: f32, y: f32) -> bool {
    point_is_inside(gesture.bounds, x, y)
}

pub(super) fn point_is_inside(bounds: [f32; 4], x: f32, y: f32) -> bool {
    let [left, top, width, height] = bounds;
    x.is_finite()
        && y.is_finite()
        && width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0
        && x >= left
        && x < left + width
        && y >= top
        && y < top + height
}

pub(super) fn beat_at_point(
    x: f32,
    bounds: [f32; 4],
    beat_start: i128,
    beat_span: u64,
) -> Option<u64> {
    u64::try_from(signed_beat_at_point(x, bounds, beat_start, beat_span)?).ok()
}
/// View coordinates may precede the unsigned authored source. Only explicit
/// source-prefix authoring may convert a negative view coordinate to storage.
pub(super) fn signed_beat_at_point(
    x: f32,
    bounds: [f32; 4],
    beat_start: i128,
    beat_span: u64,
) -> Option<i128> {
    let [left, top, width, height] = bounds;
    if ![x, left, top, width, height]
        .iter()
        .all(|value| value.is_finite())
        || width <= 0.0
        || height <= 0.0
        || beat_span == 0
    {
        return None;
    }
    let local = (x - left).clamp(0.0, width);
    let raw = (f64::from(local / width) * beat_span as f64).floor();
    if !raw.is_finite() || raw < 0.0 {
        return None;
    }
    beat_start.checked_add(i128::from(raw.min(u64::MAX as f64) as u64))
}

pub(super) fn quantize_delta(delta: i128, grid: MidiGrid) -> i128 {
    let Some(step) = grid.step_raw() else {
        return delta;
    };
    let step = i128::from(step.max(1));
    let magnitude = delta.unsigned_abs().min(i128::MAX as u128) as i128;
    let snapped = ((magnitude + step / 2) / step) * step;
    if delta < 0 { -snapped } else { snapped }
}

pub(super) fn pitch_at_point(
    y: f32,
    bounds: [f32; 4],
    pitch_low: u8,
    pitch_rows: u8,
) -> Option<u8> {
    let [left, top, width, height] = bounds;
    if !y.is_finite()
        || !left.is_finite()
        || !top.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || !super::midi_scene::valid_pitch_range(pitch_low, pitch_rows)
    {
        return None;
    }
    let row = (((y - top) / height) * f32::from(pitch_rows)).floor();
    let row = row.clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    Some(
        (i32::from(pitch_low) + i32::from(pitch_rows) - 1)
            .saturating_sub(row)
            .clamp(0, 127) as u8,
    )
}

fn snap_clamped(raw: i128, min: u64, max: u64, step: u64) -> u64 {
    let raw = raw.clamp(i128::from(min), i128::from(max));
    let grid = i128::from(step.max(1));
    let snapped = ((raw + grid / 2) / grid) * grid;
    snapped.clamp(i128::from(min), i128::from(max)) as u64
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{
        HitKind, MidiGrid, NotePreview, begin, default_gate_raw, effective_grid, hit_kind, is_noop,
        note_end_grid, snap_raw, update, update_with_modifier,
    };
    use hex_project::{BeatTime, Command, Created, Editor, Project};

    const BOUNDS: [f32; 4] = [100.0, 200.0, 400.0, 240.0];
    const CLIP_LENGTH: u64 = 4 * BeatTime::ONE.raw();

    fn gesture(kind: HitKind) -> super::NoteGesture {
        gesture_for(
            NotePreview {
                start: BeatTime::ONE.raw(),
                duration: BeatTime::ONE.raw() / 2,
                pitch: 60,
            },
            kind,
            [250.0, 335.0],
        )
    }

    fn gesture_for(original: NotePreview, kind: HitKind, pointer: [f32; 2]) -> super::NoteGesture {
        gesture_for_clip(original, kind, pointer, CLIP_LENGTH)
    }

    fn gesture_for_grid(
        original: NotePreview,
        kind: HitKind,
        pointer: [f32; 2],
        grid: MidiGrid,
    ) -> super::NoteGesture {
        let (clip, note) = ids();
        begin(super::GestureStart {
            generation: 3,
            revision: 9,
            clip,
            note,
            active: true,
            original,
            channel: 2,
            velocity: 91,
            kind,
            pointer,
            bounds: BOUNDS,
            beat_start: 0,
            beat_span: 4 * BeatTime::ONE.raw(),
            clip_length: CLIP_LENGTH,
            pitch_low: 48,
            pitch_rows: 24,
            grid,
        })
        .expect("valid note gesture")
    }

    fn gesture_for_clip(
        original: NotePreview,
        kind: HitKind,
        pointer: [f32; 2],
        clip_length: u64,
    ) -> super::NoteGesture {
        let (clip, note) = ids();
        begin(super::GestureStart {
            generation: 3,
            revision: 9,
            clip,
            note,
            active: true,
            original,
            channel: 2,
            velocity: 91,
            kind,
            pointer,
            bounds: BOUNDS,
            beat_start: 0,
            beat_span: 4 * BeatTime::ONE.raw(),
            clip_length,
            pitch_low: 48,
            pitch_rows: 24,
            grid: MidiGrid::Sixteenth,
        })
        .expect("valid note gesture")
    }

    fn gesture_for_span(
        original: NotePreview,
        kind: HitKind,
        pointer: [f32; 2],
        beat_span: u64,
    ) -> super::NoteGesture {
        let (clip, note) = ids();
        begin(super::GestureStart {
            generation: 3,
            revision: 9,
            clip,
            note,
            active: true,
            original,
            channel: 2,
            velocity: 91,
            kind,
            pointer,
            bounds: BOUNDS,
            beat_start: 0,
            beat_span,
            clip_length: CLIP_LENGTH,
            pitch_low: 48,
            pitch_rows: 24,
            grid: MidiGrid::Sixteenth,
        })
        .expect("valid note gesture")
    }

    fn ids() -> (hex_project::ClipId, hex_project::NoteId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "instrument".into(),
            })
            .expect("track")
        else {
            panic!("track result")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).expect("clip length"),
            })
            .expect("clip")
        else {
            panic!("clip result")
        };
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ONE,
                duration: BeatTime::from_ratio(1, 2).expect("note length"),
                pitch: 60,
                channel: 2,
                velocity: 91,
            })
            .expect("note")
        else {
            panic!("note result")
        };
        (clip, note)
    }

    #[test]
    fn move_preserves_grab_offset_and_changes_onset_and_pitch_in_grid_steps() {
        let mut drag = gesture(HitKind::Move);
        assert!(update(&mut drag, 350.0, 295.0));
        assert_eq!(drag.preview.start, BeatTime::ONE.raw() * 2);
        assert_eq!(drag.preview.duration, BeatTime::ONE.raw() / 2);
        assert_eq!(drag.preview.pitch, 64);
        assert!(!is_noop(&drag));
        assert_eq!((drag.channel, drag.velocity), (2, 91));
    }

    #[test]
    fn move_beat_mapping_tracks_the_current_zoom_span() {
        let original = NotePreview {
            start: BeatTime::ONE.raw(),
            duration: BeatTime::ONE.raw() / 2,
            pitch: 60,
        };
        let mut drag = gesture_for_span(
            original,
            HitKind::Move,
            [200.0, 335.0],
            2 * BeatTime::ONE.raw(),
        );
        assert!(update(&mut drag, 300.0, 335.0));
        assert_eq!(drag.preview.start, original.start + BeatTime::ONE.raw() / 2);
        assert_eq!(drag.preview.duration, original.duration);
    }

    #[test]
    fn every_grid_step_applies_to_move_and_both_resize_edges() {
        let original = NotePreview {
            start: BeatTime::ONE.raw(),
            duration: BeatTime::ONE.raw() * 2,
            pitch: 60,
        };
        for (grid, pointer_start, pointer_end, expected_delta) in [
            (MidiGrid::Quarter, 200.0, 300.0, BeatTime::ONE.raw()),
            (MidiGrid::Eighth, 200.0, 250.0, BeatTime::ONE.raw() / 2),
            (MidiGrid::Sixteenth, 200.0, 225.0, BeatTime::ONE.raw() / 4),
            (
                MidiGrid::ThirtySecond,
                200.0,
                212.5,
                BeatTime::ONE.raw() / 8,
            ),
            (MidiGrid::Off, 200.0, 225.0, BeatTime::ONE.raw() / 4),
        ] {
            let mut moving =
                gesture_for_grid(original, HitKind::Move, [pointer_start, 335.0], grid);
            update(&mut moving, pointer_end, 335.0);
            assert_eq!(moving.preview.start, original.start + expected_delta);
            assert_eq!(moving.preview.duration, original.duration);

            let mut leading = gesture_for_grid(
                original,
                HitKind::ResizeLeading,
                [pointer_start, 335.0],
                grid,
            );
            update(&mut leading, pointer_end, 335.0);
            assert_eq!(leading.preview.start, original.start + expected_delta);
            assert_eq!(leading.preview.duration, original.duration - expected_delta);

            let mut trailing = gesture_for_grid(
                original,
                HitKind::ResizeTrailing,
                [pointer_start, 335.0],
                grid,
            );
            update(&mut trailing, pointer_end, 335.0);
            assert_eq!(trailing.preview.start, original.start);
            assert_eq!(
                trailing.preview.duration,
                original.duration + expected_delta
            );
        }
    }

    #[test]
    fn off_grid_gesture_preserves_authored_values_on_noop_and_keeps_gate_positive() {
        let original = NotePreview {
            start: BeatTime::ONE.raw() / 3,
            duration: BeatTime::ONE.raw() / 257,
            pitch: 60,
        };
        let mut no_op =
            gesture_for_grid(original, HitKind::Move, [133.33334, 335.0], MidiGrid::Off);
        assert!(!update(&mut no_op, 133.33334, 335.0));
        assert_eq!(no_op.preview, original);
        assert!(is_noop(&no_op));

        let mut trailing = gesture_for_grid(
            original,
            HitKind::ResizeTrailing,
            [133.33334, 335.0],
            MidiGrid::Off,
        );
        update(&mut trailing, 100.0, 335.0);
        assert!(trailing.preview.duration > 0);
        assert!(trailing.preview.start + trailing.preview.duration <= CLIP_LENGTH);
    }

    #[test]
    fn both_note_edges_resize_with_a_quarter_beat_minimum() {
        // Trailing ends snap only while cmd is held.
        let mut trailing = gesture(HitKind::ResizeTrailing);
        update_with_modifier(&mut trailing, 300.0, 335.0, true);
        assert_eq!(trailing.preview.start, BeatTime::ONE.raw());
        assert_eq!(trailing.preview.duration, BeatTime::ONE.raw());

        let mut leading = gesture(HitKind::ResizeLeading);
        update(&mut leading, 200.0, 335.0);
        assert_eq!(leading.preview.start, BeatTime::ONE.raw() / 2);
        assert_eq!(leading.preview.duration, BeatTime::ONE.raw());

        update_with_modifier(&mut trailing, 150.0, 335.0, true);
        assert_eq!(trailing.preview.duration, BeatTime::ONE.raw() / 4);
        update(&mut leading, 350.0, 335.0);
        assert_eq!(leading.preview.start, BeatTime::ONE.raw() * 5 / 4);
        assert_eq!(leading.preview.duration, BeatTime::ONE.raw() / 4);
    }

    #[test]
    fn move_preserves_gate_and_rebases_negative_time_with_pitch_limits() {
        let mut drag = gesture(HitKind::Move);
        assert!(is_noop(&drag));
        assert!(!update(&mut drag, 250.0, 335.0));
        update(&mut drag, 500.0, 200.0);
        assert_eq!(drag.preview.duration, drag.original.duration);
        assert_eq!(drag.preview.pitch, 73);
        update(&mut drag, -10_000.0, 10_000.0);
        assert_eq!(drag.source_prefix, 4 * BeatTime::ONE.raw());
        assert_eq!(
            i128::from(drag.preview.start) - i128::from(drag.source_prefix),
            -i128::from(BeatTime::ONE.raw() / 2)
        );
        assert_eq!(drag.preview.pitch, 2);
        update(&mut drag, 250.0, -10_000.0);
        assert_eq!(
            drag.source_prefix, 0,
            "reversal clears the held origin shift"
        );
        assert_eq!(drag.preview.pitch, 127);
    }

    #[test]
    fn move_retains_gate_length_beyond_source_and_at_numeric_end() {
        let mut drag = gesture_for_span(
            NotePreview {
                start: 1,
                duration: 1,
                pitch: 60,
            },
            HitKind::Move,
            [BOUNDS[0], 335.0],
            u64::MAX,
        );
        drag.grid = MidiGrid::Off;
        assert!(update(&mut drag, BOUNDS[0] + BOUNDS[2], 335.0));
        assert_eq!(drag.preview.start, u64::MAX - 1);
        assert_eq!(drag.preview.duration, 1);
        assert_eq!(
            drag.preview.start.checked_add(drag.preview.duration),
            Some(u64::MAX)
        );
        assert!(drag.preview.start > drag.clip_length);
        assert!(update(&mut drag, BOUNDS[0], 335.0));
        assert_eq!(drag.preview, drag.original);
    }

    #[test]
    fn invalid_geometry_and_nonfinite_points_are_rejected() {
        let (clip, note) = ids();
        assert!(
            begin(super::GestureStart {
                generation: 1,
                revision: 1,
                clip,
                note,
                active: true,
                original: NotePreview {
                    start: 0,
                    duration: BeatTime::ONE.raw(),
                    pitch: 60,
                },
                channel: 0,
                velocity: 100,
                kind: HitKind::Move,
                pointer: [f32::NAN, 10.0],
                bounds: BOUNDS,
                beat_start: 0,
                beat_span: 4 * BeatTime::ONE.raw(),
                clip_length: CLIP_LENGTH,
                pitch_low: 48,
                pitch_rows: 24,
                grid: MidiGrid::Sixteenth,
            })
            .is_none()
        );
        let mut drag = gesture(HitKind::Move);
        assert!(!update(&mut drag, f32::INFINITY, 300.0));
        assert_eq!(drag.preview, drag.original);
    }

    #[test]
    fn note_hit_zones_assign_both_edges_and_body() {
        assert_eq!(
            hit_kind(100.0, 100.0, 50.0, true, true),
            HitKind::ResizeLeading
        );
        assert_eq!(hit_kind(125.0, 100.0, 50.0, true, true), HitKind::Move);
        assert_eq!(
            hit_kind(150.0, 100.0, 50.0, true, true),
            HitKind::ResizeTrailing
        );
        assert_eq!(hit_kind(100.0, 100.0, 200.0, false, true), HitKind::Move);
        assert_eq!(hit_kind(150.0, 100.0, 200.0, true, false), HitKind::Move);
        assert_eq!(hit_kind(293.0, 100.0, 200.0, true, true), HitKind::Move);
        assert_eq!(
            hit_kind(298.0, 100.0, 200.0, true, true),
            HitKind::ResizeTrailing
        );
    }

    #[test]
    fn tiny_note_hit_halo_remains_a_move_target_without_fake_edge_resize() {
        assert_eq!(hit_kind(9.0, 8.0, 4.0, true, true), HitKind::Move);
        assert_eq!(hit_kind(8.0, 8.0, 12.0, false, true), HitKind::Move);
        assert_eq!(
            hit_kind(11.0, 8.0, 12.0, true, true),
            HitKind::ResizeLeading
        );
        assert_eq!(
            hit_kind(17.0, 8.0, 12.0, true, true),
            HitKind::ResizeTrailing
        );
    }

    #[test]
    fn off_grid_and_short_notes_remain_exact_until_the_pointer_leaves_its_snap_cell() {
        let original = NotePreview {
            start: BeatTime::ONE.raw() / 4 + BeatTime::ONE.raw() / 100,
            duration: BeatTime::ONE.raw() / 32,
            pitch: 60,
        };
        let mut moving = gesture_for(original, HitKind::Move, [126.0, 335.0]);
        assert!(!update(&mut moving, 127.0, 335.0));
        assert_eq!(moving.preview, original);
        assert!(is_noop(&moving));
        assert!(update(&mut moving, 151.0, 335.0));
        assert_eq!(
            moving.preview.start,
            original.start + BeatTime::ONE.raw() / 4
        );
        assert!(update(&mut moving, 126.0, 335.0));
        assert_eq!(moving.preview, original);
        assert!(is_noop(&moving));

        for edge in [HitKind::ResizeLeading, HitKind::ResizeTrailing] {
            let snap = edge == HitKind::ResizeTrailing;
            let mut resizing = gesture_for(original, edge, [126.0, 335.0]);
            assert!(!update_with_modifier(&mut resizing, 127.0, 335.0, snap));
            assert_eq!(resizing.preview, original);
            assert!(is_noop(&resizing));
            update_with_modifier(&mut resizing, 151.0, 335.0, snap);
            update_with_modifier(&mut resizing, 126.0, 335.0, snap);
            assert_eq!(resizing.preview, original);
            assert!(is_noop(&resizing));
        }
    }

    #[test]
    fn shrinking_sub_grid_notes_does_not_expand_them() {
        let original = NotePreview {
            start: BeatTime::ONE.raw(),
            duration: BeatTime::ONE.raw() / 32,
            pitch: 60,
        };
        for (kind, x) in [
            (HitKind::ResizeLeading, 151.0),
            (HitKind::ResizeTrailing, 101.0),
        ] {
            let mut resize = gesture_for(original, kind, [126.0, 335.0]);
            update_with_modifier(&mut resize, x, 335.0, kind == HitKind::ResizeTrailing);
            assert_eq!(
                resize.preview, original,
                "a coarse-grid shrink may clamp, never grow"
            );
        }
    }

    #[test]
    fn only_explicitly_selected_tiny_edges_offer_resize_and_hidden_edges_stay_hidden() {
        use super::selected_hit_kind;
        for (x, expected) in [
            (8.25, HitKind::ResizeLeading),
            (10.0, HitKind::Move),
            (11.75, HitKind::ResizeTrailing),
        ] {
            assert_eq!(selected_hit_kind(x, 8.0, 4.0, true, true, true), expected);
            assert_eq!(
                selected_hit_kind(x, 8.0, 4.0, true, true, false),
                HitKind::Move
            );
        }
        assert_eq!(
            selected_hit_kind(8.25, 8.0, 4.0, false, true, true),
            HitKind::Move
        );
        assert_eq!(
            selected_hit_kind(11.75, 8.0, 4.0, true, false, true),
            HitKind::Move
        );
        assert_eq!(
            selected_hit_kind(f32::NAN, 8.0, 4.0, true, true, true),
            HitKind::Move
        );
    }

    #[test]
    fn changed_short_gate_resizes_to_the_quarter_beat_or_available_clip_length() {
        let original = NotePreview {
            start: BeatTime::ONE.raw() / 4,
            duration: BeatTime::ONE.raw() / 32,
            pitch: 60,
        };
        let mut trailing = gesture_for(original, HitKind::ResizeTrailing, [126.0, 335.0]);
        update(&mut trailing, 151.0, 335.0);
        assert_eq!(trailing.preview.start, original.start);
        assert!(trailing.preview.duration >= BeatTime::ONE.raw() / 4);
        assert!(trailing.preview.start + trailing.preview.duration <= CLIP_LENGTH);

        let short_clip_length = BeatTime::ONE.raw() / 8;
        let tiny_note = NotePreview {
            start: 0,
            duration: BeatTime::ONE.raw() / 64,
            pitch: 60,
        };
        let mut short_clip = gesture_for_clip(
            tiny_note,
            HitKind::ResizeTrailing,
            [100.0, 335.0],
            short_clip_length,
        );
        update(&mut short_clip, 150.0, 335.0);
        assert_eq!(short_clip.preview.duration, short_clip_length);
    }

    #[test]
    fn midi_grid_steps_and_off_mode_are_exact_and_bounded() {
        let beat = BeatTime::ONE.raw();
        assert_eq!(MidiGrid::default(), MidiGrid::Sixteenth);
        assert_eq!(MidiGrid::Quarter.step_raw(), Some(beat));
        assert_eq!(MidiGrid::Eighth.step_raw(), Some(beat / 2));
        assert_eq!(MidiGrid::Sixteenth.step_raw(), Some(beat / 4));
        assert_eq!(MidiGrid::ThirtySecond.step_raw(), Some(beat / 8));
        assert_eq!(MidiGrid::Off.step_raw(), None);
        assert_eq!(MidiGrid::Off.movement_step(), beat / 4);
        assert_eq!(snap_raw(beat / 3, MidiGrid::Sixteenth), beat / 4);
        assert_eq!(snap_raw(beat / 3, MidiGrid::Off), beat / 3);
        assert_eq!(default_gate_raw(beat, MidiGrid::Quarter), beat);
        assert_eq!(default_gate_raw(beat / 8, MidiGrid::Quarter), beat / 8);
        assert_eq!(default_gate_raw(beat, MidiGrid::Off), beat / 4);
        assert_eq!(default_gate_raw(beat / 8, MidiGrid::Off), beat / 8);
        assert_eq!(MidiGrid::Off.next(), MidiGrid::Quarter);
        assert_eq!(MidiGrid::ThirtySecond.next(), MidiGrid::Off);
    }

    #[test]
    fn platform_modifier_inverts_fixed_and_free_grids_without_changing_selection() {
        for grid in [
            MidiGrid::Quarter,
            MidiGrid::Eighth,
            MidiGrid::Sixteenth,
            MidiGrid::ThirtySecond,
        ] {
            assert_eq!(effective_grid(grid, false), grid);
            assert_eq!(effective_grid(grid, true), MidiGrid::Off);
        }
        assert_eq!(effective_grid(MidiGrid::Off, false), MidiGrid::Off);
        assert_eq!(effective_grid(MidiGrid::Off, true), MidiGrid::Sixteenth);
    }

    #[test]
    fn modifier_toggle_keeps_raw_pointer_down_anchor_and_recomputes_preview() {
        let original = NotePreview {
            start: BeatTime::ONE.raw() + BeatTime::ONE.raw() / 8,
            duration: BeatTime::ONE.raw() / 2,
            pitch: 60,
        };
        let pointer = [212.5, 335.0]; // clip-local beat 1.125, off-grid
        let mut fixed = gesture_for_grid(original, HitKind::Move, pointer, MidiGrid::Sixteenth);
        assert!(!update_with_modifier(
            &mut fixed, pointer[0], pointer[1], true
        ));
        assert_eq!(fixed.preview, original);

        assert!(update_with_modifier(&mut fixed, 218.75, pointer[1], true));
        assert_eq!(
            fixed.preview.start,
            original.start + BeatTime::ONE.raw() / 16
        );
        assert!(update_with_modifier(&mut fixed, 218.75, pointer[1], false));
        assert_eq!(fixed.preview, original);
        assert!(update_with_modifier(&mut fixed, 237.5, pointer[1], true));
        assert_eq!(
            fixed.preview.start,
            original.start + BeatTime::ONE.raw() / 4
        );

        let mut free = gesture_for_grid(original, HitKind::Move, pointer, MidiGrid::Off);
        assert!(!update_with_modifier(
            &mut free, pointer[0], pointer[1], true
        ));
        assert!(update_with_modifier(&mut free, 225.0, pointer[1], true));
        assert_eq!(free.preview.start, original.start + BeatTime::ONE.raw() / 4);
        assert!(update_with_modifier(&mut free, 225.0, pointer[1], false));
        assert_eq!(free.preview.start, original.start + BeatTime::ONE.raw() / 8);
    }

    #[test]
    fn modifier_toggle_keeps_opposite_edge_and_bounds_for_both_resizes() {
        let original = NotePreview {
            start: BeatTime::ONE.raw(),
            duration: BeatTime::ONE.raw() / 2,
            pitch: 60,
        };
        let start = BeatTime::ONE.raw();
        let end = start + original.duration;
        let pointer = [212.5, 335.0];
        let movement = BeatTime::ONE.raw() / 4;

        let mut leading =
            gesture_for_grid(original, HitKind::ResizeLeading, pointer, MidiGrid::Off);
        assert!(update_with_modifier(&mut leading, 237.5, pointer[1], true));
        assert_eq!(leading.preview.start, start + movement);
        assert_eq!(leading.preview.start + leading.preview.duration, end);
        assert!(update_with_modifier(
            &mut leading,
            206.25,
            pointer[1],
            false
        ));
        assert_eq!(leading.preview.start, start - BeatTime::ONE.raw() / 16);
        assert_eq!(leading.preview.start + leading.preview.duration, end);
        assert!(leading.preview.duration > 0);

        let mut trailing =
            gesture_for_grid(original, HitKind::ResizeTrailing, pointer, MidiGrid::Off);
        assert!(update_with_modifier(&mut trailing, 237.5, pointer[1], true));
        assert_eq!(trailing.preview.start, start);
        assert_eq!(
            trailing.preview.start + trailing.preview.duration,
            end + movement
        );
        assert!(update_with_modifier(
            &mut trailing,
            206.25,
            pointer[1],
            false
        ));
        assert_eq!(trailing.preview.start, start);
        assert_eq!(
            trailing.preview.start + trailing.preview.duration,
            end - BeatTime::ONE.raw() / 16
        );
        assert!(trailing.preview.duration > 0);
    }

    #[test]
    fn trailing_resize_is_free_by_default_and_snaps_while_cmd_is_held() {
        let beat = BeatTime::ONE.raw();
        let original = NotePreview {
            start: beat,
            duration: beat / 2,
            pitch: 60,
        };
        let end = original.start + original.duration;
        let pointer = [250.0, 335.0]; // clip-local beat 1.5, the note end
        let at = |x| super::beat_at_point(x, BOUNDS, 0, 4 * beat).unwrap();
        let free_delta = at(265.0) - at(pointer[0]); // about 0.15 beat
        for grid in [MidiGrid::Quarter, MidiGrid::Sixteenth, MidiGrid::Off] {
            let mut trailing = gesture_for_grid(original, HitKind::ResizeTrailing, pointer, grid);
            assert!(update_with_modifier(
                &mut trailing,
                265.0,
                pointer[1],
                false
            ));
            assert_eq!(trailing.preview.start, original.start, "grid={grid:?}");
            assert_eq!(
                trailing.preview.start + trailing.preview.duration,
                end + free_delta,
                "grid={grid:?}: default end is free"
            );
            assert!(update_with_modifier(&mut trailing, 265.0, pointer[1], true));
            let step = note_end_grid(grid, true).step_raw().unwrap();
            assert_eq!(
                trailing.preview.start + trailing.preview.duration,
                end + (free_delta + step / 2) / step * step,
                "grid={grid:?}: cmd snaps the end delta"
            );
            // Dragging far left keeps a positive gate; release of cmd frees it.
            assert!(update_with_modifier(
                &mut trailing,
                100.0,
                pointer[1],
                false
            ));
            assert!(trailing.preview.duration > 0);
        }
        // Leading resize keeps the grid by default.
        let mut leading = gesture_for_grid(
            original,
            HitKind::ResizeLeading,
            [200.0, 335.0],
            MidiGrid::Quarter,
        );
        assert!(!update_with_modifier(&mut leading, 215.0, 335.0, false));
        assert_eq!(leading.preview, original);
    }

    #[test]
    fn release_requires_finite_point_inside_roll_bounds() {
        use super::release_is_inside;

        let drag = gesture(HitKind::Move);
        assert!(release_is_inside(&drag, 100.0, 200.0));
        assert!(release_is_inside(&drag, 499.9, 439.9));
        assert!(!release_is_inside(&drag, 500.0, 300.0));
        assert!(!release_is_inside(&drag, 300.0, 440.0));
        assert!(!release_is_inside(&drag, f32::NAN, 300.0));
    }

    #[test]
    fn stale_revision_generation_selection_owner_and_view_geometry_cancel() {
        use super::{GestureContext, is_current};

        let drag = gesture(HitKind::Move);
        let context = GestureContext {
            generation: drag.generation,
            revision: drag.revision,
            selected_clip: Some(drag.clip),
            selected_note: Some(drag.note),
            clip_contains_note: true,
            pending: false,
            beat_start: drag.beat_start,
            beat_span: drag.beat_span,
            pitch_low: drag.pitch_low,
            pitch_rows: drag.pitch_rows,
            grid: drag.grid,
            bounds: drag.bounds,
        };
        assert!(is_current(&drag, context));
        assert!(!is_current(
            &drag,
            GestureContext {
                generation: context.generation + 1,
                ..context
            }
        ));
        assert!(!is_current(
            &drag,
            GestureContext {
                revision: context.revision + 1,
                ..context
            }
        ));
        assert!(!is_current(
            &drag,
            GestureContext {
                selected_note: None,
                ..context
            }
        ));
        assert!(!is_current(
            &drag,
            GestureContext {
                clip_contains_note: false,
                ..context
            }
        ));
        assert!(!is_current(
            &drag,
            GestureContext {
                pending: true,
                ..context
            }
        ));
        assert!(!is_current(
            &drag,
            GestureContext {
                grid: MidiGrid::Off,
                ..context
            }
        ));
        assert!(!is_current(
            &drag,
            GestureContext {
                bounds: [100.0, 200.0, 401.0, 240.0],
                ..context
            }
        ));
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod creation_geometry_tests {
    use super::*;
    #[test]
    fn creation_uses_containing_grid_cell_and_bounded_positive_pixel_gate() {
        let beat = BeatTime::ONE.raw();
        assert_eq!(creation_start(beat * 7 / 4, MidiGrid::Quarter), beat);
        assert_eq!(creation_start(beat - 1, MidiGrid::Quarter), 0);
        assert_eq!(creation_start(beat, MidiGrid::Quarter), beat);
        assert_eq!(
            creation_start(u64::MAX, MidiGrid::Quarter),
            u64::MAX / beat * beat
        );
        assert_eq!(creation_start(beat + 73, MidiGrid::Off), beat + 73);
        assert_eq!(
            creation_gate(beat, MidiGrid::Off, 4 * beat, 400.0, 1),
            Some((beat as f64 / 100.0).ceil() as u64)
        );
        assert_eq!(creation_gate(beat, MidiGrid::Off, 4, 400.0, 20), Some(20));
        assert_eq!(
            creation_gate(beat, MidiGrid::Sixteenth, 4 * beat, 400.0, 1),
            Some(beat / 4)
        );
        assert_eq!(creation_gate(1, MidiGrid::Off, 4, 400.0, 2), None);
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(creation_gate(beat, MidiGrid::Off, beat, width, 1), None);
        }
        assert_eq!(
            creation_gate(u64::MAX, MidiGrid::Off, u64::MAX, f32::MIN_POSITIVE, 1),
            Some(u64::MAX)
        );
        assert_eq!(
            default_gate_raw(beat, MidiGrid::Off),
            beat / 4,
            "step editing keeps its independent default"
        );
    }
}
