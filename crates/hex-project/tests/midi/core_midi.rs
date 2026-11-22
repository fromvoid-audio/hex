use hex_project::{
    BeatTime, Command, Created, Editor, MAX_NOTES, MidiNoteInsert, MidiNoteSnapshot,
    MidiNoteUpdate, Project, TrackContentSnapshot,
};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).expect("small beat")
}

#[test]
fn midi_overwrite_splits_victim_and_retriggers_held_note_in_finite_right_fragment() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 12).expect("editor");
    let add_track = |editor: &mut Editor, name: &str| match editor
        .execute(Command::AddTrack { name: name.into() })
        .expect("track")
    {
        Created::Track(track) => track,
        other => panic!("unexpected track: {other:?}"),
    };
    let target = add_track(&mut editor, "target");
    let source = add_track(&mut editor, "source");
    let victim = match editor
        .execute(Command::PlaceMidiClip {
            track: target,
            start: beat(0),
            length: beat(8),
        })
        .expect("victim")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip: {other:?}"),
    };
    let original_note = match editor
        .execute(Command::AddMidiNote {
            clip: victim,
            start: BeatTime::ZERO,
            duration: beat(8),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("held note")
    {
        Created::Note(note) => note,
        other => panic!("unexpected note: {other:?}"),
    };
    let incoming = match editor
        .execute(Command::PlaceMidiClip {
            track: source,
            start: beat(0),
            length: beat(2),
        })
        .expect("incoming")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip: {other:?}"),
    };
    let before = editor.project().snapshot();
    editor
        .execute(Command::MoveMidiClipWithOverwrite {
            clip: incoming,
            track: target,
            start: beat(2),
        })
        .expect("overwrite move");

    let target_track = editor.project().track(target).expect("target track");
    let victim_left = target_track
        .midi_clips()
        .iter()
        .find(|clip| clip.id() == victim)
        .expect("left keeps original clip id");
    assert_eq!(
        (victim_left.start(), victim_left.length()),
        (beat(0), beat(2))
    );
    assert_eq!(victim_left.notes()[0].id(), original_note);
    assert_eq!(victim_left.notes()[0].duration(), beat(8));
    let victim_right = target_track
        .midi_clips()
        .iter()
        .find(|clip| clip.id() != victim && clip.id() != incoming)
        .expect("right fragment");
    assert_eq!(
        (victim_right.start(), victim_right.length()),
        (beat(4), beat(4))
    );
    assert_eq!(victim_right.source_start(), BeatTime::ZERO);
    assert_eq!(victim_right.source_length(), beat(4));
    assert_eq!(victim_right.loop_start(), BeatTime::ZERO);
    assert_eq!(victim_right.loop_end(), beat(4));
    assert!(!victim_right.looping());
    assert_eq!(victim_right.notes().len(), 1);
    assert_eq!(victim_right.notes()[0].start(), BeatTime::ZERO);
    assert_eq!(victim_right.notes()[0].duration(), beat(4));
    assert_ne!(victim_right.notes()[0].id(), original_note);
    assert!(
        editor
            .project()
            .track(source)
            .expect("source")
            .midi_clips()
            .is_empty()
    );
    let committed = editor.project().snapshot();
    assert_eq!(
        Project::from_snapshot(committed.clone())
            .expect("round trip")
            .snapshot(),
        committed
    );

    assert!(editor.undo());
    let undone = editor.project().snapshot();
    let mut expected = before;
    expected.next_id = undone.next_id;
    assert_eq!(undone, expected);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), committed);
    let same_position = editor.project().snapshot();
    let can_undo = editor.can_undo();
    editor
        .execute(Command::MoveMidiClipWithOverwrite {
            clip: incoming,
            track: target,
            start: beat(2),
        })
        .expect("same-position drop");
    assert_eq!(editor.project().snapshot(), same_position);
    assert_eq!(editor.can_undo(), can_undo);
}

#[test]
fn midi_overwrite_rejects_unbounded_loop_expansion_without_mutating_project_or_history() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 12).expect("editor");
    let target = match editor
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .expect("target")
    {
        Created::Track(track) => track,
        other => panic!("unexpected track: {other:?}"),
    };
    let source = match editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .expect("source")
    {
        Created::Track(track) => track,
        other => panic!("unexpected track: {other:?}"),
    };
    let victim = match editor
        .execute(Command::PlaceMidiClip {
            track: target,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters((MAX_NOTES as u64) + 4).expect("long clip"),
        })
        .expect("long victim")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip: {other:?}"),
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip: victim,
            source_length: beat(1),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: beat(1),
            looping: true,
            length: BeatTime::from_quarters((MAX_NOTES as u64) + 4).expect("long clip"),
        })
        .expect("loop source");
    editor
        .execute(Command::AddMidiNote {
            clip: victim,
            start: BeatTime::ZERO,
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("looping note");
    let incoming = match editor
        .execute(Command::PlaceMidiClip {
            track: source,
            start: BeatTime::ZERO,
            length: beat(1),
        })
        .expect("incoming")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip: {other:?}"),
    };
    let before = editor.project().snapshot();
    let can_undo = editor.can_undo();
    assert!(
        editor
            .execute(Command::MoveMidiClipWithOverwrite {
                clip: incoming,
                track: target,
                start: beat(1),
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.can_undo(), can_undo);
    assert!(!editor.can_redo());
}
fn editor() -> (Editor, hex_project::TrackId, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 12).expect("editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "keys".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        _ => panic!("wrong result"),
    };
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(2),
            length: beat(4),
        })
        .expect("clip")
    {
        Created::Clip(id) => id,
        _ => panic!("wrong result"),
    };
    (editor, track, clip)
}

#[test]
fn one_hybrid_track_accepts_both_clip_types_and_cross_type_moves_undo() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let add_track = |editor: &mut Editor, name: &str| match editor
        .execute(Command::AddTrack { name: name.into() })
        .expect("track")
    {
        Created::Track(track) => track,
        other => panic!("unexpected track result: {other:?}"),
    };
    let first = add_track(&mut editor, "first");
    let second = add_track(&mut editor, "second");
    let asset = match editor
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .expect("asset")
    {
        Created::Asset(asset) => asset,
        other => panic!("unexpected asset result: {other:?}"),
    };
    let audio = match editor
        .execute(Command::PlaceClip {
            track: first,
            asset,
            start: 0,
            duration: 32,
            source_offset: 0,
            source_length: 32,
            looping: false,
        })
        .expect("audio clip")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip result: {other:?}"),
    };
    let midi = match editor
        .execute(Command::PlaceMidiClip {
            track: first,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .expect("midi clip on same track")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected midi result: {other:?}"),
    };
    let lane = editor.project().track(first).expect("hybrid lane");
    assert_eq!(
        lane.clips()
            .iter()
            .map(|clip| clip.id())
            .collect::<Vec<_>>(),
        [audio]
    );
    assert_eq!(
        lane.midi_clips()
            .iter()
            .map(|clip| clip.id())
            .collect::<Vec<_>>(),
        [midi]
    );
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).expect("roundtrip"),
        *editor.project()
    );

    editor
        .execute(Command::MoveClip {
            clip: audio,
            track: second,
            start: 64,
        })
        .expect("move audio across tracks");
    editor
        .execute(Command::MoveMidiClip {
            clip: midi,
            track: second,
            start: BeatTime::ONE,
        })
        .expect("move midi across tracks");
    assert!(editor.project().track(first).unwrap().clips().is_empty());
    assert!(
        editor
            .project()
            .track(first)
            .unwrap()
            .midi_clips()
            .is_empty()
    );
    assert_eq!(
        editor.project().track(second).unwrap().clips()[0].id(),
        audio
    );
    assert_eq!(
        editor.project().track(second).unwrap().midi_clips()[0].id(),
        midi
    );
    assert!(editor.undo(), "undo midi move");
    assert!(editor.undo(), "undo audio move");
    assert_eq!(
        editor.project().track(first).unwrap().clips()[0].id(),
        audio
    );
    assert_eq!(
        editor.project().track(first).unwrap().midi_clips()[0].id(),
        midi
    );
}

#[test]
fn moving_midi_clip_to_new_hybrid_track_preserves_clip_and_note_ids_in_one_step() {
    let (mut editor, source, clip) = editor();
    let anchor = match editor
        .execute(Command::AddTrack {
            name: "anchor".into(),
        })
        .expect("anchor")
    {
        Created::Track(track) => track,
        other => panic!("unexpected anchor: {other:?}"),
    };
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 64,
            channel: 2,
            velocity: 91,
        })
        .expect("note")
    else {
        panic!("note expected")
    };
    let before = editor.project().clone();
    let source_clip = before.midi_clip(clip).expect("source midi clip");
    let source_identity = source_clip.source_identity();

    let Created::Track(inserted) = editor
        .execute(Command::MoveMidiClipToNewTrack {
            initial_gain: None,
            clip,
            name: "track 3".into(),
            before_track: Some(anchor),
            start: beat(7),
        })
        .expect("move midi clip to new track")
    else {
        panic!("inserted track expected")
    };
    let after = editor.project().clone();
    assert_eq!(
        after
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>(),
        [source, inserted, anchor]
    );
    assert!(
        after
            .track(source)
            .expect("source remains")
            .midi_clips()
            .is_empty()
    );
    let inserted_track = after.track(inserted).expect("hybrid track");
    assert!(inserted_track.clips().is_empty());
    assert_eq!(inserted_track.midi_clips()[0].id(), clip);
    assert_eq!(inserted_track.midi_clips()[0].start(), beat(7));
    assert_eq!(
        inserted_track.midi_clips()[0].source_identity(),
        source_identity
    );
    assert_eq!(inserted_track.midi_clips()[0].notes()[0].id(), note);
    assert_eq!(
        Project::from_snapshot(after.snapshot()).expect("round trip"),
        after
    );
    assert!(editor.undo(), "insert+move is one history step");
    assert_eq!(editor.project().tracks(), before.tracks());
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
}

#[test]
fn midi_notes_validate_clip_local_ranges_allow_overlap_and_roundtrip() {
    let (mut editor, _, clip) = editor();
    let first = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(0),
            duration: beat(2),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("note")
    {
        Created::Note(id) => id,
        _ => panic!("wrong result"),
    };
    let second = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(2),
            pitch: 60,
            channel: 15,
            velocity: 1,
        })
        .expect("overlap")
    {
        Created::Note(id) => id,
        _ => panic!("wrong result"),
    };
    assert_ne!(first, second);
    let midi = editor.project().midi_clip(clip).expect("midi clip");
    assert_eq!(midi.notes().len(), 2);
    assert_eq!(midi.start(), beat(2));
    assert_eq!(midi.length(), beat(4));
    assert_eq!(midi.arrangement_length(), beat(4));
    assert_eq!(midi.source_length(), beat(4));
    assert_eq!(midi.source_start(), BeatTime::ZERO);
    assert_eq!(midi.loop_start(), BeatTime::ZERO);
    assert_eq!(midi.loop_end(), beat(4));
    assert!(!midi.looping());
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).expect("snapshot"),
        *editor.project()
    );
}

#[test]
fn source_settings_are_atomic_keep_hidden_notes_and_duplicate_exactly() {
    let (mut editor, track, clip) = editor();
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: beat(1),
            loop_start: beat(3),
            loop_end: beat(6),
            looping: true,
            length: beat(5),
        })
        .unwrap();
    let Created::Notes { first, .. } = editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_ratio(1, 2).unwrap(),
                    duration: beat(1),
                    pitch: 50,
                    channel: 1,
                    velocity: 70,
                },
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_ratio(3, 2).unwrap(),
                    duration: beat(1),
                    pitch: 55,
                    channel: 2,
                    velocity: 80,
                },
                MidiNoteInsert {
                    active: true,
                    start: beat(4),
                    duration: beat(2),
                    pitch: 60,
                    channel: 3,
                    velocity: 90,
                },
                MidiNoteInsert {
                    active: true,
                    start: beat(7),
                    duration: beat(1),
                    pitch: 65,
                    channel: 4,
                    velocity: 100,
                },
            ],
        })
        .unwrap()
    else {
        panic!("note range expected")
    };
    let before = editor.project().clone();
    let source = editor.project().midi_clip(clip).unwrap();
    assert_eq!(source.length(), beat(5));
    assert_eq!(source.source_length(), beat(8));
    assert_eq!(source.source_start(), beat(1));
    assert_eq!(source.loop_start(), beat(3));
    assert_eq!(source.loop_end(), beat(6));
    assert!(source.looping());
    assert_eq!(source.notes().len(), 4);
    assert_eq!(source.notes()[0].id(), first);
    assert_eq!(
        source.source_playback().unwrap().loop_range(),
        Some((beat(3), beat(6)))
    );
    assert_eq!(Project::from_snapshot(before.snapshot()).unwrap(), before);

    editor
        .execute(Command::MoveMidiClip {
            clip,
            track,
            start: beat(10),
        })
        .unwrap();
    let Created::Clip(duplicate) = editor.execute(Command::DuplicateMidiClip(clip)).unwrap() else {
        panic!("clip expected")
    };
    let duplicate_state = editor.project().midi_clip(duplicate).unwrap();
    let source_state = editor.project().midi_clip(clip).unwrap();
    assert_eq!(duplicate_state.start(), beat(15));
    assert_eq!(duplicate_state.length(), source_state.length());
    assert_eq!(
        duplicate_state.source_length(),
        source_state.source_length()
    );
    assert_eq!(duplicate_state.source_start(), source_state.source_start());
    assert_eq!(duplicate_state.loop_start(), source_state.loop_start());
    assert_eq!(duplicate_state.loop_end(), source_state.loop_end());
    assert_eq!(duplicate_state.looping(), source_state.looping());
    assert_ne!(
        duplicate_state.notes()[0].id(),
        source_state.notes()[0].id()
    );
    assert_ne!(
        duplicate_state.source_identity(),
        source_state.source_identity()
    );
}

#[test]
fn unchanged_sources_share_across_project_clones_and_note_edits_detach() {
    let (mut editor, track, clip) = editor();
    let original = editor.project().clone();
    let identity = editor.project().midi_clip(clip).unwrap().source_identity();

    editor
        .execute(Command::ToggleMute(track))
        .expect("unrelated track edit");
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().source_identity(),
        identity
    );
    assert_eq!(
        original.midi_clip(clip).unwrap().source_identity(),
        identity
    );

    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("note edit");
    assert_ne!(
        editor.project().midi_clip(clip).unwrap().source_identity(),
        identity
    );
    assert_eq!(original.midi_clip(clip).unwrap().notes().len(), 0);
}

#[test]
fn source_changes_reject_invalidated_notes_without_consuming_history_or_ids() {
    let (mut editor, _, clip) = editor();
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(3),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::AddMidiTrack {
            name: "redo branch".into(),
        })
        .unwrap();
    editor.undo();
    let before = editor.project().clone();
    let next_id = before.snapshot().next_id;
    assert!(editor.can_redo());
    assert_eq!(
        editor.execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(3),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: false,
            length: beat(4),
        }),
        Err(hex_project::Invalid(
            "midi source loop must be inside its source"
        ))
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.project().snapshot().next_id, next_id);
    assert!(editor.can_redo());

    let result = editor.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(3),
        source_start: BeatTime::ZERO,
        loop_start: BeatTime::ZERO,
        loop_end: beat(3),
        looping: false,
        length: beat(4),
    });
    assert!(result.is_err());
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.project().snapshot().next_id, next_id);
    assert!(editor.can_redo());
}

#[test]
fn disabled_loop_markers_are_retained_and_source_can_differ_from_arrangement() {
    let (mut editor, _, clip) = editor();
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: beat(1),
            loop_start: beat(2),
            loop_end: beat(7),
            looping: false,
            length: beat(3),
        })
        .unwrap();
    let value = editor.project().midi_clip(clip).unwrap();
    assert_eq!(value.length(), beat(3));
    assert_eq!(value.source_length(), beat(8));
    assert_eq!(value.loop_start(), beat(2));
    assert_eq!(value.loop_end(), beat(7));
    assert!(!value.looping());
    assert_eq!(value.source_playback().unwrap().loop_range(), None);
}

#[test]
fn batch_note_insert_preserves_input_order_expression_and_one_step_history() {
    let (mut editor, _, clip) = editor();
    let notes = vec![
        MidiNoteInsert {
            active: true,
            start: BeatTime::from_ratio(1, 3).unwrap(),
            duration: BeatTime::from_ratio(1, 8).unwrap(),
            pitch: 72,
            channel: 3,
            velocity: 91,
        },
        MidiNoteInsert {
            active: true,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 8).unwrap(),
            pitch: 72,
            channel: 3,
            velocity: 91,
        },
        MidiNoteInsert {
            active: true,
            start: beat(2),
            duration: BeatTime::from_ratio(1, 4).unwrap(),
            pitch: 40,
            channel: 15,
            velocity: 1,
        },
    ];
    let before = editor.project().clone();
    let Created::Notes {
        clip: created_clip,
        first,
        last,
    } = editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: notes.clone(),
        })
        .unwrap()
    else {
        panic!("note range expected")
    };
    assert_eq!(created_clip, clip);
    assert_eq!(last.value() - first.value(), 2);
    let inserted = editor.project().midi_clip(clip).unwrap().notes();
    assert_eq!(inserted.len(), notes.len());
    for (index, (actual, expected)) in inserted.iter().zip(&notes).enumerate() {
        assert_eq!(actual.id().value(), first.value() + index as u64);
        assert_eq!(actual.start(), expected.start);
        assert_eq!(actual.duration(), expected.duration);
        assert_eq!(actual.pitch(), expected.pitch);
        assert_eq!(actual.channel(), expected.channel);
        assert_eq!(actual.velocity(), expected.velocity);
    }
    let after = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project().tracks(), before.tracks());
    assert_eq!(editor.project().sample_rate(), before.sample_rate());
    assert_eq!(editor.project().snapshot().next_id, last.value() + 1);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);

    let Created::Note(next) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(3),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    else {
        panic!("note expected")
    };
    assert_eq!(next.value(), last.value() + 1);
}

#[test]
fn mixed_note_edit_compacts_then_appends_in_one_ordered_history_step() {
    let (mut editor, track, clip) = editor();
    let other_clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(8),
            length: beat(4),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip expected"),
    };
    let add = |editor: &mut Editor, clip, start, pitch, channel, velocity| match editor
        .execute(Command::AddMidiNote {
            clip,
            start,
            duration: beat(1),
            pitch,
            channel,
            velocity,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note expected"),
    };
    let first = add(&mut editor, clip, beat(0), 60, 0, 80);
    let removed = add(&mut editor, clip, beat(1), 61, 1, 81);
    let last = add(&mut editor, clip, beat(2), 62, 2, 82);
    let other = add(&mut editor, other_clip, beat(0), 70, 3, 90);
    let before = editor.project().clone();
    let inserts = vec![
        MidiNoteInsert {
            active: true,
            start: BeatTime::from_ratio(1, 3).unwrap(),
            duration: BeatTime::from_ratio(1, 8).unwrap(),
            pitch: 90,
            channel: 14,
            velocity: 1,
        },
        MidiNoteInsert {
            active: true,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 8).unwrap(),
            pitch: 90,
            channel: 14,
            velocity: 1,
        },
    ];
    let Created::Notes {
        clip: created_clip,
        first: first_inserted,
        last: last_inserted,
    } = editor
        .execute(Command::EditMidiNotes {
            clip,
            remove: vec![removed],
            insert: inserts.clone(),
        })
        .unwrap()
    else {
        panic!("created note range expected")
    };
    assert_eq!(created_clip, clip);
    assert_eq!(last_inserted.value() - first_inserted.value(), 1);
    let target_notes = editor.project().midi_clip(clip).unwrap().notes();
    assert_eq!(target_notes.len(), 4);
    assert_eq!(target_notes[0].id(), first);
    assert_eq!(target_notes[1].id(), last);
    for (offset, (actual, expected)) in target_notes[2..].iter().zip(&inserts).enumerate() {
        assert_eq!(actual.id().value(), first_inserted.value() + offset as u64);
        assert_eq!(actual.start(), expected.start);
        assert_eq!(actual.duration(), expected.duration);
        assert_eq!(actual.pitch(), expected.pitch);
        assert_eq!(actual.channel(), expected.channel);
        assert_eq!(actual.velocity(), expected.velocity);
    }
    assert_eq!(editor.project().midi_note(other).unwrap().pitch(), 70);
    let after = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project().tracks(), before.tracks());
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);

    let next_id = editor.project().snapshot().next_id;
    assert_eq!(
        editor
            .execute(Command::EditMidiNotes {
                clip,
                remove: vec![first],
                insert: Vec::new(),
            })
            .unwrap(),
        Created::None
    );
    assert_eq!(editor.project().snapshot().next_id, next_id);
    let Created::Notes {
        clip: inserted_clip,
        first,
        last,
    } = editor
        .execute(Command::EditMidiNotes {
            clip,
            remove: Vec::new(),
            insert: vec![MidiNoteInsert {
                active: true,
                start: beat(3),
                duration: BeatTime::from_raw(1),
                pitch: 50,
                channel: 0,
                velocity: 100,
            }],
        })
        .unwrap()
    else {
        panic!("created note range expected")
    };
    assert_eq!(inserted_clip, clip);
    assert_eq!(first.value(), next_id);
    assert_eq!(last, first);
}

#[test]
fn invalid_mixed_note_batches_preserve_project_allocator_and_redo() {
    let (mut editor, track, clip) = editor();
    let other_clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(8),
            length: beat(4),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip expected"),
    };
    let add = |editor: &mut Editor, clip, start| match editor
        .execute(Command::AddMidiNote {
            clip,
            start,
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note expected"),
    };
    let first = add(&mut editor, clip, beat(0));
    let second = add(&mut editor, clip, beat(1));
    let foreign = add(&mut editor, other_clip, beat(0));
    let stale = add(&mut editor, clip, beat(2));
    editor.execute(Command::RemoveMidiNote(stale)).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let next_id = before.snapshot().next_id;
    assert!(editor.can_redo());

    let valid = MidiNoteInsert {
        active: true,
        start: beat(2),
        duration: BeatTime::from_raw(1),
        pitch: 72,
        channel: 3,
        velocity: 90,
    };
    let invalid = vec![
        (Vec::new(), Vec::new()),
        (vec![first, first], vec![valid]),
        (vec![first, foreign], vec![valid]),
        (vec![stale], vec![valid]),
        (
            vec![first],
            vec![
                valid,
                MidiNoteInsert {
                    active: true,
                    duration: BeatTime::ZERO,
                    ..valid
                },
            ],
        ),
        (
            vec![first],
            vec![
                valid,
                MidiNoteInsert {
                    active: true,
                    pitch: 128,
                    ..valid
                },
            ],
        ),
        (
            vec![first],
            vec![
                valid,
                MidiNoteInsert {
                    active: true,
                    channel: 16,
                    ..valid
                },
            ],
        ),
        (
            vec![first],
            vec![
                valid,
                MidiNoteInsert {
                    active: true,
                    velocity: 0,
                    ..valid
                },
            ],
        ),
        (
            vec![first],
            vec![
                valid,
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_raw(u64::MAX),
                    duration: BeatTime::from_raw(1),
                    ..valid
                },
            ],
        ),
        (
            vec![first],
            vec![
                valid,
                MidiNoteInsert {
                    active: true,
                    start: beat(4),
                    duration: BeatTime::from_raw(1),
                    ..valid
                },
            ],
        ),
        (vec![first; MAX_NOTES + 1], Vec::new()),
        (Vec::new(), vec![valid; MAX_NOTES + 1]),
    ];
    for (remove, insert) in invalid {
        assert!(
            editor
                .execute(Command::EditMidiNotes {
                    clip,
                    remove,
                    insert,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert_eq!(editor.project().snapshot().next_id, next_id);
        assert!(editor.can_redo());
    }
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().notes()[1].id(),
        second
    );
}

#[test]
fn mixed_batch_checks_final_global_cap_and_id_range_before_removing() {
    let (mut base, _, clip) = editor();
    let Created::Note(first) = base
        .execute(Command::AddMidiNote {
            clip,
            start: beat(0),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    else {
        panic!("note expected")
    };
    let Created::Note(second) = base
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 61,
            channel: 1,
            velocity: 90,
        })
        .unwrap()
    else {
        panic!("note expected")
    };
    let exact_project = project_with_notes_in_other_clip(&base, MAX_NOTES - 2);
    let mut exact = Editor::new(exact_project, 4).unwrap();
    let inserts = vec![
        MidiNoteInsert {
            active: true,
            start: beat(2),
            duration: BeatTime::from_raw(1),
            pitch: 70,
            channel: 2,
            velocity: 80,
        },
        MidiNoteInsert {
            active: true,
            start: beat(3),
            duration: BeatTime::from_raw(1),
            pitch: 71,
            channel: 3,
            velocity: 81,
        },
    ];
    let Created::Notes { first: created, .. } = exact
        .execute(Command::EditMidiNotes {
            clip,
            remove: vec![first, second],
            insert: inserts.clone(),
        })
        .unwrap()
    else {
        panic!("created notes expected")
    };
    assert_eq!(
        exact
            .project()
            .tracks()
            .iter()
            .flat_map(|track| track.midi_clips())
            .map(|midi_clip| midi_clip.notes().len())
            .sum::<usize>(),
        MAX_NOTES
    );
    assert!(exact.project().midi_note(first).is_none());
    assert!(exact.project().midi_note(second).is_none());
    assert!(exact.project().midi_note(created).is_some());

    let full_project = exact.project().clone();
    let mut reject = Editor::new(full_project, 4).unwrap();
    let before = reject.project().clone();
    let next_id = before.snapshot().next_id;
    assert!(
        reject
            .execute(Command::EditMidiNotes {
                clip,
                remove: vec![created],
                insert: vec![
                    inserts[0],
                    inserts[1],
                    MidiNoteInsert {
                        active: true,
                        pitch: 72,
                        ..inserts[0]
                    }
                ],
            })
            .is_err()
    );
    assert_eq!(reject.project(), &before);
    assert_eq!(reject.project().snapshot().next_id, next_id);

    let mut exhausted_snapshot = base.project().snapshot();
    exhausted_snapshot.next_id = u64::MAX - 1;
    let exhausted = Project::from_snapshot(exhausted_snapshot).unwrap();
    let mut exhausted = Editor::new(exhausted, 4).unwrap();
    let before = exhausted.project().clone();
    let next_id = before.snapshot().next_id;
    assert!(
        exhausted
            .execute(Command::EditMidiNotes {
                clip,
                remove: vec![first],
                insert: vec![inserts[0], inserts[1]],
            })
            .is_err()
    );
    assert_eq!(exhausted.project(), &before);
    assert_eq!(exhausted.project().snapshot().next_id, next_id);
    assert!(exhausted.project().midi_note(first).is_some());
}

#[test]
fn invalid_batch_insertions_preserve_project_ids_and_redo() {
    let (mut editor, _, clip) = editor();
    let Created::Clip(missing_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: editor.project().tracks()[0].id(),
            start: beat(8),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("clip expected")
    };
    editor
        .execute(Command::RemoveMidiClip(missing_clip))
        .unwrap();
    let Created::Imported {
        clip: audio_clip, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("audio import expected")
    };
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let next_id = before.snapshot().next_id;
    let valid = MidiNoteInsert {
        active: true,
        start: beat(0),
        duration: beat(1),
        pitch: 60,
        channel: 0,
        velocity: 100,
    };
    let invalid = [
        Vec::new(),
        vec![
            valid,
            MidiNoteInsert {
                active: true,
                duration: BeatTime::ZERO,
                ..valid
            },
        ],
        vec![
            valid,
            MidiNoteInsert {
                active: true,
                pitch: 128,
                ..valid
            },
        ],
        vec![
            valid,
            MidiNoteInsert {
                active: true,
                channel: 16,
                ..valid
            },
        ],
        vec![
            valid,
            MidiNoteInsert {
                active: true,
                velocity: 0,
                ..valid
            },
        ],
        vec![
            valid,
            MidiNoteInsert {
                active: true,
                start: BeatTime::from_raw(u64::MAX),
                duration: BeatTime::from_raw(1),
                ..valid
            },
        ],
        vec![
            valid,
            MidiNoteInsert {
                active: true,
                start: beat(4),
                duration: BeatTime::from_raw(1),
                ..valid
            },
        ],
    ];
    for notes in invalid {
        assert!(
            editor
                .execute(Command::AddMidiNotes { clip, notes })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert_eq!(editor.project().snapshot().next_id, next_id);
        assert!(editor.can_redo());
    }
    assert!(
        editor
            .execute(Command::AddMidiNotes {
                clip: audio_clip,
                notes: vec![MidiNoteInsert {
                    active: true,
                    start: beat(0),
                    duration: beat(1),
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.project().snapshot().next_id, next_id);
    assert!(editor.can_redo());
    assert!(
        editor
            .execute(Command::AddMidiNotes {
                clip: missing_clip,
                notes: vec![valid],
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.project().snapshot().next_id, next_id);
    assert!(editor.can_redo());
}

#[test]
fn batch_insert_rejects_size_project_cap_and_allocator_exhaustion_atomically() {
    let (editor, _, clip) = editor();
    let note = MidiNoteInsert {
        active: true,
        start: beat(0),
        duration: BeatTime::from_raw(1),
        pitch: 60,
        channel: 0,
        velocity: 100,
    };
    let mut oversized = Editor::new(editor.project().clone(), 4).unwrap();
    let before = oversized.project().clone();
    assert!(
        oversized
            .execute(Command::AddMidiNotes {
                clip,
                notes: vec![note; MAX_NOTES + 1],
            })
            .is_err()
    );
    assert_eq!(oversized.project(), &before);

    let mut exact =
        Editor::new(project_with_notes_in_other_clip(&editor, MAX_NOTES - 2), 4).unwrap();
    let Created::Notes { first, last, .. } = exact
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![
                note,
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_raw(1),
                    ..note
                },
            ],
        })
        .unwrap()
    else {
        panic!("created note range expected")
    };
    assert_eq!(last.value() - first.value(), 1);
    assert_eq!(
        exact
            .project()
            .tracks()
            .iter()
            .flat_map(|track| track.midi_clips())
            .map(|midi_clip| midi_clip.notes().len())
            .sum::<usize>(),
        MAX_NOTES
    );

    let mut full = Editor::new(project_with_notes_in_other_clip(&editor, MAX_NOTES), 4).unwrap();
    let before = full.project().clone();
    assert!(
        full.execute(Command::AddMidiNotes {
            clip,
            notes: vec![note],
        })
        .is_err()
    );
    assert_eq!(full.project(), &before);

    let mut exhausted_snapshot = editor.project().snapshot();
    exhausted_snapshot.next_id = u64::MAX - 1;
    let exhausted = Project::from_snapshot(exhausted_snapshot).unwrap();
    let mut exhausted = Editor::new(exhausted, 4).unwrap();
    let before = exhausted.project().clone();
    assert!(
        exhausted
            .execute(Command::AddMidiNotes {
                clip,
                notes: vec![
                    note,
                    MidiNoteInsert {
                        active: true,
                        start: BeatTime::from_raw(1),
                        ..note
                    }
                ],
            })
            .is_err()
    );
    assert_eq!(exhausted.project(), &before);
}

fn project_with_notes_in_other_clip(editor: &Editor, count: usize) -> Project {
    let mut snapshot = editor.project().snapshot();
    let other_track_id = snapshot.next_id;
    let other_clip_id = other_track_id + 1;
    let first_note_id = other_track_id + 10;
    snapshot.tracks.push(hex_project::TrackSnapshot {
        group_output_mutes: Vec::new(),
        tags: Vec::new(),
        kind: hex_project::TrackKind::Ordinary,
        parent_group: None,
        color: hex_project::ElementColor::default(),
        gain_envelope: Vec::new(),
        gain_touch_spans: Vec::new(),
        gain_touch_epoch: 0,
        id: other_track_id,
        name: "other instrument".into(),
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        instrument_position: None,
        content: TrackContentSnapshot::Instrument(vec![hex_project::MidiClipSnapshot {
            content_view: None,
            active: true,
            playback_offset_raw: 0,
            one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
            volume: hex_project::MidiClipVolume::default(),
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            id: other_clip_id,
            start_raw: beat(8).raw(),
            length_raw: beat(4).raw(),
            source_length_raw: beat(4).raw(),
            source_start_raw: 0,
            loop_start_raw: 0,
            loop_end_raw: beat(4).raw(),
            looping: false,
            notes: (0..count)
                .map(|index| MidiNoteSnapshot {
                    active: true,
                    id: first_note_id + index as u64,
                    start_raw: index as u64,
                    duration_raw: 1,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                })
                .collect(),
        }]),
        sends: Vec::new(),
    });
    snapshot.next_id = first_note_id + count as u64;
    Project::from_snapshot(snapshot).unwrap()
}

#[test]
fn invalid_midi_note_edits_are_atomic_and_keep_redo() {
    let (mut editor, instrument, clip) = editor();
    let note = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(0),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("note")
    {
        Created::Note(id) => id,
        _ => panic!("wrong result"),
    };
    editor
        .execute(Command::UpdateMidiNote {
            note,
            start: beat(0),
            duration: beat(1),
            pitch: 61,
            channel: 0,
            velocity: 100,
        })
        .expect("valid edit before rejected edits");
    let before = editor.project().clone();
    let undo = editor.can_undo();
    for command in [
        Command::UpdateMidiNote {
            note,
            start: beat(4),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
        Command::UpdateMidiNote {
            note,
            start: beat(0),
            duration: beat(1),
            pitch: 128,
            channel: 0,
            velocity: 100,
        },
        Command::UpdateMidiNote {
            note,
            start: beat(0),
            duration: beat(1),
            pitch: 60,
            channel: 16,
            velocity: 100,
        },
        Command::UpdateMidiNote {
            note,
            start: beat(0),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 0,
        },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project(), &before);
        assert_eq!(editor.can_undo(), undo);
    }
    assert_eq!(
        editor
            .project()
            .track(instrument)
            .expect("instrument")
            .midi_clips()
            .len(),
        1
    );
    editor.undo();
    assert!(editor.can_redo());
    assert!(
        editor
            .execute(Command::UpdateMidiNote {
                note,
                start: beat(0),
                duration: beat(1),
                pitch: 62,
                channel: 0,
                velocity: 100
            })
            .is_ok()
    );
    assert!(!editor.can_redo());
}

#[test]
fn duplicate_midi_clip_assigns_fresh_clip_and_every_note_id_across_history_branch() {
    let (mut editor, _, clip) = editor();
    let note = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 64,
            channel: 2,
            velocity: 90,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("wrong result"),
    };
    let duplicate = match editor.execute(Command::DuplicateMidiClip(clip)).unwrap() {
        Created::Clip(id) => id,
        _ => panic!("wrong result"),
    };
    let copy = editor.project().midi_clip(duplicate).unwrap();
    assert_eq!(copy.start(), beat(6));
    assert_ne!(copy.notes()[0].id(), note);
    assert_eq!(copy.notes()[0].pitch(), 64);
    let copied_note = copy.notes()[0].id();
    assert!(editor.undo());
    assert!(editor.project().midi_clip(duplicate).is_none());
    assert!(editor.redo());
    assert_eq!(
        editor.project().midi_clip(duplicate).unwrap().notes()[0].id(),
        copied_note
    );
    assert!(editor.undo());
    let next = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(0),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("wrong result"),
    };
    assert!(next.value() > copied_note.value());
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().notes()[0].id(),
        note
    );
    assert_eq!(editor.project().midi_clip(clip).unwrap().notes().len(), 2);
}

#[test]
fn batch_update_and_remove_are_single_order_preserving_history_steps() {
    let (mut editor, _, clip) = editor();
    let first = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(0),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    {
        Created::Note(note) => note,
        _ => panic!("note expected"),
    };
    let second = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 62,
            channel: 1,
            velocity: 90,
        })
        .unwrap()
    {
        Created::Note(note) => note,
        _ => panic!("note expected"),
    };
    let untouched = match editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(2),
            duration: beat(1),
            pitch: 64,
            channel: 2,
            velocity: 80,
        })
        .unwrap()
    {
        Created::Note(note) => note,
        _ => panic!("note expected"),
    };
    let before = editor.project().clone();
    editor
        .execute(Command::UpdateMidiNotes {
            clip,
            notes: vec![
                MidiNoteUpdate {
                    note: second,
                    start: beat(3),
                    duration: beat(1),
                    pitch: 70,
                    channel: 3,
                    velocity: 71,
                },
                MidiNoteUpdate {
                    note: first,
                    start: beat(1),
                    duration: beat(2),
                    pitch: 68,
                    channel: 4,
                    velocity: 72,
                },
            ],
        })
        .unwrap();
    let after_update = editor.project().clone();
    let updated = after_update.midi_clip(clip).unwrap().notes();
    assert_eq!(
        updated.iter().map(|note| note.id()).collect::<Vec<_>>(),
        [first, second, untouched]
    );
    assert_eq!(updated[0].pitch(), 68);
    assert_eq!(updated[0].channel(), 4);
    assert_eq!(updated[0].velocity(), 72);
    assert_eq!(updated[1].pitch(), 70);
    assert_eq!(updated[2].pitch(), 64);
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after_update);

    editor
        .execute(Command::RemoveMidiNotes {
            clip,
            notes: vec![untouched, first],
        })
        .unwrap();
    let after_remove = editor.project().clone();
    assert_eq!(
        after_remove
            .midi_clip(clip)
            .unwrap()
            .notes()
            .iter()
            .map(|note| note.id())
            .collect::<Vec<_>>(),
        [second]
    );
    assert!(editor.undo());
    assert_eq!(editor.project(), &after_update);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after_remove);
}

#[test]
fn batch_midi_edits_validate_every_target_before_mutation_and_preserve_redo() {
    let (mut editor, _, clip) = editor();
    let add_note = |editor: &mut Editor, clip, start| match editor
        .execute(Command::AddMidiNote {
            clip,
            start,
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    {
        Created::Note(note) => note,
        _ => panic!("note expected"),
    };
    let first = add_note(&mut editor, clip, beat(0));
    let second = add_note(&mut editor, clip, beat(1));
    let other_clip = match editor
        .execute(Command::PlaceMidiClip {
            track: editor.project().tracks()[0].id(),
            start: beat(8),
            length: beat(4),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip expected"),
    };
    let foreign = add_note(&mut editor, other_clip, beat(0));

    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());
    let before = editor.project().clone();
    let invalid_batches = [
        Command::UpdateMidiNotes {
            clip,
            notes: Vec::new(),
        },
        Command::UpdateMidiNotes {
            clip,
            notes: vec![
                MidiNoteUpdate {
                    note: first,
                    start: beat(2),
                    duration: beat(1),
                    pitch: 61,
                    channel: 0,
                    velocity: 90,
                },
                MidiNoteUpdate {
                    note: second,
                    start: beat(2),
                    duration: BeatTime::ZERO,
                    pitch: 61,
                    channel: 0,
                    velocity: 90,
                },
            ],
        },
        Command::UpdateMidiNotes {
            clip,
            notes: vec![
                MidiNoteUpdate {
                    note: first,
                    start: beat(2),
                    duration: beat(1),
                    pitch: 61,
                    channel: 0,
                    velocity: 90,
                },
                MidiNoteUpdate {
                    note: second,
                    start: BeatTime::from_raw(u64::MAX),
                    duration: BeatTime::from_raw(1),
                    pitch: 61,
                    channel: 0,
                    velocity: 90,
                },
            ],
        },
        Command::UpdateMidiNotes {
            clip,
            notes: vec![
                MidiNoteUpdate {
                    note: first,
                    start: beat(2),
                    duration: beat(1),
                    pitch: 61,
                    channel: 0,
                    velocity: 90,
                },
                MidiNoteUpdate {
                    note: foreign,
                    start: beat(2),
                    duration: beat(1),
                    pitch: 61,
                    channel: 0,
                    velocity: 90,
                },
            ],
        },
        Command::UpdateMidiNotes {
            clip,
            notes: vec![
                MidiNoteUpdate {
                    note: first,
                    start: beat(2),
                    duration: beat(1),
                    pitch: 61,
                    channel: 0,
                    velocity: 90,
                },
                MidiNoteUpdate {
                    note: first,
                    start: beat(3),
                    duration: beat(1),
                    pitch: 62,
                    channel: 0,
                    velocity: 90,
                },
            ],
        },
        Command::RemoveMidiNotes {
            clip,
            notes: Vec::new(),
        },
        Command::RemoveMidiNotes {
            clip,
            notes: vec![first, first],
        },
        Command::RemoveMidiNotes {
            clip,
            notes: vec![first, foreign],
        },
        Command::UpdateMidiNotes {
            clip,
            notes: vec![
                MidiNoteUpdate {
                    note: first,
                    start: beat(2),
                    duration: beat(1),
                    pitch: 61,
                    channel: 0,
                    velocity: 90,
                };
                MAX_NOTES + 1
            ],
        },
    ];
    for command in invalid_batches {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }
}

#[test]
fn snapshot_rejects_bad_global_ids_and_note_cap() {
    let (editor, _, _) = editor();
    let mut duplicate_id = editor.project().snapshot();
    let track_id = duplicate_id.tracks[0].id;
    let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut duplicate_id.tracks[0].content
    else {
        panic!("instrument snapshot")
    };
    clips[0].notes.push(MidiNoteSnapshot {
        active: true,
        id: track_id,
        start_raw: 0,
        duration_raw: 1,
        pitch: 60,
        channel: 0,
        velocity: 1,
    });
    assert!(Project::from_snapshot(duplicate_id).is_err());

    let mut over_limit = editor.project().snapshot();
    let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut over_limit.tracks[0].content
    else {
        panic!("instrument snapshot")
    };
    clips[0].notes = (0..=MAX_NOTES)
        .map(|id| MidiNoteSnapshot {
            active: true,
            id: 100 + id as u64,
            start_raw: 0,
            duration_raw: 1,
            pitch: 60,
            channel: 0,
            velocity: 1,
        })
        .collect();
    over_limit.next_id = 100 + MAX_NOTES as u64 + 1;
    assert!(Project::from_snapshot(over_limit).is_err());
}

#[test]
fn duplicate_refuses_overflow_of_the_duplicated_timeline_interval_atomically() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "edge".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_raw(u64::MAX - 2),
            length: BeatTime::from_raw(2),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip"),
    };
    let before = editor.project().clone();
    let can_undo = editor.can_undo();
    assert!(editor.execute(Command::DuplicateMidiClip(clip)).is_err());
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.can_undo(), can_undo);
}
