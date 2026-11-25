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
                expression: crate::NoteExpression::default(),
                expression_points: Vec::new(),
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

pub(crate) fn edit_expressive(
    project: &mut Project,
    clip_id: ClipId,
    remove: Vec<NoteId>,
    insert: Vec<crate::MidiExpressiveNoteInsert>,
) -> Result<Created, Invalid> {
    if remove.is_empty() && insert.is_empty() {
        return Err(Invalid("midi note edit batch must not be empty"));
    }
    let length = project
        .midi_source(clip_id)
        .ok_or(Invalid("midi clip no longer exists"))?
        .source_length();
    let mut plain = Vec::with_capacity(insert.len());
    for note in &insert {
        note.expression.validate()?;
        crate::midi::validate_note_expression_points(&note.expression_points, note.duration)?;
        plain.push(MidiNoteInsert {
            active: note.active,
            start: note.start,
            duration: note.duration,
            pitch: note.pitch,
            channel: note.channel,
            velocity: note.velocity,
        });
    }
    let target = project
        .midi_source(clip_id)
        .ok_or(Invalid("midi clip no longer exists"))?;
    let plan = target.preview_note_overwrite(&remove, &plain)?;
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
            .filter_map(|(mut note, gate)| {
                gate.map(|duration| {
                    note.duration = duration;
                    note.expression_points.retain(|p| p.offset <= duration);
                    note
                })
            });
        for (id, note) in (first..=last).zip(surviving) {
            target.notes.push(MidiNote {
                id: NoteId(id),
                active: note.active,
                start: note.start,
                duration: note.duration,
                pitch: note.pitch,
                channel: note.channel,
                velocity: note.velocity,
                expression: note.expression,
                expression_points: note.expression_points,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: u64, duration: u64) -> MidiNoteInsert {
        MidiNoteInsert {
            active: true,
            start: BeatTime::from_quarters(start).unwrap(),
            duration: BeatTime::from_quarters(duration).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        }
    }

    #[test]
    fn extended_source_length_grows_to_max_end_and_rejects_overflow() {
        let base = BeatTime::from_quarters(4).unwrap();
        assert_eq!(extended_source_length(base, &[note(1, 2)]).unwrap(), base);
        assert_eq!(
            extended_source_length(base, &[note(3, 4)]).unwrap(),
            BeatTime::from_quarters(7).unwrap()
        );
        let overflow = MidiNoteInsert {
            start: BeatTime::from_raw(u64::MAX),
            duration: BeatTime::from_raw(1),
            ..note(0, 1)
        };
        assert!(extended_source_length(base, &[overflow]).is_err());
    }

    #[test]
    fn expressive_batch_authoring_preserves_distinct_notes_trims_points_and_refuses_invalid() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(crate::Command::AddMidiTrack { name: "mpe".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(crate::Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let before = editor.project().snapshot();
        // Invalid pitch bend (> 4800) must refuse without mutating.
        assert!(
            editor
                .execute(crate::Command::AddMidiNotesWithExpression {
                    clip,
                    notes: vec![crate::MidiExpressiveNoteInsert {
                        active: true,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch: 60,
                        channel: 1,
                        velocity: 100,
                        expression: crate::NoteExpression {
                            pitch_offset_cents: 5000,
                            timbre: 0,
                            pressure: 0,
                        },
                        expression_points: Vec::new(),
                    }],
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);

        // Two overlapping notes on different pitches with distinct expression + same-pitch overwrite trimming points.
        editor
            .execute(crate::Command::AddMidiNotesWithExpression {
                clip,
                notes: vec![
                    crate::MidiExpressiveNoteInsert {
                        active: true,
                        start: BeatTime::ZERO,
                        duration: BeatTime::from_quarters(2).unwrap(),
                        pitch: 60,
                        channel: 1,
                        velocity: 100,
                        expression: crate::NoteExpression {
                            pitch_offset_cents: 200,
                            timbre: 90,
                            pressure: 80,
                        },
                        expression_points: vec![
                            crate::NoteExpressionPoint {
                                offset: BeatTime::ZERO,
                                pitch_offset_cents: 200,
                                timbre: 90,
                                pressure: 80,
                            },
                            crate::NoteExpressionPoint {
                                offset: BeatTime::from_ratio(3, 2).unwrap(),
                                pitch_offset_cents: 400,
                                timbre: 100,
                                pressure: 95,
                            },
                        ],
                    },
                    crate::MidiExpressiveNoteInsert {
                        active: true,
                        start: BeatTime::ZERO,
                        duration: BeatTime::from_quarters(2).unwrap(),
                        pitch: 64,
                        channel: 2,
                        velocity: 105,
                        expression: crate::NoteExpression {
                            pitch_offset_cents: -300,
                            timbre: 30,
                            pressure: 110,
                        },
                        expression_points: vec![crate::NoteExpressionPoint {
                            offset: BeatTime::ONE,
                            pitch_offset_cents: -350,
                            timbre: 25,
                            pressure: 115,
                        }],
                    },
                    // Overlaps pitch 60 starting at beat 1 -> trims first pitch 60 to duration 1 and drops its 1.5-beat point!
                    crate::MidiExpressiveNoteInsert {
                        active: true,
                        start: BeatTime::ONE,
                        duration: BeatTime::ONE,
                        pitch: 60,
                        channel: 3,
                        velocity: 110,
                        expression: crate::NoteExpression {
                            pitch_offset_cents: 50,
                            timbre: 60,
                            pressure: 70,
                        },
                        expression_points: Vec::new(),
                    },
                ],
            })
            .unwrap();
        let notes = editor.project().midi_clip(clip).unwrap().notes();
        assert_eq!(notes.len(), 3);
        let first_60 = notes
            .iter()
            .find(|n| n.pitch() == 60 && n.start() == BeatTime::ZERO)
            .unwrap();
        assert_eq!(first_60.duration(), BeatTime::ONE);
        assert_eq!(first_60.expression().pitch_offset_cents, 200);
        assert_eq!(first_60.expression_points().len(), 1);
        let note_64 = notes.iter().find(|n| n.pitch() == 64).unwrap();
        assert_eq!(note_64.expression().pitch_offset_cents, -300);
        assert_eq!(note_64.expression_points().len(), 1);
        let after = editor.project().snapshot();
        assert!(editor.undo());
        let mut expected_before = before;
        expected_before.next_id = after.next_id;
        assert_eq!(editor.project().snapshot(), expected_before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}
