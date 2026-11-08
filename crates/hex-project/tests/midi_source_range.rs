use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, Invalid, MAX_NOTES, MidiNoteInsert,
    MidiNoteSnapshot, Project, TrackContentSnapshot,
};
fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn note(start: u64, duration: u64, pitch: u8, velocity: u8) -> MidiNoteInsert {
    MidiNoteInsert {
        active: true,
        start: beat(start),
        duration: beat(duration),
        pitch,
        channel: pitch % 16,
        velocity,
    }
}
fn fixture() -> (Editor, ClipId) {
    let mut p = Editor::new(Project::new(48_000).unwrap(), 12).unwrap();
    let Created::Track(track) = p
        .execute(Command::AddTrack {
            name: "range".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = p
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(3),
            length: beat(20),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    p.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(6),
        source_start: beat(1),
        loop_start: beat(2),
        loop_end: beat(4),
        looping: true,
        length: beat(20),
    })
    .unwrap();
    (p, clip)
}
fn command(clip: ClipId, start: u64, end: u64) -> Command {
    Command::DuplicateMidiSourceRange {
        clip,
        start: beat(start),
        end: beat(end),
    }
}
fn history_cycle(editor: &mut Editor, before: &Project) {
    let after = editor.project().clone();
    let effect = editor
        .next_midi_history_effect(false)
        .expect("closed canonical effect");
    assert!(effect.validates_forward(before, &after));
    editor
        .undo_prepared_transition::<_, Invalid>(|source, target| {
            assert!(effect.validates_forward(target, source));
            Ok(())
        })
        .unwrap()
        .unwrap();
    let mut normalized = before.snapshot();
    normalized.next_id = after.next_identity();
    assert_eq!(editor.project().snapshot(), normalized);
    editor
        .redo_prepared_transition::<_, Invalid>(|source, target| {
            assert!(effect.validates_forward(source, target));
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(editor.project(), &after);
    assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
}

#[test]
fn duplicating_a_source_range_preserves_inactive_notes_and_history() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![note(0, 1, 60, 100)],
        })
        .unwrap();
    let id = editor.project().midi_source(clip).unwrap().notes()[0].id();
    editor
        .execute(Command::SetMidiNotesActive {
            clip,
            notes: vec![id],
            active: false,
        })
        .unwrap();
    let before = editor.project().clone();
    editor.execute(command(clip, 0, 2)).unwrap();
    let notes = editor.project().midi_source(clip).unwrap().notes();
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().all(|note| !note.active()));
    history_cycle(&mut editor, &before);
}
#[test]
fn partial_boundaries_fresh_stored_order_and_repeated_range_preserve_source_fields() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![note(3, 2, 64, 93), note(0, 2, 60, 37)],
        })
        .unwrap();
    let before = editor.project().clone();
    let originals = before.midi_source(clip).unwrap().notes().to_vec();
    let first_id = before.next_identity();
    let mut cmd = command(clip, 1, 4);
    assert!(cmd.supports_sampler_midi_edit());
    assert!(cmd.prepare_sampler_midi_edit(&before).unwrap());
    let (created, ()) = editor
        .execute_midi_prepared_transition::<_, Invalid>(cmd, |_, candidate| {
            assert_eq!(
                candidate.midi_source(clip).unwrap().source_length(),
                beat(7)
            );
            Ok(())
        })
        .unwrap();
    let Created::Notes {
        clip: target,
        first,
        last,
    } = created
    else {
        panic!("fresh range")
    };
    assert_eq!(
        (target, first.value(), last.value()),
        (clip, first_id, first_id + 1)
    );
    let source = editor.project().midi_source(clip).unwrap();
    assert_eq!(&source.notes()[..2], &originals);
    for (n, (start, duration, pitch, velocity)) in source.notes()[2..]
        .iter()
        .zip([(6, 1, 64, 93), (4, 1, 60, 37)])
    {
        assert_eq!(
            (
                n.start(),
                n.duration(),
                n.pitch(),
                n.channel(),
                n.velocity()
            ),
            (beat(start), beat(duration), pitch, pitch % 16, velocity)
        );
        assert!(!originals.iter().any(|old| old.id() == n.id()));
    }
    let c = editor.project().midi_clip(clip).unwrap();
    assert_eq!(
        (
            c.start(),
            c.arrangement_length(),
            c.source_start(),
            c.loop_start(),
            c.loop_end(),
            c.looping()
        ),
        (beat(3), beat(20), beat(1), beat(2), beat(4), true)
    );
    history_cycle(&mut editor, &before);
    let before_repeat = editor.project().clone();
    editor
        .execute_midi_prepared_transition::<_, Invalid>(command(clip, 4, 7), |_, _| Ok(()))
        .unwrap();
    let s = editor.project().midi_source(clip).unwrap();
    assert_eq!(s.source_length(), beat(10));
    assert!(
        s.notes()
            .iter()
            .any(|n| n.pitch() == 60 && n.start() == beat(7) && n.velocity() == 37)
    );
    assert!(
        s.notes()
            .iter()
            .any(|n| n.pitch() == 64 && n.start() == beat(9) && n.velocity() == 93)
    );
    history_cycle(&mut editor, &before_repeat);
}
#[test]
fn copied_intersection_uses_canonical_destructive_collision_and_closed_history() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![note(0, 6, 60, 93)],
        })
        .unwrap();
    let before = editor.project().clone();
    let old = before.midi_source(clip).unwrap().notes()[0].id();
    editor
        .execute_midi_prepared_transition::<_, Invalid>(command(clip, 2, 4), |_, _| Ok(()))
        .unwrap();
    let notes = editor.project().midi_source(clip).unwrap().notes();
    assert_eq!(notes.len(), 2);
    assert_eq!(
        (notes[0].id(), notes[0].start(), notes[0].duration()),
        (old, beat(0), beat(4))
    );
    assert_eq!(
        (notes[1].start(), notes[1].duration(), notes[1].velocity()),
        (beat(4), beat(2), 93)
    );
    assert_ne!(notes[1].id(), old);
    history_cycle(&mut editor, &before);
}
#[test]
fn silence_retains_trailing_extent_and_empty_noop_preserves_redo_and_identity() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![note(3, 1, 60, 37)],
        })
        .unwrap();
    let before = editor.project().clone();
    editor
        .execute_midi_prepared_transition::<_, Invalid>(command(clip, 3, 6), |_, _| Ok(()))
        .unwrap();
    let source = editor.project().midi_source(clip).unwrap();
    assert_eq!(
        source.source_length(),
        beat(9),
        "keep two beats of copied trailing silence"
    );
    assert_eq!(source.notes().last().unwrap().start(), beat(6));
    assert_eq!(source.notes().last().unwrap().duration(), beat(1));
    history_cycle(&mut editor, &before);
    let before_growth = editor.project().clone();
    let id = before_growth.next_identity();
    assert_eq!(
        editor
            .execute_midi_prepared_transition::<_, Invalid>(command(clip, 10, 12), |_, _| Ok(()))
            .unwrap()
            .0,
        Created::None
    );
    assert_eq!(
        editor.project().midi_source(clip).unwrap().source_length(),
        beat(14)
    );
    assert_eq!(editor.project().next_identity(), id);
    history_cycle(&mut editor, &before_growth);
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before_noop = editor.project().clone();
    let prior_effect = editor.next_midi_history_effect(false).unwrap();
    let mut noop = command(clip, 10, 12);
    assert!(!noop.prepare_sampler_midi_edit(editor.project()).unwrap());
    assert_eq!(
        editor
            .execute_midi_prepared_transition::<_, Invalid>(noop, |a, b| {
                assert_eq!(a, b);
                Ok(())
            })
            .unwrap()
            .0,
        Created::None
    );
    assert_eq!(editor.project(), &before_noop);
    assert!(prior_effect.validates_forward(&before_growth, editor.project()));
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().master_gain(), 0.5);
}
#[test]
fn console_range_owns_only_its_source_and_current_format_roundtrips() {
    let (mut editor, arrangement) = fixture();
    editor
        .execute(Command::AddMidiNotes {
            clip: arrangement,
            notes: vec![note(0, 2, 60, 37)],
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "one".into() })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: arrangement,
            scene,
        })
        .unwrap()
    else {
        panic!("cell")
    };
    let before = editor.project().clone();
    editor
        .execute_midi_prepared_transition::<_, Invalid>(command(clip, 1, 4), |_, _| Ok(()))
        .unwrap();
    assert_eq!(
        editor.project().midi_clip(arrangement),
        before.midi_clip(arrangement)
    );
    let source = editor.project().midi_source(clip).unwrap();
    assert_eq!(source.source_length(), beat(7));
    assert_eq!(
        (
            source.source_start(),
            source.loop_start(),
            source.loop_end(),
            source.looping()
        ),
        (beat(1), beat(2), beat(4), true)
    );
    assert_eq!(source.notes().last().unwrap().start(), beat(4));
    history_cycle(&mut editor, &before);
}
#[test]
fn malformed_overflow_failed_prepare_and_note_capacity_are_atomic() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![note(0, 2, 60, 37)],
        })
        .unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    for cmd in [
        command(clip, 2, 2),
        command(clip, 3, 2),
        Command::DuplicateMidiSourceRange {
            clip,
            start: BeatTime::ZERO,
            end: BeatTime::from_raw(u64::MAX),
        },
    ] {
        assert!(
            editor
                .execute_midi_prepared_transition::<(), Invalid>(cmd, |_, _| panic!(
                    "must refuse before preparation"
                ))
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }
    assert!(
        editor
            .execute_midi_prepared_transition::<(), Invalid>(command(clip, 1, 4), |a, b| {
                assert_ne!(a, b);
                Err(Invalid("prepared graph refused"))
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    let mut exhausted = before.snapshot();
    exhausted.next_id = u64::MAX;
    let mut exhausted = Editor::new(Project::from_snapshot(exhausted).unwrap(), 4).unwrap();
    let unchanged = exhausted.project().clone();
    assert!(exhausted.execute(command(clip, 1, 4)).is_err());
    assert_eq!(exhausted.project(), &unchanged);
    let mut snapshot = before.snapshot();
    let first = snapshot.next_id;
    let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content else {
        panic!("hybrid")
    };
    let source = &mut midi_clips[0];
    source.source_length_raw = beat(MAX_NOTES as u64 + 1).raw();
    source.notes = (0..MAX_NOTES)
        .map(|i| MidiNoteSnapshot {
            active: true,
            id: first + i as u64,
            start_raw: beat(i as u64).raw(),
            duration_raw: BeatTime::ONE.raw() / 4,
            pitch: 60,
            channel: 0,
            velocity: 37,
        })
        .collect();
    snapshot.next_id = first + MAX_NOTES as u64;
    let mut full = Editor::new(Project::from_snapshot(snapshot).unwrap(), 4).unwrap();
    let unchanged = full.project().clone();
    assert!(
        full.execute(Command::DuplicateMidiSourceRange {
            clip,
            start: BeatTime::ZERO,
            end: BeatTime::from_ratio(1, 2).unwrap()
        })
        .is_err()
    );
    assert_eq!(full.project(), &unchanged);
    let mut removed = Editor::new(before, 4).unwrap();
    removed.execute(Command::RemoveMidiClip(clip)).unwrap();
    let unchanged = removed.project().clone();
    assert!(removed.execute(command(clip, 1, 4)).is_err());
    assert_eq!(removed.project(), &unchanged);
}
