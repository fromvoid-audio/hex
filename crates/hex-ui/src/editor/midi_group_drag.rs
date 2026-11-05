//! Immutable note-group move, edge-resize and copy previews with atomic payloads.
use super::midi_drag::{self, HitKind, MidiGrid};
use hex_project::{BeatTime, ClipId, MidiNoteUpdate, NoteId};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GroupNote {
    pub(super) id: NoteId,
    pub(super) active: bool,
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) pitch: u8,
    pub(super) channel: u8,
    pub(super) velocity: u8,
}

/// Capture the selected members from one clip snapshot. IDs are expected to be
/// sorted and unique, as provided by `MidiSelection`.
pub(super) fn capture_selected_notes(
    notes: impl Iterator<Item = GroupNote>,
    selected: &[NoteId],
) -> Result<Vec<GroupNote>, &'static str> {
    if selected.is_empty()
        || selected.len() > hex_project::MAX_NOTES
        || selected.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err("invalid selected midi note ids");
    }
    let mut originals = Vec::new();
    originals
        .try_reserve_exact(selected.len())
        .map_err(|_| "could not reserve selected midi notes")?;
    for note in notes {
        if selected.binary_search(&note.id).is_ok() {
            originals.push(note);
        }
    }
    if originals.len() != selected.len() {
        return Err("selected midi notes changed · edit canceled");
    }
    Ok(originals)
}

#[derive(Clone, Copy)]
pub(super) enum GroupParameter {
    Pitch(i16),
    Duration(i8),
    Velocity(i16),
}

/// Build one all-or-nothing parameter update while keeping each note's other
/// fields intact. Parameter buttons refuse the whole edit at a boundary.
pub(super) fn parameter_updates(
    originals: &[GroupNote],
    clip_length: u64,
    parameter: GroupParameter,
    grid_step: u64,
) -> Result<Option<Vec<MidiNoteUpdate>>, &'static str> {
    if originals.is_empty() || originals.len() > hex_project::MAX_NOTES || clip_length == 0 {
        return Err("invalid selected midi note set");
    }
    if grid_step == 0 {
        return Err("invalid midi grid step");
    }
    let mut updates = Vec::new();
    updates
        .try_reserve_exact(originals.len())
        .map_err(|_| "could not reserve group midi parameter edit")?;
    let mut changed = false;
    for note in originals {
        note.start
            .checked_add(note.duration)
            .filter(|end| note.duration > 0 && *end <= clip_length)
            .ok_or("selected midi note exceeds clip bounds")?;
        if note.pitch > 127 || note.channel > 15 || !(1..=127).contains(&note.velocity) {
            return Err("invalid selected midi note");
        }
        let (duration, pitch, velocity) = match parameter {
            GroupParameter::Pitch(delta) => {
                let pitch = i32::from(note.pitch) + i32::from(delta);
                if !(0..=127).contains(&pitch) {
                    return Err("group pitch edit exceeds midi range");
                }
                (note.duration, pitch as u8, note.velocity)
            }
            GroupParameter::Duration(delta) => {
                let delta = i128::from(delta) * i128::from(grid_step);
                let duration = u64::try_from(i128::from(note.duration) + delta)
                    .ok()
                    .filter(|duration| *duration > 0)
                    .ok_or("group length edit exceeds note range")?;
                if note
                    .start
                    .checked_add(duration)
                    .is_none_or(|new_end| new_end > clip_length)
                {
                    return Err("group length edit exceeds clip bounds");
                }
                (duration, note.pitch, note.velocity)
            }
            GroupParameter::Velocity(delta) => {
                let velocity = i32::from(note.velocity) + i32::from(delta);
                if !(1..=127).contains(&velocity) {
                    return Err("group velocity edit exceeds midi range");
                }
                (note.duration, note.pitch, velocity as u8)
            }
        };
        changed |= duration != note.duration || pitch != note.pitch || velocity != note.velocity;
        updates.push(MidiNoteUpdate {
            note: note.id,
            start: BeatTime::from_raw(note.start),
            duration: BeatTime::from_raw(duration),
            pitch,
            channel: note.channel,
            velocity,
        });
    }
    if !changed {
        return Ok(None);
    }
    Ok(Some(updates))
}

/// Resolve one common time/pitch offset against every admitted member.
/// Callers validate unique IDs when admitting the immutable selection.
pub(super) fn resolve_common_delta(
    originals: &[GroupNote],
    clip_length: u64,
    requested_time: i128,
    requested_pitch: i16,
) -> Result<(i128, i16), &'static str> {
    if originals.is_empty() || originals.len() > hex_project::MAX_NOTES || clip_length == 0 {
        return Err("invalid selected midi note set");
    }
    let mut min_start = u64::MAX;
    let mut max_end = 0_u64;
    let mut min_pitch = 127_u8;
    let mut max_pitch = 0_u8;
    for note in originals {
        if note.duration == 0
            || note.pitch > 127
            || note.channel > 15
            || !(1..=127).contains(&note.velocity)
        {
            return Err("invalid selected midi note");
        }
        let end = note
            .start
            .checked_add(note.duration)
            .filter(|end| *end <= clip_length)
            .ok_or("selected midi note exceeds clip bounds")?;
        min_start = min_start.min(note.start);
        max_end = max_end.max(end);
        min_pitch = min_pitch.min(note.pitch);
        max_pitch = max_pitch.max(note.pitch);
    }
    let minimum_time = -i128::from(min_start);
    let maximum_time = i128::from(clip_length - max_end);
    let time = requested_time.clamp(minimum_time, maximum_time);
    let pitch = requested_pitch.clamp(-i16::from(min_pitch), i16::from(127 - max_pitch));
    Ok((time, pitch))
}

/// Build one validated batch for a keyboard nudge. A common zero delta is a
/// no-op and deliberately returns no command payload.
pub(super) fn nudge_updates(
    originals: &[GroupNote],
    clip_length: u64,
    requested_time: i128,
    requested_pitch: i16,
) -> Result<Option<Vec<MidiNoteUpdate>>, &'static str> {
    let (time, pitch) =
        resolve_common_delta(originals, clip_length, requested_time, requested_pitch)?;
    if time == 0 && pitch == 0 {
        return Ok(None);
    }
    let mut updates = Vec::new();
    updates
        .try_reserve_exact(originals.len())
        .map_err(|_| "could not reserve group midi nudge")?;
    for note in originals {
        let start = u64::try_from(i128::from(note.start) + time)
            .map_err(|_| "group midi nudge exceeds beat range")?;
        let pitch = u8::try_from(i16::from(note.pitch) + pitch)
            .map_err(|_| "group midi nudge exceeds pitch range")?;
        updates.push(MidiNoteUpdate {
            note: note.id,
            start: BeatTime::from_raw(start),
            duration: BeatTime::from_raw(note.duration),
            pitch,
            channel: note.channel,
            velocity: note.velocity,
        });
    }
    Ok(Some(updates))
}

#[derive(Clone, Copy)]
pub(super) struct GroupContext {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) clip: Option<ClipId>,
    pub(super) selection_epoch: u64,
    pub(super) beat_start: i128,
    pub(super) beat_span: u64,
    pub(super) pitch_low: u8,
    pub(super) pitch_rows: u8,
    pub(super) grid: MidiGrid,
    pub(super) bounds: [f32; 4],
    pub(super) pending: bool,
}

pub(super) struct GroupStart {
    pub(super) copy: bool,
    pub(super) kind: HitKind,
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) clip: ClipId,
    pub(super) selection_epoch: u64,
    pub(super) originals: Vec<GroupNote>,
    pub(super) pointer: [f32; 2],
    pub(super) bounds: [f32; 4],
    pub(super) beat_start: i128,
    pub(super) beat_span: u64,
    pub(super) pitch_low: u8,
    pub(super) pitch_rows: u8,
    pub(super) clip_length: u64,
    pub(super) grid: MidiGrid,
}

#[derive(Clone)]
pub(super) struct GroupGesture {
    copy: bool,
    kind: HitKind,
    generation: u64,
    revision: u64,
    clip: ClipId,
    selection_epoch: u64,
    originals: Arc<[GroupNote]>,
    pointer_beat: u64,
    pointer_pitch: u8,
    bounds: [f32; 4],
    beat_start: i128,
    beat_span: u64,
    pitch_low: u8,
    pitch_rows: u8,
    clip_length: u64,
    grid: MidiGrid,
    delta: i128,
    pitch_delta: i16,
    source_prefix: u64,
}

impl GroupGesture {
    pub(super) fn begin(mut start: GroupStart) -> Option<Self> {
        if start.originals.is_empty()
            || (!start.copy && start.originals.len() < 2)
            || start.originals.len() > hex_project::MAX_NOTES
            || start.beat_span == 0
            || start.clip_length == 0
        {
            return None;
        }
        start.originals.sort_unstable_by_key(|note| note.id);
        for pair in start.originals.windows(2) {
            if pair[0].id == pair[1].id {
                return None;
            }
        }
        for note in &start.originals {
            if note.duration == 0
                || note.pitch > 127
                || note.channel > 15
                || !(1..=127).contains(&note.velocity)
                || note
                    .start
                    .checked_add(note.duration)
                    .is_none_or(|end| end > start.clip_length)
            {
                return None;
            }
        }
        let pointer_beat = midi_drag::beat_at_point(
            start.pointer[0],
            start.bounds,
            start.beat_start,
            start.beat_span,
        )?;
        let pointer_pitch = midi_drag::pitch_at_point(
            start.pointer[1],
            start.bounds,
            start.pitch_low,
            start.pitch_rows,
        )?;
        Some(Self {
            copy: start.copy,
            kind: if start.copy {
                HitKind::Move
            } else {
                start.kind
            },
            generation: start.generation,
            revision: start.revision,
            clip: start.clip,
            selection_epoch: start.selection_epoch,
            originals: Arc::from(start.originals),
            pointer_beat,
            pointer_pitch,
            bounds: start.bounds,
            beat_start: start.beat_start,
            beat_span: start.beat_span,
            pitch_low: start.pitch_low,
            pitch_rows: start.pitch_rows,
            clip_length: start.clip_length,
            grid: start.grid,
            delta: 0,
            pitch_delta: 0,
            source_prefix: 0,
        })
    }

    pub(super) fn is_current(&self, context: GroupContext) -> bool {
        self.generation == context.generation
            && self.revision == context.revision
            && context.clip == Some(self.clip)
            && self.selection_epoch == context.selection_epoch
            && self.beat_start == context.beat_start
            && self.beat_span == context.beat_span
            && self.pitch_low == context.pitch_low
            && self.pitch_rows == context.pitch_rows
            && self.grid == context.grid
            && self.bounds == context.bounds
            && !context.pending
    }

    pub(super) fn update(&mut self, point: [f32; 2], invert_snap: bool) -> bool {
        let Some(pointer_beat) =
            midi_drag::signed_beat_at_point(point[0], self.bounds, self.beat_start, self.beat_span)
        else {
            return false;
        };
        let Some(pointer_pitch) =
            midi_drag::pitch_at_point(point[1], self.bounds, self.pitch_low, self.pitch_rows)
        else {
            return false;
        };
        let grid = if self.kind == HitKind::ResizeTrailing {
            midi_drag::note_end_grid(self.grid, invert_snap)
        } else {
            midi_drag::effective_grid(self.grid, invert_snap)
        };
        let desired_delta = midi_drag::quantize_delta(
            i128::from(pointer_beat) - i128::from(self.pointer_beat),
            grid,
        );
        let source_prefix = if self.kind == HitKind::Move && !self.copy {
            let first = self
                .originals
                .iter()
                .map(|note| note.start)
                .min()
                .unwrap_or(0);
            (-(i128::from(first) + desired_delta))
                .max(0)
                .min(i128::from(u64::MAX - self.clip_length)) as u64
        } else {
            0
        };
        let resolved = if self.kind == HitKind::Move {
            resolve_common_delta(
                &self.originals,
                u64::MAX,
                desired_delta + i128::from(source_prefix),
                i16::from(pointer_pitch) - i16::from(self.pointer_pitch),
            )
        } else {
            resize_delta(
                &self.originals,
                self.clip_length,
                self.kind,
                desired_delta,
                grid.minimum_gate(),
            )
            .map(|delta| (delta, 0))
        };
        let Ok((delta, pitch_delta)) = resolved else {
            return false;
        };
        if self.delta == delta
            && self.pitch_delta == pitch_delta
            && self.source_prefix == source_prefix
        {
            return false;
        }
        self.delta = delta;
        self.pitch_delta = pitch_delta;
        self.source_prefix = source_prefix;
        true
    }

    pub(super) fn is_noop(&self) -> bool {
        self.source_prefix == 0 && self.delta == 0 && self.pitch_delta == 0
    }

    /// Copy toggles only a move gesture; a captured resize keeps its edge.
    pub(super) fn set_copy(&mut self, copy: bool) -> bool {
        if self.kind != HitKind::Move || self.copy == copy {
            return false;
        }
        self.copy = copy;
        true
    }

    /// Promote the existing single-note move without resampling its pointer
    /// anchor. Both paths then share copy payloads and overwrite previews.
    pub(super) fn from_note_copy(
        note: midi_drag::NoteGesture,
        selection_epoch: u64,
    ) -> Result<Option<Self>, &'static str> {
        let midi_drag::GestureKind::Move {
            pointer_beat_raw,
            pointer_pitch,
        } = note.kind
        else {
            return Ok(None);
        };
        let mut originals = Vec::new();
        originals
            .try_reserve_exact(1)
            .map_err(|_| "could not reserve midi copy preview")?;
        originals.push(GroupNote {
            id: note.note,
            active: note.active,
            start: note.original.start,
            duration: note.original.duration,
            pitch: note.original.pitch,
            channel: note.channel,
            velocity: note.velocity,
        });
        Ok(Some(Self {
            copy: true,
            kind: HitKind::Move,
            generation: note.generation,
            revision: note.revision,
            clip: note.clip,
            selection_epoch,
            originals: Arc::from(originals),
            pointer_beat: pointer_beat_raw,
            pointer_pitch,
            bounds: note.bounds,
            beat_start: note.beat_start,
            beat_span: note.beat_span,
            pitch_low: note.pitch_low,
            pitch_rows: note.pitch_rows,
            clip_length: note.clip_length,
            grid: note.grid,
            delta: i128::from(note.preview.start) - i128::from(note.original.start),
            pitch_delta: i16::from(note.preview.pitch) - i16::from(note.original.pitch),
            source_prefix: 0,
        }))
    }

    pub(super) fn is_copy(&self) -> bool {
        self.copy
    }

    pub(super) fn copied_notes(&self) -> Result<Vec<hex_project::MidiNoteInsert>, &'static str> {
        if !self.copy || self.is_noop() {
            return Err("midi copy is a no-op");
        }
        let mut result = Vec::new();
        result
            .try_reserve_exact(self.originals.len())
            .map_err(|_| "could not reserve midi copies")?;
        for original in self.originals.iter() {
            let note = self
                .preview(original.id)
                .ok_or("midi copy target changed")?;
            result.push(hex_project::MidiNoteInsert {
                active: original.active,
                start: BeatTime::from_raw(note.start),
                duration: BeatTime::from_raw(note.duration),
                pitch: note.pitch,
                channel: note.channel,
                velocity: note.velocity,
            });
        }
        Ok(result)
    }

    /// Immutable admission context; borrowed input identity does not own notes.
    pub(super) fn captured_context(&self) -> GroupContext {
        GroupContext {
            generation: self.generation,
            revision: self.revision,
            clip: Some(self.clip),
            selection_epoch: self.selection_epoch,
            beat_start: self.beat_start,
            beat_span: self.beat_span,
            pitch_low: self.pitch_low,
            pitch_rows: self.pitch_rows,
            grid: self.grid,
            bounds: self.bounds,
            pending: false,
        }
    }

    pub(super) fn kind(&self) -> HitKind {
        self.kind
    }

    pub(super) fn shared_originals(&self) -> Arc<[GroupNote]> {
        Arc::clone(&self.originals)
    }

    pub(super) fn bounds(&self) -> [f32; 4] {
        self.bounds
    }

    pub(super) fn source_prefix(&self) -> u64 {
        self.source_prefix
    }

    pub(super) fn source_length(&self) -> u64 {
        self.clip_length
    }

    pub(super) fn preview(&self, id: NoteId) -> Option<GroupNote> {
        let index = self
            .originals
            .binary_search_by_key(&id, |note| note.id)
            .ok()?;
        let mut note = *self.originals.get(index)?;
        match self.kind {
            HitKind::Move => {
                note.start = (i128::from(note.start) + self.delta) as u64;
                note.pitch = (i16::from(note.pitch) + self.pitch_delta) as u8;
            }
            HitKind::ResizeLeading => {
                note.start = (i128::from(note.start) + self.delta) as u64;
                note.duration = (i128::from(note.duration) - self.delta) as u64;
            }
            HitKind::ResizeTrailing => {
                note.duration = (i128::from(note.duration) + self.delta) as u64
            }
        }
        Some(note)
    }

    pub(super) fn updates(&self) -> Result<Vec<MidiNoteUpdate>, &'static str> {
        if self.kind != HitKind::Move || self.source_prefix > 0 {
            if self.is_noop() {
                return Err("group midi edit is a no-op");
            }
            let mut updates = Vec::new();
            updates
                .try_reserve_exact(self.originals.len())
                .map_err(|_| "could not reserve group resize")?;
            for original in self.originals.iter() {
                let note = self
                    .preview(original.id)
                    .ok_or("group resize note changed")?;
                updates.push(MidiNoteUpdate {
                    note: note.id,
                    start: BeatTime::from_raw(note.start),
                    duration: BeatTime::from_raw(note.duration),
                    pitch: note.pitch,
                    channel: note.channel,
                    velocity: note.velocity,
                });
            }
            return Ok(updates);
        }
        nudge_updates(&self.originals, u64::MAX, self.delta, self.pitch_delta)?
            .ok_or("group midi edit is a no-op")
    }
}

/// One common edge delta keeps each member's opposite edge anchored. The
/// shortest member limits shrinking; preexisting sub-grid notes never jump to
/// a longer gate merely because a resize begins. Only explicit pointer motion
/// changes geometry, and all results stay within the captured source.
fn resize_delta(
    originals: &[GroupNote],
    length: u64,
    kind: HitKind,
    requested: i128,
    minimum: u64,
) -> Result<i128, &'static str> {
    if kind == HitKind::Move || minimum == 0 {
        return Err("invalid group resize");
    }
    // Reuse the same malformed note and source bounds checks as group movement.
    resolve_common_delta(originals, length, 0, 0)?;
    let mut lower = i128::MIN;
    let mut upper = i128::MAX;
    for note in originals {
        let minimum = minimum.min(note.duration);
        if kind == HitKind::ResizeLeading {
            lower = lower.max(-i128::from(note.start));
            upper = upper.min(i128::from(note.duration - minimum));
        } else {
            lower = lower.max(i128::from(minimum) - i128::from(note.duration));
            let end = note
                .start
                .checked_add(note.duration)
                .ok_or("invalid group resize end")?;
            upper = upper.min(i128::from(length - end));
        }
    }
    Ok(requested.clamp(lower, upper))
}

#[cfg(test)]
mod tests {
    use super::{
        GroupContext, GroupGesture, GroupNote, GroupParameter, GroupStart, capture_selected_notes,
        nudge_updates, parameter_updates,
    };
    use crate::editor::midi_drag::MidiGrid;
    use hex_project::{BeatTime, Command, Created, Editor, Project};

    const BOUNDS: [f32; 4] = [100.0, 200.0, 400.0, 240.0];
    const CLIP: u64 = 4 * BeatTime::ONE.raw();

    #[test]
    fn common_resize_limits_anchor_every_member_and_preserve_sub_grid_gates() {
        use super::{HitKind, resize_delta};
        let beat = BeatTime::ONE.raw();
        let f = fixture(&[(beat / 2, beat, 60), (2 * beat, beat / 8, 64)]);
        assert_eq!(
            resize_delta(&f.notes, CLIP, HitKind::ResizeLeading, i128::MIN, beat / 4).unwrap(),
            -i128::from(beat / 2)
        );
        assert_eq!(
            resize_delta(&f.notes, CLIP, HitKind::ResizeLeading, i128::MAX, beat / 4).unwrap(),
            0
        );
        assert_eq!(
            resize_delta(&f.notes, CLIP, HitKind::ResizeTrailing, i128::MIN, beat / 4).unwrap(),
            0
        );
        assert_eq!(
            resize_delta(&f.notes, CLIP, HitKind::ResizeTrailing, i128::MAX, beat / 4).unwrap(),
            i128::from(CLIP - 2 * beat - beat / 8)
        );
        assert!(resize_delta(&f.notes, CLIP, HitKind::ResizeLeading, 0, 0).is_err());
        assert!(resize_delta(&[], CLIP, HitKind::ResizeLeading, 0, 1).is_err());
        assert!(
            resize_delta(
                &[GroupNote {
                    start: u64::MAX,
                    ..f.notes[0]
                }],
                CLIP,
                HitKind::ResizeTrailing,
                1,
                1
            )
            .is_err()
        );
        for kind in [HitKind::ResizeLeading, HitKind::ResizeTrailing] {
            for grid in [
                MidiGrid::Quarter,
                MidiGrid::Eighth,
                MidiGrid::Sixteenth,
                MidiGrid::ThirtySecond,
                MidiGrid::Off,
            ] {
                let f = fixture(&[(beat, beat, 60), (2 * beat, beat / 2, 64)]);
                let originals = f.notes.clone();
                let mut drag = gesture(grid, f, [200.0, 315.0]);
                drag.kind = kind;
                assert!(drag.update(
                    [
                        if kind == HitKind::ResizeLeading {
                            100.0
                        } else {
                            300.0
                        },
                        335.0
                    ],
                    false
                ));
                for original in &originals {
                    let resized = drag.preview(original.id).unwrap();
                    assert_eq!(
                        (resized.pitch, resized.channel, resized.velocity),
                        (original.pitch, original.channel, original.velocity)
                    );
                    if kind == HitKind::ResizeLeading {
                        assert_eq!(
                            resized.start + resized.duration,
                            original.start + original.duration
                        );
                    } else {
                        assert_eq!(resized.start, original.start);
                    }
                    assert!(resized.duration > 0 && resized.start + resized.duration <= CLIP);
                }
                assert!(drag.updates().is_ok());
                drag.update([200.0, 315.0], false);
                assert!(drag.is_noop());
            }
        }
    }

    struct Fixture {
        editor: Editor,
        clip: hex_project::ClipId,
        notes: Vec<GroupNote>,
    }

    fn fixture(values: &[(u64, u64, u8)]) -> Fixture {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "instrument".into(),
            })
            .unwrap()
        else {
            panic!("track expected")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip expected")
        };
        let notes = values
            .iter()
            .enumerate()
            .map(|(index, &(start, duration, pitch))| {
                let Created::Note(id) = editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_raw(start),
                        duration: BeatTime::from_raw(duration),
                        pitch,
                        channel: index as u8,
                        velocity: 80 + index as u8,
                    })
                    .unwrap()
                else {
                    panic!("note expected")
                };
                GroupNote {
                    id,
                    active: true,
                    start,
                    duration,
                    pitch,
                    channel: index as u8,
                    velocity: 80 + index as u8,
                }
            })
            .collect();
        Fixture {
            editor,
            clip,
            notes,
        }
    }

    fn gesture(grid: MidiGrid, fixture: Fixture, pointer: [f32; 2]) -> GroupGesture {
        gesture_with_pitch(grid, fixture, pointer, 48)
    }

    fn gesture_with_pitch(
        grid: MidiGrid,
        fixture: Fixture,
        pointer: [f32; 2],
        pitch_low: u8,
    ) -> GroupGesture {
        gesture_of_kind(grid, fixture, pointer, pitch_low, super::HitKind::Move)
    }

    fn gesture_of_kind(
        grid: MidiGrid,
        fixture: Fixture,
        pointer: [f32; 2],
        pitch_low: u8,
        kind: super::HitKind,
    ) -> GroupGesture {
        GroupGesture::begin(GroupStart {
            copy: false,
            kind,
            generation: 3,
            revision: 7,
            clip: fixture.clip,
            selection_epoch: 4,
            originals: fixture.notes,
            pointer,
            bounds: BOUNDS,
            beat_start: 0,
            beat_span: CLIP,
            pitch_low,
            pitch_rows: 24,
            clip_length: CLIP,
            grid,
        })
        .expect("group gesture")
    }

    #[test]
    fn common_delta_preserves_spacing_pitch_intervals_and_note_expression() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat / 2, beat / 4, 60), (beat + beat / 2, beat / 8, 62)]);
        let first = fixture.notes[0];
        let second = fixture.notes[1];
        let mut drag = gesture(MidiGrid::Off, fixture, [150.0, 315.0]);
        assert!(drag.update([175.0, 305.0], false));
        let moved_first = drag.preview(first.id).unwrap();
        let moved_second = drag.preview(second.id).unwrap();
        assert_eq!(moved_first.start, first.start + beat / 4);
        assert_eq!(moved_second.start, second.start + beat / 4);
        assert_eq!(
            moved_second.start - moved_first.start,
            second.start - first.start
        );
        assert_eq!(moved_first.pitch, 61);
        assert_eq!(moved_second.pitch, 63);
        assert_eq!(moved_first.duration, first.duration);
        assert_eq!(moved_first.channel, first.channel);
        assert_eq!(moved_first.velocity, first.velocity);
        let updates = drag.updates().unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].note, first.id);
        assert_eq!(updates[1].note, second.id);
    }

    #[test]
    fn shared_move_crosses_source_end_preserving_spacing_and_numeric_gate_bounds() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat / 2, beat / 4, 60), (15 * beat / 4, beat / 4, 62)]);
        let first = fixture.notes[0];
        let edge = fixture.notes[1];
        let mut drag = gesture(MidiGrid::Off, fixture, [150.0, 315.0]);
        assert!(drag.update([250.0, 315.0], false));
        assert_eq!(drag.preview(first.id).unwrap().start, first.start + beat);
        assert_eq!(drag.preview(edge.id).unwrap().start, edge.start + beat);
        let updates = drag.updates().unwrap();
        assert_eq!(
            updates[1].start.raw() + updates[1].duration.raw(),
            CLIP + beat
        );
        let (delta, _) =
            super::resolve_common_delta(&drag.originals, u64::MAX, i128::MAX, 0).unwrap();
        assert_eq!(
            i128::from(edge.start + edge.duration) + delta,
            i128::from(u64::MAX)
        );
        assert!(drag.update([-10_000.0, 315.0], false));
        assert_eq!(drag.preview(first.id).unwrap().start, 0);
        assert_eq!(
            drag.preview(edge.id).unwrap().start,
            edge.start - first.start
        );
    }

    #[test]
    fn group_trailing_resize_is_free_by_default_and_snaps_while_cmd_is_held() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat, beat / 2, 60), (2 * beat, beat / 4, 61)]);
        let first = fixture.notes[0];
        let second = fixture.notes[1];
        let mut drag = gesture_of_kind(
            MidiGrid::Sixteenth,
            fixture,
            [250.0, 315.0],
            48,
            super::HitKind::ResizeTrailing,
        );
        let at = |x| crate::editor::midi_drag::beat_at_point(x, BOUNDS, 0, CLIP).unwrap();
        let free = at(265.0) - at(250.0); // about 0.15 beat
        assert!(drag.update([265.0, 315.0], false));
        assert_eq!(
            drag.preview(first.id).unwrap().duration,
            first.duration + free
        );
        assert_eq!(
            drag.preview(second.id).unwrap().duration,
            second.duration + free
        );
        assert_eq!(drag.preview(first.id).unwrap().start, first.start);
        assert!(drag.update([265.0, 315.0], true));
        assert_eq!(
            drag.preview(first.id).unwrap().duration,
            first.duration + beat / 4
        );
        assert_eq!(
            drag.preview(second.id).unwrap().duration,
            second.duration + beat / 4
        );
        assert!(drag.update([265.0, 315.0], false));
        assert_eq!(
            drag.preview(first.id).unwrap().duration,
            first.duration + free
        );
    }

    #[test]
    fn fixed_free_modifier_and_noop_are_resolved_from_the_original_anchor() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat, beat / 4, 60), (2 * beat, beat / 4, 61)]);
        let first = fixture.notes[0];
        let mut drag = gesture(MidiGrid::Sixteenth, fixture, [150.0, 315.0]);
        assert!(!drag.update([150.0, 315.0], true));
        assert!(!drag.update([156.25, 315.0], false));
        assert!(drag.update([156.25, 315.0], true));
        assert_eq!(
            drag.preview(first.id).unwrap().start,
            first.start + beat / 16
        );
        assert!(drag.update([156.25, 315.0], false));
        assert!(drag.is_noop());
        assert_eq!(drag.preview(first.id).unwrap().start, first.start);
    }

    #[test]
    fn group_pitch_delta_clamps_to_every_member_and_stale_selection_cancels() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat, beat / 4, 126), (2 * beat, beat / 4, 127)]);
        let mut drag = gesture_with_pitch(MidiGrid::Off, fixture, [150.0, 215.0], 104);
        assert!(!drag.update([150.0, 205.0], false));
        let note_id = drag.shared_originals()[0].id;
        assert_eq!(drag.preview(note_id).unwrap().pitch, 126);
        let mut context = GroupContext {
            generation: 3,
            revision: 7,
            clip: Some(drag.clip),
            selection_epoch: 5,
            beat_start: 0,
            beat_span: CLIP,
            pitch_low: 104,
            pitch_rows: 24,
            grid: MidiGrid::Off,
            bounds: BOUNDS,
            pending: false,
        };
        assert!(!drag.is_current(context));
        context.selection_epoch = 4;
        assert!(drag.is_current(context));
    }

    #[test]
    fn keyboard_nudge_preserves_mixed_off_grid_timing_gate_and_expression() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat / 3, beat / 8, 60), (beat + beat / 7, beat / 512, 63)]);
        let before = fixture.notes.clone();
        let updates = nudge_updates(&before, CLIP, i128::from(beat / 4), 1)
            .unwrap()
            .unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].start.raw(), before[0].start + beat / 4);
        assert_eq!(updates[1].start.raw(), before[1].start + beat / 4);
        for (update, original) in updates.iter().zip(&before) {
            assert_eq!(update.duration.raw(), original.duration);
            assert_eq!(update.pitch, original.pitch + 1);
            assert_eq!(update.channel, original.channel);
            assert_eq!(update.velocity, original.velocity);
        }
    }

    #[test]
    fn octave_nudge_preserves_pitch_intervals_and_note_metadata() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat / 3, beat / 8, 60), (beat + beat / 7, beat / 512, 65)]);
        let before = fixture.notes;
        let updates = nudge_updates(&before, CLIP, 0, 12).unwrap().unwrap();
        assert_eq!(updates[0].pitch, 72);
        assert_eq!(updates[1].pitch, 77);
        assert_eq!(i16::from(updates[1].pitch) - i16::from(updates[0].pitch), 5);
        for (update, original) in updates.iter().zip(&before) {
            assert_eq!(update.start.raw(), original.start);
            assert_eq!(update.duration.raw(), original.duration);
            assert_eq!(update.channel, original.channel);
            assert_eq!(update.velocity, original.velocity);
        }
    }

    #[test]
    fn octave_nudge_uses_one_common_edge_delta_and_noops_when_blocked() {
        let beat = BeatTime::ONE.raw();
        let near_edge = fixture(&[(beat, beat / 8, 115), (2 * beat, beat / 8, 120)]);
        let updates = nudge_updates(&near_edge.notes, CLIP, 0, 12)
            .unwrap()
            .unwrap();
        assert_eq!(updates[0].pitch, 122);
        assert_eq!(updates[1].pitch, 127);
        assert_eq!(i16::from(updates[1].pitch) - i16::from(updates[0].pitch), 5);

        let blocked = fixture(&[(beat, beat / 8, 121), (2 * beat, beat / 8, 127)]);
        assert!(
            nudge_updates(&blocked.notes, CLIP, 0, 12)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn group_parameter_edits_change_only_the_requested_field_and_preserve_expression() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat / 3, beat / 8, 60), (beat + beat / 7, beat / 512, 63)]);
        let before = fixture.notes;

        let pitch = parameter_updates(&before, CLIP, GroupParameter::Pitch(1), beat / 16)
            .unwrap()
            .unwrap();
        let length = parameter_updates(&before, CLIP, GroupParameter::Duration(1), beat / 16)
            .unwrap()
            .unwrap();
        let velocity = parameter_updates(&before, CLIP, GroupParameter::Velocity(-8), beat / 16)
            .unwrap()
            .unwrap();

        for (index, original) in before.iter().enumerate() {
            for update in [&pitch[index], &length[index], &velocity[index]] {
                assert_eq!(update.note, original.id);
                assert_eq!(update.start.raw(), original.start);
                assert_eq!(update.channel, original.channel);
            }
            assert_eq!(pitch[index].duration.raw(), original.duration);
            assert_eq!(pitch[index].pitch, original.pitch + 1);
            assert_eq!(pitch[index].velocity, original.velocity);
            assert_eq!(length[index].duration.raw(), original.duration + beat / 16);
            assert_eq!(length[index].pitch, original.pitch);
            assert_eq!(length[index].velocity, original.velocity);
            assert_eq!(velocity[index].duration.raw(), original.duration);
            assert_eq!(velocity[index].pitch, original.pitch);
            assert_eq!(velocity[index].velocity, original.velocity - 8);
        }
    }

    #[test]
    fn group_parameter_edits_refuse_the_whole_batch_at_any_member_boundary() {
        let beat = BeatTime::ONE.raw();
        let pitch_edge = fixture(&[(beat, beat / 8, 126), (2 * beat, beat / 8, 127)]);
        assert_eq!(
            parameter_updates(&pitch_edge.notes, CLIP, GroupParameter::Pitch(1), beat / 16,),
            Err("group pitch edit exceeds midi range")
        );

        let short_edge = fixture(&[(beat, beat / 32, 60), (2 * beat, beat / 8, 62)]);
        assert_eq!(
            parameter_updates(
                &short_edge.notes,
                CLIP,
                GroupParameter::Duration(-1),
                beat / 16,
            ),
            Err("group length edit exceeds note range")
        );

        let end_edge = fixture(&[(beat, beat / 8, 60), (CLIP - beat / 16, beat / 16, 62)]);
        assert_eq!(
            parameter_updates(
                &end_edge.notes,
                CLIP,
                GroupParameter::Duration(1),
                beat / 16,
            ),
            Err("group length edit exceeds clip bounds")
        );

        let velocity_edge = fixture(&[(beat, beat / 8, 60), (2 * beat, beat / 8, 62)]);
        let mut notes = velocity_edge.notes;
        notes[1].velocity = 4;
        assert_eq!(
            parameter_updates(&notes, CLIP, GroupParameter::Velocity(-8), beat / 16),
            Err("group velocity edit exceeds midi range")
        );

        let velocity_high_edge = fixture(&[(beat, beat / 8, 60), (2 * beat, beat / 8, 62)]);
        let mut notes = velocity_high_edge.notes;
        notes[1].velocity = 120;
        assert_eq!(
            parameter_updates(&notes, CLIP, GroupParameter::Velocity(8), beat / 16),
            Err("group velocity edit exceeds midi range")
        );
    }

    #[test]
    fn group_parameter_noop_returns_no_batch() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat, beat / 8, 60), (2 * beat, beat / 8, 62)]);
        assert!(
            parameter_updates(&fixture.notes, CLIP, GroupParameter::Pitch(0), beat / 16,)
                .unwrap()
                .is_none()
        );
        assert!(
            parameter_updates(&fixture.notes, CLIP, GroupParameter::Duration(0), beat / 16,)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn selected_group_capture_rejects_missing_or_stale_ids() {
        let beat = BeatTime::ONE.raw();
        let fixture = fixture(&[(beat, beat / 8, 60), (2 * beat, beat / 8, 62)]);
        let selected = [fixture.notes[0].id, fixture.notes[1].id];
        let captured =
            capture_selected_notes(fixture.notes.clone().into_iter(), &selected).unwrap();
        assert_eq!(captured, fixture.notes);

        assert_eq!(
            capture_selected_notes(
                fixture.notes[..1].iter().copied(),
                &[selected[0], selected[1]],
            ),
            Err("selected midi notes changed · edit canceled")
        );
        assert_eq!(
            capture_selected_notes(fixture.notes.into_iter(), &[selected[1], selected[0]]),
            Err("invalid selected midi note ids")
        );
    }

    #[test]
    fn keyboard_nudge_clamps_common_onset_and_pitch_deltas_at_every_boundary() {
        let beat = BeatTime::ONE.raw();
        let at_start = fixture(&[(0, beat / 8, 0), (beat / 3, beat / 512, 2)]);
        assert!(
            nudge_updates(&at_start.notes, CLIP, -i128::from(beat / 4), -1)
                .unwrap()
                .is_none()
        );

        let at_end = fixture(&[
            (CLIP - beat / 4, beat / 4, 125),
            (CLIP - beat / 8, beat / 8, 127),
        ]);
        assert!(
            nudge_updates(&at_end.notes, CLIP, i128::from(beat / 4), 1)
                .unwrap()
                .is_none()
        );

        let mixed = fixture(&[(beat, beat / 8, 0), (2 * beat, beat / 8, 127)]);
        let updates = nudge_updates(&mixed.notes, CLIP, i128::from(beat / 4), 1)
            .unwrap()
            .unwrap();
        assert!(updates.iter().all(|update| update.start.raw() > 0));
        assert_eq!(updates[0].pitch, 0);
        assert_eq!(updates[1].pitch, 127);
    }

    #[test]
    fn one_group_keyboard_batch_is_one_undo_redo_step_and_noop_keeps_redo() {
        let beat = BeatTime::ONE.raw();
        let mut project_fixture =
            fixture(&[(beat / 3, beat / 8, 60), (beat + beat / 7, beat / 512, 63)]);
        let before = project_fixture.editor.project().clone();
        let updates = nudge_updates(&project_fixture.notes, CLIP, i128::from(beat / 4), 1)
            .unwrap()
            .unwrap();
        project_fixture
            .editor
            .execute(Command::UpdateMidiNotes {
                clip: project_fixture.clip,
                notes: updates,
            })
            .unwrap();
        let after = project_fixture.editor.project().clone();
        assert_ne!(after, before);
        assert!(project_fixture.editor.undo());
        assert_eq!(project_fixture.editor.project(), &before);
        assert!(project_fixture.editor.redo());
        assert_eq!(project_fixture.editor.project(), &after);
        assert!(project_fixture.editor.undo());
        assert!(project_fixture.editor.can_redo());
        let boundary = fixture(&[(0, beat / 4, 60), (beat, beat / 8, 63)]);
        assert!(
            nudge_updates(&boundary.notes, CLIP, -i128::from(beat), 0)
                .unwrap()
                .is_none()
        );
        assert!(project_fixture.editor.can_redo());
    }

    #[test]
    fn one_group_parameter_batch_is_one_undo_redo_step_and_leaves_unselected_notes_unchanged() {
        let beat = BeatTime::ONE.raw();
        let mut project_fixture = fixture(&[
            (beat / 3, beat / 8, 60),
            (beat + beat / 7, beat / 512, 63),
            (3 * beat, beat / 4, 71),
        ]);
        let before = project_fixture.editor.project().clone();
        let updates = parameter_updates(
            &project_fixture.notes[..2],
            CLIP,
            GroupParameter::Velocity(8),
            beat / 16,
        )
        .unwrap()
        .unwrap();
        project_fixture
            .editor
            .execute(Command::UpdateMidiNotes {
                clip: project_fixture.clip,
                notes: updates,
            })
            .unwrap();
        let after = project_fixture.editor.project().clone();
        assert_ne!(after, before);
        assert_eq!(
            after.midi_note(project_fixture.notes[2].id).unwrap(),
            before.midi_note(project_fixture.notes[2].id).unwrap()
        );
        assert!(project_fixture.editor.undo());
        assert_eq!(project_fixture.editor.project(), &before);
        assert!(project_fixture.editor.redo());
        assert_eq!(project_fixture.editor.project(), &after);
    }

    #[test]
    fn one_octave_group_edit_is_one_history_step_and_preserves_unselected_metadata() {
        let beat = BeatTime::ONE.raw();
        let mut project_fixture = fixture(&[
            (beat / 3, beat / 8, 60),
            (beat + beat / 7, beat / 512, 65),
            (3 * beat, beat / 4, 71),
        ]);
        let before = project_fixture.editor.project().clone();
        let selected = &project_fixture.notes[..2];
        let updates = nudge_updates(selected, CLIP, 0, 12).unwrap().unwrap();
        project_fixture
            .editor
            .execute(Command::UpdateMidiNotes {
                clip: project_fixture.clip,
                notes: updates,
            })
            .unwrap();
        let after = project_fixture.editor.project().clone();
        assert_ne!(after, before);
        for note in selected {
            let changed = after.midi_note(note.id).unwrap();
            assert_eq!(changed.pitch(), note.pitch + 12);
            assert_eq!(changed.start().raw(), note.start);
            assert_eq!(changed.duration().raw(), note.duration);
            assert_eq!(changed.channel(), note.channel);
            assert_eq!(changed.velocity(), note.velocity);
        }
        let unselected = project_fixture.notes[2];
        assert_eq!(
            after.midi_note(unselected.id).unwrap(),
            before.midi_note(unselected.id).unwrap()
        );
        assert!(project_fixture.editor.undo());
        assert_eq!(project_fixture.editor.project(), &before);
        assert!(project_fixture.editor.redo());
        assert_eq!(project_fixture.editor.project(), &after);
    }
}
