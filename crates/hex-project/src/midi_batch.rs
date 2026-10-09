//! Shared atomic admission, compaction and append logic for MIDI batch edits.
use crate::{
    ClipId, Created, Invalid, MAX_NOTES, MidiNote, MidiNoteInsert, NoteId, Project,
    midi::{note_count, note_indices, validate_note},
};

pub(crate) fn edit(
    project: &mut Project,
    clip_id: ClipId,
    remove: Vec<NoteId>,
    insert: Vec<MidiNoteInsert>,
) -> Result<Created, Invalid> {
    if remove.is_empty() && insert.is_empty() {
        return Err(Invalid("midi note edit batch must not be empty"));
    }
    if remove.len() > MAX_NOTES || insert.len() > MAX_NOTES {
        return Err(Invalid("midi note batch exceeds note limit"));
    }

    let target = project
        .midi_source(clip_id)
        .ok_or(Invalid("midi clip no longer exists"))?;
    let length = target.source_length();
    let removal_indices = if remove.is_empty() {
        Vec::new()
    } else {
        note_indices(target, remove.iter().copied())?
    };
    for note in &insert {
        validate_note(
            note.start,
            note.duration,
            length,
            note.pitch,
            note.channel,
            note.velocity,
        )?;
    }

    let final_count = note_count(project)
        .checked_sub(remove.len())
        .ok_or(Invalid("note count underflow"))?
        .checked_add(insert.len())
        .ok_or(Invalid("note count overflow"))?;
    if final_count > MAX_NOTES {
        return Err(Invalid("note limit reached"));
    }

    let final_clip_len = target
        .notes()
        .len()
        .checked_sub(remove.len())
        .ok_or(Invalid("midi note removal count exceeds target clip"))?
        .checked_add(insert.len())
        .ok_or(Invalid("midi clip note count overflow"))?;
    let additional_capacity = final_clip_len.saturating_sub(target.notes().len());
    let target = project.midi_source_mut(clip_id)?;
    target
        .notes
        .try_reserve(additional_capacity)
        .map_err(|_| Invalid("could not reserve midi note storage"))?;

    let id_range = if insert.is_empty() {
        None
    } else {
        Some(project.allocate_id_range(insert.len())?)
    };

    let target = project.midi_source_mut(clip_id)?;
    if !removal_indices.is_empty() {
        let mut indices = removal_indices;
        indices.sort_unstable();
        let mut removed = indices.into_iter().peekable();
        let mut index = 0;
        target.notes.retain(|_| {
            let should_remove = removed.peek().copied() == Some(index);
            if should_remove {
                removed.next();
            }
            index += 1;
            !should_remove
        });
    }

    if let Some((first, last)) = id_range {
        for (id, note) in (first..=last).zip(insert) {
            target.notes.push(MidiNote {
                id: NoteId(id),
                start: note.start,
                duration: note.duration,
                pitch: note.pitch,
                channel: note.channel,
                velocity: note.velocity,
            });
        }
        Ok(Created::Notes {
            clip: clip_id,
            first: NoteId(first),
            last: NoteId(last),
        })
    } else {
        Ok(Created::None)
    }
}
