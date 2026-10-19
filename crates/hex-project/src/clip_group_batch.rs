//! Private complete final-state plan; no intermediate Project exceeds capacity.
use super::{ClipGroupContent, ClipGroupPreview};
use crate::{
    BeatTime, Clip, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MidiClip, NoteId, Project,
};
use std::{collections::BTreeSet, sync::Arc};

type Interval = (u64, u64);
#[derive(Default)]
struct Row {
    frames: Vec<Interval>,
    beats: Vec<Interval>,
    audio: Vec<Clip>,
    midi: Vec<MidiClip>,
}
struct Counts {
    clips: usize,
    notes: usize,
    occurrences: usize,
}
impl Counts {
    fn clip(&mut self, notes: usize) -> Result<(), Invalid> {
        self.clips = self
            .clips
            .checked_add(1)
            .filter(|n| *n <= MAX_CLIPS)
            .ok_or(Invalid("clip limit reached"))?;
        self.notes = self
            .notes
            .checked_add(notes)
            .filter(|n| *n <= MAX_NOTES)
            .ok_or(Invalid("midi note limit reached"))?;
        Ok(())
    }
    fn enumerate(&mut self, clip: &MidiClip) -> Result<(), Invalid> {
        self.occurrences = self
            .occurrences
            .checked_add(crate::midi::overwrite_occurrence_count(clip)?)
            .filter(|n| *n <= MAX_NOTES)
            .ok_or(Invalid(
                "midi overwrite note expansion exceeds project limit",
            ))?;
        Ok(())
    }
}
fn push<T>(items: &mut Vec<T>, item: T) -> Result<(), Invalid> {
    items
        .try_reserve(1)
        .map_err(|_| Invalid("could not reserve group overwrite storage"))?;
    items.push(item);
    Ok(())
}
fn union(intervals: &mut Vec<Interval>) {
    intervals.sort_unstable();
    let mut kept = 0;
    for index in 0..intervals.len() {
        let (start, end) = intervals[index];
        if start >= end {
            continue;
        }
        if kept > 0 && start <= intervals[kept - 1].1 {
            intervals[kept - 1].1 = intervals[kept - 1].1.max(end);
        } else {
            intervals[kept] = (start, end);
            kept += 1;
        }
    }
    intervals.truncate(kept);
}
/// None means untouched (including valid sub-frame MIDI); Some(empty) removed.
fn remainder(start: u64, end: u64, cuts: &[Interval]) -> Result<Option<Vec<Interval>>, Invalid> {
    let first = cuts.partition_point(|(_, right)| *right <= start);
    if cuts.get(first).is_none_or(|(left, _)| *left >= end) {
        return Ok(None);
    }
    let mut pieces = Vec::new();
    let mut cursor = start;
    for &(left, right) in &cuts[first..] {
        if left >= end {
            break;
        }
        if cursor < left {
            push(&mut pieces, (cursor, left.min(end)))?;
        }
        cursor = cursor.max(right).min(end);
        if cursor == end {
            break;
        }
    }
    if cursor < end {
        push(&mut pieces, (cursor, end))?;
    }
    Ok(Some(pieces))
}
fn private_copy_source(clip: &mut MidiClip) -> Result<(), Invalid> {
    let old = &clip.source;
    let mut notes = Vec::new();
    notes
        .try_reserve_exact(old.notes.len())
        .map_err(|_| Invalid("could not reserve copied midi notes"))?;
    notes.extend_from_slice(&old.notes);
    clip.source = Arc::new(crate::midi::MidiSource {
        source_length: old.source_length,
        source_start: old.source_start,
        loop_start: old.loop_start,
        loop_end: old.loop_end,
        looping: old.looping,
        notes,
    });
    Ok(())
}
fn identity(cursor: &mut u64) -> Result<u64, Invalid> {
    let id = *cursor;
    *cursor = cursor
        .checked_add(1)
        .ok_or(Invalid("project id space exhausted"))?;
    Ok(id)
}
fn id_charge(total: &mut usize, count: usize) -> Result<(), Invalid> {
    *total = total
        .checked_add(count)
        .ok_or(Invalid("project id space exhausted"))?;
    Ok(())
}

pub(super) fn apply(
    project: &mut Project,
    mut incoming: Vec<ClipGroupPreview>,
    selected: &BTreeSet<ClipId>,
    copy: bool,
) -> Result<Created, Invalid> {
    let mut rows = Vec::new();
    rows.try_reserve_exact(project.tracks.len())
        .map_err(|_| Invalid("could not reserve group row plan"))?;
    rows.resize_with(project.tracks.len(), Row::default);
    let tempo = project.tempo_map();
    let mut count = Counts {
        clips: 0,
        notes: 0,
        occurrences: 0,
    };
    for cell in &project.session.cells {
        match &cell.content {
            crate::SessionCellContent::Audio(_) => count.clip(0)?,
            crate::SessionCellContent::Midi(clip) => count.clip(clip.source.notes.len())?,
            crate::SessionCellContent::Stop => (),
        }
    }
    // All incoming ownership is part of the final count, before any fragments.
    for member in &incoming {
        let index = project
            .tracks
            .iter()
            .position(|row| row.id() == member.track)
            .ok_or(Invalid("destination track no longer exists"))?;
        let row = &mut rows[index];
        let (frames, beats) = match &member.content {
            ClipGroupContent::Audio(clip) => {
                count.clip(0)?;
                let end = clip
                    .start()
                    .checked_add(clip.duration())
                    .ok_or(Invalid("group audio end overflow"))?;
                (
                    (clip.start(), end),
                    crate::midi::midi_overwrite_frame_bounds(tempo, clip.start(), end)?,
                )
            }
            ClipGroupContent::Midi(clip) => {
                count.clip(clip.notes().len())?;
                let end = BeatTime::from_raw(
                    clip.start()
                        .raw()
                        .checked_add(clip.length().raw())
                        .ok_or(Invalid("group midi end overflow"))?,
                );
                (
                    (
                        tempo.beat_to_frame(clip.start())?,
                        tempo.beat_to_frame(end)?,
                    ),
                    (clip.start(), end),
                )
            }
        };
        if frames.0 < frames.1 {
            push(&mut row.frames, frames)?;
        }
        if beats.0 < beats.1 {
            push(&mut row.beats, (beats.0.raw(), beats.1.raw()))?;
        }
    }
    for (original_row, row) in project.tracks.iter().zip(&mut rows) {
        union(&mut row.frames);
        union(&mut row.beats);
        for original in original_row.clips() {
            if selected.contains(&original.id()) {
                if copy {
                    count.clip(0)?;
                    push(&mut row.audio, original.clone())?;
                }
                continue;
            }
            let end = original
                .start()
                .checked_add(original.duration())
                .ok_or(Invalid("timeline end overflow"))?;
            if let Some(pieces) = remainder(original.start(), end, &row.frames)? {
                for (index, (start, end)) in pieces.into_iter().enumerate() {
                    count.clip(0)?;
                    let id = if index == 0 { original.id() } else { ClipId(0) };
                    push(
                        &mut row.audio,
                        crate::edit::retained_audio_fragment(original, start, end, id, tempo)?,
                    )?;
                }
            } else {
                count.clip(0)?;
                push(&mut row.audio, original.clone())?;
            }
        }
        for original in original_row.midi_clips() {
            if selected.contains(&original.id()) {
                if copy {
                    count.clip(original.notes().len())?;
                    push(&mut row.midi, original.clone())?;
                }
                continue;
            }
            let end = original
                .start()
                .raw()
                .checked_add(original.length().raw())
                .ok_or(Invalid("midi timeline end overflow"))?;
            let Some(mut pieces) = remainder(original.start().raw(), end, &row.beats)? else {
                count.clip(original.notes().len())?;
                push(&mut row.midi, original.clone())?;
                continue;
            };
            // Match single-interval positive integer-frame fragment eligibility.
            let mut kept = 0;
            for index in 0..pieces.len() {
                let (start, end) = pieces[index];
                if tempo.beat_to_frame(BeatTime::from_raw(start))?
                    < tempo.beat_to_frame(BeatTime::from_raw(end))?
                {
                    pieces[kept] = (start, end);
                    kept += 1;
                }
            }
            pieces.truncate(kept);
            let prefix = pieces
                .first()
                .is_some_and(|(start, _)| *start == original.start().raw());
            if prefix {
                let mut clip = original.clone();
                clip.length = BeatTime::from_raw(pieces[0].1 - pieces[0].0);
                count.clip(clip.notes().len())?;
                push(&mut row.midi, clip)?;
            }
            let materialized = &pieces[usize::from(prefix)..];
            if !materialized.is_empty() {
                count.enumerate(original)?;
                for (index, mut clip) in
                    crate::midi::materialize_overwrite_ranges(original, materialized)?
                        .into_iter()
                        .enumerate()
                {
                    clip.id = if !prefix && index == 0 {
                        original.id()
                    } else {
                        ClipId(0)
                    };
                    count.clip(clip.notes().len())?;
                    push(&mut row.midi, clip)?;
                }
            }
        }
    }
    let mut identities = 0;
    for row in &rows {
        id_charge(
            &mut identities,
            row.audio
                .iter()
                .filter(|clip| clip.id() == ClipId(0))
                .count(),
        )?;
        for clip in &row.midi {
            id_charge(&mut identities, usize::from(clip.id() == ClipId(0)))?;
            id_charge(
                &mut identities,
                clip.notes()
                    .iter()
                    .filter(|note| note.id() == NoteId(0))
                    .count(),
            )?;
        }
    }
    // Reserve each complete final row, not one future push repeatedly.
    for (original, row) in project.tracks.iter().zip(&mut rows) {
        let audio = incoming
            .iter()
            .filter(|member| {
                member.track == original.id()
                    && matches!(member.content, ClipGroupContent::Audio(_))
            })
            .count();
        let midi = incoming
            .iter()
            .filter(|member| {
                member.track == original.id() && matches!(member.content, ClipGroupContent::Midi(_))
            })
            .count();
        row.audio
            .try_reserve_exact(audio)
            .map_err(|_| Invalid("could not reserve incoming audio"))?;
        row.midi
            .try_reserve_exact(midi)
            .map_err(|_| Invalid("could not reserve incoming midi"))?;
    }
    // Prepare unique note storage before reserving IDs or installing anything.
    if copy {
        for member in &mut incoming {
            id_charge(&mut identities, 1)?;
            if let ClipGroupContent::Midi(clip) = &mut member.content {
                id_charge(&mut identities, clip.notes().len())?;
                private_copy_source(clip)?;
            }
        }
    }
    let mut cursor = if identities > 0 {
        project.allocate_id_range(identities)?.0
    } else {
        project.next_id
    };
    // Every overwrite-fragment birth precedes the first copied ClipId.
    for row in &mut rows {
        for clip in &mut row.audio {
            if clip.id == ClipId(0) {
                clip.id = ClipId(identity(&mut cursor)?);
            }
        }
        for clip in &mut row.midi {
            if clip.id == ClipId(0) {
                clip.id = ClipId(identity(&mut cursor)?);
            }
            if clip.notes().iter().any(|note| note.id() == NoteId(0)) {
                let source = Arc::get_mut(&mut clip.source)
                    .ok_or(Invalid("materialized midi source was unexpectedly shared"))?;
                for note in &mut source.notes {
                    note.id = NoteId(identity(&mut cursor)?);
                }
            }
        }
    }
    let mut first_copy = None;
    let mut last_copy = None;
    for member in incoming {
        let index = project
            .tracks
            .iter()
            .position(|row| row.id() == member.track)
            .ok_or(Invalid("destination track no longer exists"))?;
        let id = match member.content {
            ClipGroupContent::Audio(mut clip) => {
                if copy {
                    clip.id = ClipId(identity(&mut cursor)?);
                }
                let id = clip.id;
                rows[index].audio.push(clip);
                id
            }
            ClipGroupContent::Midi(mut clip) => {
                if copy {
                    clip.id = ClipId(identity(&mut cursor)?);
                    let source = Arc::get_mut(&mut clip.source)
                        .ok_or(Invalid("copied midi source was unexpectedly shared"))?;
                    for note in &mut source.notes {
                        note.id = NoteId(identity(&mut cursor)?);
                    }
                }
                let id = clip.id;
                rows[index].midi.push(clip);
                id
            }
        };
        if copy {
            first_copy.get_or_insert(id);
            last_copy = Some(id);
        }
    }
    if cursor != project.next_id {
        return Err(Invalid(
            "group identity reservation disagrees with final plan",
        ));
    }
    // Remove old Arrangement ownership first; installing row replacements can
    // never produce an over-capacity intermediate Project.
    for row in &mut project.tracks {
        row.content.audio_clips.clear();
        row.content.midi_clips.clear();
    }
    for (row, replacement) in project.tracks.iter_mut().zip(rows) {
        row.content.audio_clips = replacement.audio;
        row.content.midi_clips = replacement.midi;
    }
    Ok(match (first_copy, last_copy) {
        (Some(first), Some(last)) => Created::Clips { first, last },
        _ => Created::None,
    })
}
