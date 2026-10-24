//! Authored activation and atomic time-span fragmentation. This owner changes
//! project metadata only; preparation/publication and audio voices stay outside.
//! MIDI fragments retain the source law through elapsed windows. Crossing gates
//! retrigger with their remaining duration; envelope/phase continuity is not promised.
use crate::{BeatTime, ClipId, Created, Invalid, MidiClip, MidiSource, NoteId, Project, SessionCellContent, TimeRange, MAX_CLIPS, MAX_NOTES};
use std::sync::Arc;

pub(crate) fn set(project: &mut Project, id: ClipId, active: bool) -> Result<Created, Invalid> {
    for track in &mut project.tracks {
        if let Some(clip) = track.content.audio_clips.iter_mut().find(|clip| clip.id == id) {
            clip.active = active; return Ok(Created::None);
        }
        if let Some(clip) = track.content.midi_clips.iter_mut().find(|clip| clip.id == id) {
            clip.active = active; return Ok(Created::None);
        }
    }
    for cell in &mut project.session.cells {
        match &mut cell.content {
            SessionCellContent::Audio(clip) if clip.id == id => { clip.active = active; return Ok(Created::None); }
            SessionCellContent::Midi(clip) if clip.id == id => { clip.active = active; return Ok(Created::None); }
            _ => (),
        }
    }
    Err(Invalid("clip no longer exists"))
}
fn clone_midi(project: &mut Project, clip: &MidiClip) -> Result<MidiClip, Invalid> {
    let mut notes = Vec::new();
    notes.try_reserve_exact(clip.notes().len()).map_err(|_| Invalid("could not reserve activation fragment notes"))?;
    let mut copy = clip.clone();
    copy.id = ClipId(project.allocate_id()?);
    for note in clip.notes() {
        let mut note = note.clone();
        note.id = NoteId(project.allocate_id()?);
        notes.push(note);
    }
    let source = clip.source();
    copy.source = Arc::new(MidiSource {
        one_shot_boundary: source.one_shot_boundary,
        source_length: source.source_length,
        source_start: source.source_start,
        loop_start: source.loop_start,
        loop_end: source.loop_end,
        looping: source.looping,
        notes,
    });
    Ok(copy)
}
fn charge(clips: &mut usize, notes: &mut usize, more_notes: usize) -> Result<(), Invalid> {
    *clips = clips.checked_add(1).filter(|count| *count <= MAX_CLIPS).ok_or(Invalid("clip limit reached"))?;
    *notes = notes.checked_add(more_notes).filter(|count| *count <= MAX_NOTES).ok_or(Invalid("midi note limit reached"))?;
    Ok(())
}
pub(crate) fn set_range(project: &mut Project, range: &TimeRange, active: bool) -> Result<Created, Invalid> {
    let first = range.validate(project)?;
    let mut candidate = project.clone();
    let mut clips = candidate.tracks().iter().map(|track| track.clips().len() + track.midi_clips().len()).sum::<usize>()
        + candidate.session_cells().iter().filter(|cell| !matches!(cell.content(), SessionCellContent::Stop)).count();
    let mut notes = crate::midi::note_count(&candidate);
    let (left_beat, right_beat) = crate::midi::midi_overwrite_frame_bounds(candidate.tempo_map(), range.start(), range.end())?;
    for index in first..first + range.tracks().len() {
        let original = candidate.tracks[index].content.clone();
        let mut audio = Vec::new();
        let mut midi = Vec::new();
        audio.try_reserve_exact(original.audio_clips.len().saturating_mul(3).min(MAX_CLIPS)).map_err(|_| Invalid("could not reserve activation audio fragments"))?;
        midi.try_reserve_exact(original.midi_clips.len().saturating_mul(3).min(MAX_CLIPS)).map_err(|_| Invalid("could not reserve activation midi fragments"))?;
        for clip in original.audio_clips {
            let end = clip.start.checked_add(clip.duration).ok_or(Invalid("clip end overflow"))?;
            let left = range.start().max(clip.start);
            let right = range.end().min(end);
            if left >= right || clip.active == active { audio.push(clip); continue; }
            let mut middle = clip;
            if left > middle.start {
                charge(&mut clips, &mut notes, 0)?;
                let id = ClipId(candidate.allocate_id()?);
                let (before, after) = crate::edit::split_clip_at_tempo(&middle, left, id, candidate.tempo_map())?;
                audio.push(before); middle = after;
            }
            if right < end {
                charge(&mut clips, &mut notes, 0)?;
                let id = ClipId(candidate.allocate_id()?);
                let (mut selected, after) = crate::edit::split_clip_at_tempo(&middle, right, id, candidate.tempo_map())?;
                selected.active = active; audio.push(selected); audio.push(after);
            } else { middle.active = active; audio.push(middle); }
        }
        for clip in original.midi_clips {
            let start = clip.start.raw();
            let end = start.checked_add(clip.length.raw()).ok_or(Invalid("midi clip end overflow"))?;
            let left = left_beat.raw().max(start);
            let right = right_beat.raw().min(end);
            if left >= right || clip.active == active { midi.push(clip); continue; }
            let boundaries = [start, left, right, end];
            let mut first_fragment = true;
            for pair in boundaries.windows(2) {
                let [from, to] = [pair[0], pair[1]];
                if from == to { continue; }
                let mut fragment = if first_fragment {
                    first_fragment = false; clip.clone()
                } else {
                    charge(&mut clips, &mut notes, clip.notes().len())?;
                    clone_midi(&mut candidate, &clip)?
                };
                fragment.start = BeatTime::from_raw(from);
                fragment.length = BeatTime::from_raw(to - from);
                fragment.playback_offset = BeatTime::from_raw(clip.playback_offset.raw().checked_add(from - start).ok_or(Invalid("midi playback window offset overflow"))?);
                if from >= left && to <= right { fragment.active = active; }
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
