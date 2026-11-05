//! Authored activation and atomic time-span fragmentation. This owner changes
//! project metadata only; preparation/publication and audio voices stay outside.
//! MIDI fragments retain the source law through elapsed windows. Crossing gates
//! retrigger with their remaining duration; envelope/phase continuity is not promised.
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MidiClip, MidiSource, NoteId,
    Project, SessionCellContent, TimeRange,
};
use std::sync::Arc;

impl Project {
    /// Authored activation of an exact Arrangement or Session clip identity.
    /// Missing/retired identities have no fallback to their track or source.
    pub fn clip_active(&self, id: ClipId) -> Option<bool> {
        for track in self.tracks() {
            if let Some(clip) = track.clips().iter().find(|clip| clip.id() == id) {
                return Some(clip.active());
            }
            if let Some(clip) = track.midi_clips().iter().find(|clip| clip.id() == id) {
                return Some(clip.active());
            }
        }
        self.session_cells()
            .iter()
            .find_map(|cell| match cell.content() {
                SessionCellContent::Audio(clip) if clip.id() == id => Some(clip.active()),
                SessionCellContent::Midi(clip) if clip.id() == id => Some(clip.active()),
                _ => None,
            })
    }
    /// Query the same half-open selected spans as `SetTimeRangeActive`. Track
    /// membership/order and MIDI frame-to-beat cuts are validated canonically.
    /// None means no intersecting clips; Some(true) means at least one active
    /// clip; Some(false) means every intersecting clip is inactive. Selection
    /// does not change project state or enumerate repeated note occurrences.
    pub fn time_range_has_active_clips(&self, range: &TimeRange) -> Result<Option<bool>, Invalid> {
        let first = range.validate(self)?;
        let (left, right) = activation_beats(self.tempo_map(), range)?;
        let mut found = None;
        for track in &self.tracks()[first..first + range.tracks().len()] {
            for clip in track.clips() {
                let end = clip
                    .start()
                    .checked_add(clip.duration())
                    .ok_or(Invalid("clip end overflow"))?;
                if clip.start() < range.end() && range.start() < end {
                    found = Some(found.unwrap_or(false) || clip.active());
                }
            }
            for clip in track.midi_clips() {
                let end = clip
                    .start()
                    .raw()
                    .checked_add(clip.length().raw())
                    .ok_or(Invalid("midi clip end overflow"))?;
                if clip.start() < right && left.raw() < end {
                    found = Some(found.unwrap_or(false) || clip.active());
                }
            }
        }
        Ok(found)
    }
}

// Fragment edges are projected once from selected project frames. Unlike
// onset-cell overwrite bounds, these are exact editable placement boundaries.
fn activation_beats(
    tempo: &crate::TempoMap,
    range: &TimeRange,
) -> Result<(BeatTime, BeatTime), Invalid> {
    Ok((
        tempo.frame_to_beat(range.start())?,
        tempo.frame_to_beat(range.end())?,
    ))
}

pub(crate) fn set(project: &mut Project, id: ClipId, active: bool) -> Result<Created, Invalid> {
    for track in &mut project.tracks {
        if let Some(clip) = track
            .content
            .audio_clips
            .iter_mut()
            .find(|clip| clip.id == id)
        {
            clip.active = active;
            return Ok(Created::None);
        }
        if let Some(clip) = track
            .content
            .midi_clips
            .iter_mut()
            .find(|clip| clip.id == id)
        {
            clip.active = active;
            return Ok(Created::None);
        }
    }
    for cell in &mut project.session.cells {
        match &mut cell.content {
            SessionCellContent::Audio(clip) if clip.id == id => {
                clip.active = active;
                return Ok(Created::None);
            }
            SessionCellContent::Midi(clip) if clip.id == id => {
                clip.active = active;
                return Ok(Created::None);
            }
            _ => (),
        }
    }
    Err(Invalid("clip no longer exists"))
}
fn clone_midi(project: &mut Project, clip: &MidiClip) -> Result<MidiClip, Invalid> {
    let mut notes = Vec::new();
    notes
        .try_reserve_exact(clip.notes().len())
        .map_err(|_| Invalid("could not reserve activation fragment notes"))?;
    let mut copy = clip.clone();
    copy.id = ClipId(project.allocate_id()?);
    for note in clip.notes() {
        let mut note = note.clone();
        note.id = NoteId(project.allocate_id()?);
        notes.push(note);
    }
    let source = clip.source();
    copy.source = Arc::new(MidiSource {
        content: std::sync::Arc::new(crate::MidiContent {
            source_length: source.source_length,
            notes,
        }),
        one_shot_boundary: source.one_shot_boundary,
        source_start: source.source_start,
        loop_start: source.loop_start,
        loop_end: source.loop_end,
        looping: source.looping,
    });
    Ok(copy)
}
fn charge(clips: &mut usize, notes: &mut usize, more_notes: usize) -> Result<(), Invalid> {
    *clips = clips
        .checked_add(1)
        .filter(|count| *count <= MAX_CLIPS)
        .ok_or(Invalid("clip limit reached"))?;
    *notes = notes
        .checked_add(more_notes)
        .filter(|count| *count <= MAX_NOTES)
        .ok_or(Invalid("midi note limit reached"))?;
    Ok(())
}
pub(crate) fn set_range(
    project: &mut Project,
    range: &TimeRange,
    active: bool,
) -> Result<Created, Invalid> {
    let intersects = project.time_range_has_active_clips(range)?;
    if intersects.is_none() || intersects == Some(false) && !active {
        return Ok(Created::None);
    }
    let first = range.validate(project)?;
    let mut candidate = project.clone();
    let mut clips = candidate
        .tracks()
        .iter()
        .map(|track| track.clips().len() + track.midi_clips().len())
        .sum::<usize>()
        + candidate
            .session_cells()
            .iter()
            .filter(|cell| !matches!(cell.content(), SessionCellContent::Stop))
            .count();
    let mut notes = crate::midi::note_count(&candidate);
    let (left_beat, right_beat) = activation_beats(candidate.tempo_map(), range)?;
    for index in first..first + range.tracks().len() {
        let original = candidate.tracks[index].content.clone();
        let mut audio = Vec::new();
        let mut midi = Vec::new();
        audio
            .try_reserve_exact(original.audio_clips.len().saturating_mul(3).min(MAX_CLIPS))
            .map_err(|_| Invalid("could not reserve activation audio fragments"))?;
        midi.try_reserve_exact(original.midi_clips.len().saturating_mul(3).min(MAX_CLIPS))
            .map_err(|_| Invalid("could not reserve activation midi fragments"))?;
        for clip in original.audio_clips {
            let end = clip
                .start
                .checked_add(clip.duration)
                .ok_or(Invalid("clip end overflow"))?;
            let left = range.start().max(clip.start);
            let right = range.end().min(end);
            if left >= right || clip.active == active {
                audio.push(clip);
                continue;
            }
            let mut middle = clip;
            if left > middle.start {
                charge(&mut clips, &mut notes, 0)?;
                let id = ClipId(candidate.allocate_id()?);
                let (before, after) =
                    crate::edit::split_clip_at_tempo(&middle, left, id, candidate.tempo_map())?;
                audio.push(before);
                middle = after;
            }
            if right < end {
                charge(&mut clips, &mut notes, 0)?;
                let id = ClipId(candidate.allocate_id()?);
                let (mut selected, after) =
                    crate::edit::split_clip_at_tempo(&middle, right, id, candidate.tempo_map())?;
                selected.active = active;
                audio.push(selected);
                audio.push(after);
            } else {
                middle.active = active;
                audio.push(middle);
            }
        }
        for clip in original.midi_clips {
            let start = clip.start.raw();
            let end = start
                .checked_add(clip.length.raw())
                .ok_or(Invalid("midi clip end overflow"))?;
            let left = left_beat.raw().max(start);
            let right = right_beat.raw().min(end);
            if left >= right || clip.active == active {
                midi.push(clip);
                continue;
            }
            let boundaries = [start, left, right, end];
            let mut first_fragment = true;
            for pair in boundaries.windows(2) {
                let [from, to] = [pair[0], pair[1]];
                if from == to {
                    continue;
                }
                let mut fragment = if first_fragment {
                    first_fragment = false;
                    clip.clone()
                } else {
                    charge(&mut clips, &mut notes, clip.notes().len())?;
                    clone_midi(&mut candidate, &clip)?
                };
                fragment.start = BeatTime::from_raw(from);
                fragment.length = BeatTime::from_raw(to - from);
                fragment.playback_offset = BeatTime::from_raw(
                    clip.playback_offset
                        .raw()
                        .checked_add(from - start)
                        .ok_or(Invalid("midi playback window offset overflow"))?,
                );
                if from >= left && to <= right {
                    fragment.active = active;
                }
                fragment.source_playback()?;
                midi.push(fragment);
            }
        }
        candidate.tracks[index].content.audio_clips = audio;
        candidate.tracks[index].content.midi_clips = midi;
    }
    // Validate the complete candidate without replacing its monotonic ID cursor.
    Project::from_snapshot(candidate.snapshot())?;
    *project = candidate;
    Ok(Created::None)
}

#[cfg(test)]
mod tests;
