use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, Invalid, MidiNoteInsert, MidiNoteUpdate, Project,
};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

fn note(start: u64, duration: u64, pitch: u8, channel: u8) -> MidiNoteInsert {
    MidiNoteInsert {
        active: true,
        start: beat(start),
        duration: beat(duration),
        pitch,
        channel,
        velocity: 91,
    }
}

fn fixture(console: bool) -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "notes".into(),
        })
        .unwrap()
    else {
        panic!("track identity")
    };
    let clip = if console {
        let Created::Scene(scene) = editor
            .execute(Command::AddScene { name: "one".into() })
            .unwrap()
        else {
            panic!("scene identity")
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "notes".into(),
                source_length: beat(20),
            })
            .unwrap()
        else {
            panic!("console clip identity")
        };
        clip
    } else {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: beat(20),
            })
            .unwrap()
        else {
            panic!("arrangement clip identity")
        };
        clip
    };
    editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![note(0, 12, 60, 3), note(1, 10, 64, 3)],
        })
        .unwrap();
    (editor, clip)
}

fn check_edit(editor: &mut Editor, clip: ClipId, command: Command) {
    let before = editor.project().midi_source(clip).unwrap().clone();
    let old_id = before.notes()[0].id();
    let other = before.notes()[1].clone();
    editor
        .execute_midi_prepared_transition::<_, Invalid>(command, |_, candidate| {
            let source = candidate.midi_source(clip).unwrap();
            let old = source.notes().iter().find(|n| n.id() == old_id).unwrap();
            assert_eq!(old.start(), BeatTime::ZERO);
            assert_eq!(old.duration(), beat(4), "discard the entire old right tail");
            assert_eq!(
                source.notes().iter().find(|n| n.id() == other.id()),
                Some(&other)
            );
            let same_pitch: Vec<_> = source.notes().iter().filter(|n| n.pitch() == 60).collect();
            assert_eq!(same_pitch.len(), 2);
            assert!(
                same_pitch
                    .iter()
                    .any(|n| n.start() == beat(4) && n.duration() == beat(2))
            );
            Ok(())
        })
        .unwrap();
    let after = editor.project().midi_source(clip).unwrap().clone();
    let effect = editor.next_midi_history_effect(false).unwrap();
    editor
        .undo_prepared_transition::<_, Invalid>(|source, target| {
            assert!(effect.validates_forward(target, source));
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(editor.project().midi_source(clip), Some(&before));
    editor
        .redo_prepared_transition::<_, Invalid>(|source, target| {
            assert!(effect.validates_forward(source, target));
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(editor.project().midi_source(clip), Some(&after));
}

#[test]
fn creation_commands_overwrite_same_pitch_across_channels_in_both_clip_contexts() {
    for console in [false, true] {
        for path in 0..4 {
            let (mut editor, clip) = fixture(console);
            let inserted = note(4, 2, 60, 7);
            let command = match path {
                0 => Command::AddMidiNote {
                    clip,
                    start: inserted.start,
                    duration: inserted.duration,
                    pitch: inserted.pitch,
                    channel: inserted.channel,
                    velocity: inserted.velocity,
                },
                1 => Command::AddMidiNotes {
                    clip,
                    notes: vec![inserted],
                },
                2 => Command::EditMidiNotes {
                    clip,
                    remove: vec![],
                    insert: vec![inserted],
                },
                _ => Command::EditMidiNotesWithSourceExtent {
                    clip,
                    remove: vec![],
                    insert: vec![inserted],
                },
            };
            check_edit(&mut editor, clip, command);
        }
    }
}

#[test]
fn single_and_group_updates_overwrite_without_breaking_closed_history() {
    for console in [false, true] {
        for group in [false, true] {
            let (mut editor, clip) = fixture(console);
            let inserted = note(14, 2, 60, 7);
            let Created::Note(id) = editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: inserted.start,
                    duration: inserted.duration,
                    pitch: inserted.pitch,
                    channel: inserted.channel,
                    velocity: inserted.velocity,
                })
                .unwrap()
            else {
                panic!("note identity")
            };
            let update = MidiNoteUpdate {
                note: id,
                start: beat(4),
                duration: beat(2),
                pitch: 60,
                channel: 7,
                velocity: 91,
            };
            let command = if group {
                Command::UpdateMidiNotes {
                    clip,
                    notes: vec![update],
                }
            } else {
                Command::UpdateMidiNote {
                    note: update.note,
                    start: update.start,
                    duration: update.duration,
                    pitch: update.pitch,
                    channel: update.channel,
                    velocity: update.velocity,
                }
            };
            check_edit(&mut editor, clip, command);
        }
    }
}

#[test]
fn interval_query_matches_independent_destructive_sequence_oracle() {
    use hex_project::MidiNoteOverwriteChange;
    let (editor, clip) = fixture(false);
    let source = editor.project().midi_source(clip).unwrap();
    // Origin is an existing-vector index, followed by authored-vector indices.
    // This deliberately uses the simple forward O(n*m) definition, independent
    // of the production reverse interval-union algorithm.
    for a in 0..9 {
        for a_length in 1..4 {
            for b in 0..9 {
                for b_length in 1..4 {
                    for pitch in [60, 64] {
                        let input = [note(a, a_length, 60, 7), note(b, b_length, pitch, 1)];
                        let mut expected: Vec<_> = source
                            .notes()
                            .iter()
                            .enumerate()
                            .map(|(index, n)| {
                                (index, n.start().raw(), n.duration().raw(), n.pitch())
                            })
                            .collect();
                        for (index, new) in input.iter().enumerate() {
                            let start = new.start.raw();
                            let end = start + new.duration.raw();
                            expected.retain_mut(|(_, old_start, old_gate, old_pitch)| {
                                if *old_pitch != new.pitch
                                    || *old_start >= end
                                    || *old_start + *old_gate <= start
                                {
                                    return true;
                                }
                                if *old_start < start {
                                    *old_gate = start - *old_start;
                                    true
                                } else {
                                    false
                                }
                            });
                            expected.push((
                                source.notes().len() + index,
                                start,
                                new.duration.raw(),
                                new.pitch,
                            ));
                        }
                        let plan = source.preview_note_overwrite(&[], &input).unwrap();
                        for (index, old) in source.notes().iter().enumerate() {
                            let actual = match plan.existing_change(old.id()) {
                                None => Some(old.duration()),
                                Some(MidiNoteOverwriteChange::Remove) => None,
                                Some(MidiNoteOverwriteChange::TruncateTo(gate)) => Some(gate),
                            };
                            let expected = expected
                                .iter()
                                .find(|n| n.0 == index)
                                .map(|n| BeatTime::from_raw(n.2));
                            assert_eq!(actual, expected, "{input:?} existing {index}");
                        }
                        for index in 0..input.len() {
                            let expected = expected
                                .iter()
                                .find(|n| n.0 == source.notes().len() + index)
                                .map(|n| BeatTime::from_raw(n.2));
                            assert_eq!(
                                plan.authored_duration(index),
                                expected,
                                "{input:?} author {index}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn erased_batch_authors_never_resurrect_old_right_tails_or_consume_ids() {
    let (mut editor, clip) = fixture(false);
    let before = editor.project().midi_source(clip).unwrap().clone();
    let next_id = editor.project().snapshot().next_id;
    let Created::Notes { first, last, .. } = editor
        .execute_midi_prepared_transition::<_, Invalid>(
            Command::AddMidiNotes {
                clip,
                notes: vec![note(0, 16, 60, 7), note(0, 2, 60, 1)],
            },
            |_, _| Ok(()),
        )
        .unwrap()
        .0
    else {
        panic!("authored identities")
    };
    assert_eq!(first, last);
    assert_eq!(first.value(), next_id);
    let source = editor.project().midi_source(clip).unwrap();
    assert_eq!(source.notes().len(), 2);
    assert_eq!(source.notes()[0], before.notes()[1]);
    assert_eq!(source.notes()[1].duration(), beat(2));
    assert_eq!(source.notes()[1].channel(), 1);
    let effect = editor.next_midi_history_effect(false).unwrap();
    let after = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project().midi_source(clip), Some(&before));
    assert!(effect.validates_forward(editor.project(), &after));
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
}

#[test]
fn all_selected_originals_are_excluded_before_group_authors_retire_members() {
    let (mut editor, clip) = fixture(true);
    editor
        .execute(Command::RemoveMidiNotes {
            clip,
            notes: editor
                .project()
                .midi_source(clip)
                .unwrap()
                .notes()
                .iter()
                .map(|n| n.id())
                .collect(),
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![note(0, 2, 60, 1), note(4, 2, 60, 2), note(8, 8, 60, 3)],
        })
        .unwrap();
    let before = editor.project().midi_source(clip).unwrap().clone();
    let updates = before.notes()[..2]
        .iter()
        .map(|old| MidiNoteUpdate {
            note: old.id(),
            start: beat(10),
            duration: beat(if old.channel() == 1 { 2 } else { 1 }),
            pitch: old.pitch(),
            channel: old.channel(),
            velocity: old.velocity(),
        })
        .collect();
    editor
        .execute_midi_prepared_transition::<_, Invalid>(
            Command::UpdateMidiNotes {
                clip,
                notes: updates,
            },
            |_, _| Ok(()),
        )
        .unwrap();
    let after = editor.project().midi_source(clip).unwrap();
    assert_eq!(after.notes().len(), 2);
    assert_eq!(after.notes()[0].id(), before.notes()[1].id());
    assert_eq!(after.notes()[0].start(), beat(10));
    assert_eq!(after.notes()[0].duration(), beat(1));
    assert_eq!(after.notes()[1].id(), before.notes()[2].id());
    assert_eq!(after.notes()[1].start(), beat(8));
    assert_eq!(after.notes()[1].duration(), beat(2));
    let effect = editor.next_midi_history_effect(false).unwrap();
    let committed = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project().midi_source(clip), Some(&before));
    assert!(effect.validates_forward(editor.project(), &committed));
    assert!(editor.redo());
}

#[test]
fn preparation_refusal_preserves_project_ids_and_redo_after_candidate_clipping() {
    let (mut editor, clip) = fixture(false);
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let mut prepared = false;
    let result = editor.execute_midi_prepared_transition::<_, Invalid>(
        Command::AddMidiNotes {
            clip,
            notes: vec![note(4, 2, 60, 7)],
        },
        |_, candidate| {
            prepared = true;
            assert_eq!(
                candidate.midi_source(clip).unwrap().notes()[0].duration(),
                beat(4)
            );
            Err::<(), _>(Invalid("actual preparation refused"))
        },
    );
    assert!(prepared);
    assert_eq!(result, Err(Invalid("actual preparation refused")));
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
}

#[test]
fn current_format_overlaps_refuse_and_exact_adjacency_roundtrips_without_rewrite() {
    use hex_project::{SessionCellContentSnapshot, TrackContentSnapshot};
    for console in [false, true] {
        let (editor, clip) = fixture(console);
        let mut snapshot = editor.project().snapshot();
        let notes = if console {
            let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content
            else {
                panic!("midi cell")
            };
            &mut clip.source.notes
        } else {
            let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content
            else {
                panic!("hybrid track")
            };
            &mut midi_clips[0].notes
        };
        notes[1].pitch = 60;
        notes[1].channel = 7;
        let overlapping = snapshot.clone();
        assert_eq!(
            Project::from_snapshot(snapshot),
            Err(Invalid("midi notes on the same pitch must not overlap"))
        );
        let mut adjacent = overlapping;
        let notes = if console {
            let SessionCellContentSnapshot::Midi(clip) = &mut adjacent.session.cells[0].content
            else {
                panic!("midi cell")
            };
            &mut clip.source.notes
        } else {
            let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut adjacent.tracks[0].content
            else {
                panic!("hybrid track")
            };
            &mut midi_clips[0].notes
        };
        notes[1].start_raw = beat(12).raw();
        notes[1].duration_raw = beat(2).raw();
        let loaded = Project::from_snapshot(adjacent.clone()).unwrap();
        assert_eq!(loaded.snapshot(), adjacent);
        assert_eq!(loaded.midi_source(clip).unwrap().notes().len(), 2);
    }
}

#[test]
fn overwrite_at_full_note_capacity_charges_only_the_actual_final_set() {
    use hex_project::{MAX_NOTES, MidiNoteSnapshot, TrackContentSnapshot};
    let (editor, clip) = fixture(false);
    let mut snapshot = editor.project().snapshot();
    let first = snapshot.next_id;
    let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content else {
        panic!("hybrid")
    };
    midi_clips[0].notes = (0..MAX_NOTES)
        .map(|index| MidiNoteSnapshot {
            active: true,
            id: first + index as u64,
            start_raw: index as u64,
            duration_raw: 1,
            pitch: 60,
            channel: 3,
            velocity: 91,
        })
        .collect();
    snapshot.next_id = first + MAX_NOTES as u64;
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 4).unwrap();
    let before = editor.project().midi_source(clip).unwrap().clone();
    let expected_id = editor.project().snapshot().next_id;
    let (Created::Note(created), ()) = editor
        .execute_midi_prepared_transition::<_, Invalid>(
            Command::AddMidiNote {
                clip,
                start: BeatTime::from_raw(1),
                duration: BeatTime::from_raw(1),
                pitch: 60,
                channel: 7,
                velocity: 103,
            },
            |_, _| Ok(()),
        )
        .unwrap()
    else {
        panic!("replacement identity")
    };
    let after = editor.project().midi_source(clip).unwrap();
    assert_eq!(after.notes().len(), MAX_NOTES);
    assert_eq!(created.value(), expected_id);
    assert!(
        after
            .notes()
            .iter()
            .all(|note| note.id() != before.notes()[1].id())
    );
    assert_eq!(after.notes()[0], before.notes()[0]);
    assert_eq!(after.notes()[1..MAX_NOTES - 1], before.notes()[2..]);
    assert!(editor.undo());
    assert_eq!(editor.project().midi_source(clip), Some(&before));
    assert!(editor.redo());
}

#[test]
fn fixed_receipts_reject_wrong_single_prefix_and_extra_batch_retirement() {
    use hex_project::TrackContentSnapshot;
    for batch in [false, true] {
        let (mut editor, clip) = fixture(false);
        let before = editor.project().clone();
        let mut input = vec![note(4, 2, 60, 7)];
        if batch {
            input.push(note(8, 1, 70, 1));
        }
        editor
            .execute_midi_prepared_transition::<_, Invalid>(
                Command::AddMidiNotes { clip, notes: input },
                |_, _| Ok(()),
            )
            .unwrap();
        let effect = editor.next_midi_history_effect(false).unwrap();
        let mut forged = editor.project().snapshot();
        let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut forged.tracks[0].content else {
            panic!("hybrid")
        };
        if batch {
            midi_clips[0].notes.remove(1);
        } else {
            midi_clips[0].notes[0].duration_raw = beat(2).raw();
        }
        let forged = Project::from_snapshot(forged).unwrap();
        assert!(!effect.validates_forward(&before, &forged));
    }
}

#[test]
fn splitting_long_looped_gates_preserves_existing_bounded_occurrence_semantics() {
    let (mut editor, clip) = fixture(false);
    let ids = editor
        .project()
        .midi_source(clip)
        .unwrap()
        .notes()
        .iter()
        .map(|n| n.id())
        .collect();
    editor
        .execute(Command::RemoveMidiNotes { clip, notes: ids })
        .unwrap();
    editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![note(0, 2, 60, 3)],
        })
        .unwrap();
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(20),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::ONE,
            looping: true,
            length: beat(4),
        })
        .unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let mut prepared = false;
    let result = editor.execute_validated::<Invalid>(
        Command::SplitMidiClip {
            clip,
            beat: beat(2),
        },
        |_| {
            prepared = true;
            Ok(())
        },
    );
    let Created::Clip(right) = result.unwrap() else {
        panic!("split identity")
    };
    assert!(prepared);
    assert_eq!(
        editor.project().midi_source(clip).unwrap(),
        before.midi_source(clip).unwrap()
    );
    // The existing playback occurrence owner caps at each loop boundary;
    // splitting materializes two adjacent gates, not overlapping authored notes.
    let gates: Vec<_> = editor
        .project()
        .midi_source(right)
        .unwrap()
        .notes()
        .iter()
        .map(|note| (note.start(), note.duration(), note.channel()))
        .collect();
    assert_eq!(
        gates,
        vec![
            (BeatTime::ZERO, BeatTime::ONE, 3),
            (BeatTime::ONE, BeatTime::ONE, 3)
        ]
    );
    let after = editor.project().clone();
    assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
    assert!(editor.undo());
    assert_eq!(editor.project().tracks(), before.tracks());
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
}
