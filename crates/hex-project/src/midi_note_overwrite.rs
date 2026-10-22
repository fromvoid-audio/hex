//! Same-source/pitch authoring: later input wins, retaining only an older left prefix.
//! Channels remain musical data, not separate collision lanes. The bounded union
//! of authored intervals and stable-ID indexing use bounded
//! O((existing + authored) log(existing + authored)) work. All
//! selected originals are excluded together. Authors then run in input order;
//! an erased author's earlier destructive cuts do not resurrect old right tails.
use crate::{BeatTime, Invalid, MAX_NOTES, MidiNote, MidiNoteInsert, MidiSource, NoteId, Project};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

/// Current-format sources must already obey the authoring invariant. Refuse an
/// overlapping saved source; loading never silently trims or deletes its notes.
/// Repeated loop occurrences and release/DSP tails are not authored intervals.
pub(crate) fn validate_occupancy(notes: &[MidiNote]) -> Result<(), Invalid> {
    if notes.len() > MAX_NOTES {
        return Err(Invalid("midi note limit reached"));
    }
    if notes.len() < 2 {
        return Ok(());
    }
    let mut intervals = Vec::new();
    intervals
        .try_reserve_exact(notes.len())
        .map_err(|_| Invalid("could not reserve midi note occupancy validation"))?;
    for note in notes {
        if note.pitch > 127 || note.duration == BeatTime::ZERO {
            return Err(Invalid("invalid midi note occupancy"));
        }
        let end = note
            .start
            .raw()
            .checked_add(note.duration.raw())
            .ok_or(Invalid("midi note end overflow"))?;
        intervals.push((note.pitch, note.start.raw(), end));
    }
    intervals.sort_unstable();
    if intervals.windows(2).any(|pair| {
        matches!(pair, [left, right]
        if left.0 == right.0 && left.2 > right.1)
    }) {
        return Err(Invalid("midi notes on the same pitch must not overlap"));
    }
    Ok(())
}

fn sources(project: &Project) -> impl Iterator<Item = &Arc<MidiSource>> {
    project
        .tracks
        .iter()
        .flat_map(|track| track.content.midi_clips.iter().map(|clip| &clip.source))
        .chain(
            project
                .session
                .cells
                .iter()
                .filter_map(|cell| match &cell.content {
                    crate::SessionCellContent::Midi(clip) => Some(&clip.source),
                    _ => None,
                }),
        )
}

impl Project {
    /// Final private-candidate guard, including copy/flatten/alternate callers.
    /// Unchanged COW sources need no note traversal. Borrowing both projects
    /// prevents source-address reuse; this local identity set owns no sources.
    pub(crate) fn validate_changed_midi_occupancy(&self, before: &Self) -> Result<(), Invalid> {
        if sources(self)
            .map(Arc::as_ptr)
            .eq(sources(before).map(Arc::as_ptr))
        {
            return Ok(());
        }
        let mut validated = HashSet::new();
        validated
            .try_reserve(sources(before).count())
            .map_err(|_| Invalid("could not reserve midi source validation identities"))?;
        validated.extend(sources(before).map(Arc::as_ptr));
        for source in sources(self) {
            if !validated.contains(&Arc::as_ptr(source)) {
                validate_occupancy(&source.notes)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiNoteOverwriteChange {
    Remove,
    /// Positive duration of the original note's retained left prefix.
    TruncateTo(BeatTime),
}

/// Pure per-source geometry query shared with canonical authoring. Input order is
/// priority order; later authored notes win. All removed originals are excluded
/// together, so selected members cannot collide with their old positions.
/// Bind a preview to its actual source/revision and discard it when that changes.
/// This query checks numeric/batch bounds, not source extent, final project
/// capacity or playback admission;
/// the explicit source-extent command and strict edit commands retain their rules.
pub struct MidiNoteOverwritePreview {
    pub(crate) retained: Vec<Option<BeatTime>>,
    pub(crate) authored: Vec<Option<BeatTime>>,
    changes: Vec<(NoteId, MidiNoteOverwriteChange)>,
}
impl MidiNoteOverwritePreview {
    /// None means this existing note is unchanged. Selected originals appear as
    /// Remove and are painted separately using their authored geometry.
    pub fn existing_change(&self, note: NoteId) -> Option<MidiNoteOverwriteChange> {
        self.changes
            .binary_search_by_key(&note, |(id, _)| *id)
            .ok()
            .and_then(|index| self.changes.get(index).map(|(_, change)| *change))
    }

    /// Final positive gate of an authored member, or None if a later member
    /// removes it (also None for an index outside this query's input).
    pub fn authored_duration(&self, index: usize) -> Option<BeatTime> {
        self.authored.get(index).copied().flatten()
    }
}

impl MidiSource {
    pub fn preview_note_overwrite(
        &self,
        removed: &[NoteId],
        authored: &[MidiNoteInsert],
    ) -> Result<MidiNoteOverwritePreview, Invalid> {
        if self.notes.len() > MAX_NOTES || authored.len() > MAX_NOTES {
            return Err(Invalid("midi note batch exceeds note limit"));
        }
        let mut removed = if removed.is_empty() {
            Vec::new()
        } else {
            crate::midi::note_indices(self, removed.iter().copied())?
        };
        removed.sort_unstable();
        for note in authored {
            crate::midi::validate_note(
                note.start,
                note.duration,
                BeatTime::from_raw(u64::MAX),
                note.pitch,
                note.channel,
                note.velocity,
            )?;
        }
        let mut gates = Vec::new();
        gates
            .try_reserve_exact(authored.len())
            .map_err(|_| Invalid("could not reserve midi overwrite geometry"))?;
        gates.resize(authored.len(), None);
        let mut union: [BTreeMap<u64, u64>; 128] = std::array::from_fn(|_| BTreeMap::new());
        for (note, gate) in authored.iter().zip(gates.iter_mut()).rev() {
            let end = note
                .start
                .raw()
                .checked_add(note.duration.raw())
                .ok_or(Invalid("midi note end overflow"))?;
            let intervals = union
                .get_mut(usize::from(note.pitch))
                .ok_or(Invalid("midi note pitch must be 0..127"))?;
            *gate = prefix(intervals, note.start.raw(), end);
            cover(intervals, note.start.raw(), end);
        }
        let mut retained = Vec::new();
        let mut changes = Vec::new();
        retained
            .try_reserve_exact(self.notes.len())
            .map_err(|_| Invalid("could not reserve retained midi geometry"))?;
        changes
            .try_reserve(self.notes.len())
            .map_err(|_| Invalid("could not reserve midi overwrite changes"))?;
        let mut removed = removed.into_iter().peekable();
        for (index, note) in self.notes.iter().enumerate() {
            let gate = if removed.peek() == Some(&index) {
                removed.next();
                None
            } else {
                let intervals = union
                    .get(usize::from(note.pitch))
                    .ok_or(Invalid("midi note pitch must be 0..127"))?;
                let end = note
                    .start
                    .raw()
                    .checked_add(note.duration.raw())
                    .ok_or(Invalid("midi note end overflow"))?;
                prefix(intervals, note.start.raw(), end)
            };
            retained.push(gate);
            if gate != Some(note.duration) {
                changes.push((
                    note.id,
                    gate.map_or(
                        MidiNoteOverwriteChange::Remove,
                        MidiNoteOverwriteChange::TruncateTo,
                    ),
                ));
            }
        }
        changes.sort_unstable_by_key(|(id, _)| *id);
        Ok(MidiNoteOverwritePreview {
            retained,
            authored: gates,
            changes,
        })
    }
}

/// Recheck a minted history edge against its final authors, without retaining
/// source owners in the receipt. Runs on control, never on an audio callback.
pub(crate) fn prefixes(
    existing: &[MidiNote],
    authored: &[MidiNote],
) -> Result<Vec<Option<BeatTime>>, Invalid> {
    if existing.len() > MAX_NOTES || authored.len() > MAX_NOTES {
        return Err(Invalid("midi note batch exceeds note limit"));
    }
    let mut union: [BTreeMap<u64, u64>; 128] = std::array::from_fn(|_| BTreeMap::new());
    for note in authored {
        let intervals = union
            .get_mut(usize::from(note.pitch))
            .ok_or(Invalid("midi note pitch must be 0..127"))?;
        let end = note
            .start
            .raw()
            .checked_add(note.duration.raw())
            .ok_or(Invalid("midi note end overflow"))?;
        cover(intervals, note.start.raw(), end);
    }
    let mut gates = Vec::new();
    gates
        .try_reserve_exact(existing.len())
        .map_err(|_| Invalid("could not reserve midi history geometry"))?;
    for note in existing {
        let intervals = union
            .get(usize::from(note.pitch))
            .ok_or(Invalid("midi note pitch must be 0..127"))?;
        let end = note
            .start
            .raw()
            .checked_add(note.duration.raw())
            .ok_or(Invalid("midi note end overflow"))?;
        gates.push(prefix(intervals, note.start.raw(), end));
    }
    Ok(gates)
}

fn prefix(intervals: &BTreeMap<u64, u64>, start: u64, end: u64) -> Option<BeatTime> {
    if intervals
        .range(..=start)
        .next_back()
        .is_some_and(|(_, end)| *end > start)
    {
        return None;
    }
    let cut = intervals
        .range(start..end)
        .next()
        .map_or(end, |(start, _)| *start);
    cut.checked_sub(start)
        .filter(|gate| *gate > 0)
        .map(BeatTime::from_raw)
}

fn cover(intervals: &mut BTreeMap<u64, u64>, mut start: u64, mut end: u64) {
    if let Some((&left, &right)) = intervals.range(..=start).next_back()
        && right >= start
    {
        start = left;
        end = end.max(right);
        intervals.remove(&left);
    }
    while let Some((&left, &right)) = intervals.range(start..=end).next() {
        end = end.max(right);
        intervals.remove(&left);
    }
    intervals.insert(start, end);
}

/// Apply an already validated source-sized plan without allocation or ID changes.
pub(crate) fn retain_gates(
    source: &mut MidiSource,
    gates: &[Option<BeatTime>],
) -> Result<(), Invalid> {
    if source.notes.len() != gates.len() {
        return Err(Invalid("midi overwrite source changed"));
    }
    for (note, gate) in source.notes.iter_mut().zip(gates) {
        note.duration = gate.unwrap_or(BeatTime::ZERO);
    }
    source.notes.retain(|note| note.duration != BeatTime::ZERO);
    Ok(())
}
