//! Shared atomic admission, compaction and append logic for MIDI batch edits.
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_NOTES, MidiNote, MidiNoteInsert, NoteId, Project,
    midi::{note_count, validate_note},
};

pub(crate) fn edit(
    project: &mut Project,
    clip_id: ClipId,
    remove: Vec<NoteId>,
    insert: Vec<MidiNoteInsert>,
) -> Result<Created, Invalid> {
    edit_bounded(project, clip_id, remove, insert, None)
}

pub(crate) fn edit_with_source_extent(
    project: &mut Project,
    clip_id: ClipId,
    remove: Vec<NoteId>,
    insert: Vec<MidiNoteInsert>,
) -> Result<Created, Invalid> {
    let target = project
        .midi_source(clip_id)
        .ok_or(Invalid("midi clip no longer exists"))?;
    let length = extended_source_length(target.source_length(), &insert)?;
    edit_bounded(project, clip_id, remove, insert, Some(length))
}

/// Range authoring retains silent destination time as well as note ends. The
/// caller has checked the complete extent before entering atomic batch admission.
/// Empty growth changes only extent; empty/no-growth remains a true no-op.
pub(crate) fn edit_range(
    project: &mut Project,
    clip_id: ClipId,
    insert: Vec<MidiNoteInsert>,
    length: BeatTime,
) -> Result<Created, Invalid> {
    let old = project
        .midi_source(clip_id)
        .ok_or(Invalid("midi clip no longer exists"))?;
    if length < old.source_length() {
        return Err(Invalid("midi range cannot shrink its source"));
    }
    if insert.is_empty() {
        if length != old.source_length() {
            project.midi_source_mut(clip_id)?.source_length = length;
        }
        return Ok(Created::None);
    }
    edit_bounded(project, clip_id, Vec::new(), insert, Some(length))
}

/// Checked extent calculation shared with the closed MIDI history proof.
pub(crate) fn extended_source_length(
    length: BeatTime,
    insert: &[MidiNoteInsert],
) -> Result<BeatTime, Invalid> {
    if insert.len() > MAX_NOTES {
        return Err(Invalid("midi note batch exceeds note limit"));
    }
    insert.iter().try_fold(length, |length, note| {
        let end = note
            .start
            .raw()
            .checked_add(note.duration.raw())
            .ok_or(Invalid("midi note end overflow"))?;
        Ok(BeatTime::from_raw(length.raw().max(end)))
    })
}

fn edit_bounded(
    project: &mut Project,
    clip_id: ClipId,
    remove: Vec<NoteId>,
    insert: Vec<MidiNoteInsert>,
    source_length: Option<BeatTime>,
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
    let length = source_length.unwrap_or_else(|| target.source_length());
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

    let plan = target.preview_note_overwrite(&remove, &insert)?;
    let authored_count = plan.authored.iter().filter(|gate| gate.is_some()).count();
    let final_clip_len = plan
        .retained
        .iter()
        .filter(|gate| gate.is_some())
        .count()
        .checked_add(authored_count)
        .ok_or(Invalid("midi clip note count overflow"))?;
    let final_count = note_count(project)
        .checked_sub(target.notes().len())
        .ok_or(Invalid("note count underflow"))?
        .checked_add(final_clip_len)
        .ok_or(Invalid("note count overflow"))?;
    if final_count > MAX_NOTES {
        return Err(Invalid("note limit reached"));
    }

    let additional_capacity = final_clip_len.saturating_sub(target.notes().len());
    let target = project.midi_source_mut(clip_id)?;
    target
        .notes
        .try_reserve(additional_capacity)
        .map_err(|_| Invalid("could not reserve midi note storage"))?;

    let id_range = if authored_count == 0 {
        None
    } else {
        Some(project.allocate_id_range(authored_count)?)
    };

    let target = project.midi_source_mut(clip_id)?;
    target.source_length = length;
    crate::midi_note_overwrite::retain_gates(target, &plan.retained)?;

    if let Some((first, last)) = id_range {
        let surviving = insert
            .into_iter()
            .zip(plan.authored)
            .filter_map(|(note, gate)| gate.map(|duration| MidiNoteInsert { duration, ..note }));
        for (id, note) in (first..=last).zip(surviving) {
            target.notes.push(MidiNote {
                id: NoteId(id),
                active: note.active,
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
