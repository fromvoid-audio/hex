use hex_project::{BeatTime, ClipId, Command, Created, Editor, MidiOneShotBoundary, Project};

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture() -> (Editor, ClipId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(1),
            length: beat(12),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    for (start, duration, pitch) in [(4, 5, 60), (6, 1, 61), (10, 2, 62)] {
        e.execute(Command::AddMidiNote {
            clip,
            start: beat(start),
            duration: beat(duration),
            pitch,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    }
    (e, clip)
}
fn source(clip: ClipId, looping: bool) -> Command {
    Command::SetMidiClipSource {
        clip,
        source_length: beat(12),
        source_start: beat(2),
        loop_start: beat(2),
        loop_end: beat(6),
        looping,
        length: beat(20),
    }
}
fn capture(e: &mut Editor, clip: ClipId) {
    e.execute(Command::SetMidiClipSourceWithOverwrite {
        clip,
        source_length: beat(12),
        source_start: beat(2),
        loop_start: beat(2),
        loop_end: beat(6),
        looping: true,
        length: beat(20),
    })
    .unwrap();
}

#[test]
fn captured_once_keeps_all_notes_and_numeric_toggle_undo_restore_exact_policy() {
    let (mut e, clip) = fixture();
    let ordinary = e.project().snapshot();
    capture(&mut e, clip);
    let captured = e.project().snapshot();
    let notes = e.project().midi_clip(clip).unwrap().notes().to_vec();
    e.execute(source(clip, false)).unwrap();
    let c = e.project().midi_clip(clip).unwrap();
    assert_eq!(c.source().one_shot_boundary(), MidiOneShotBoundary::LoopEnd);
    assert_eq!(c.source().effective_end(), beat(6));
    assert_eq!(c.notes(), notes);
    assert_eq!(c.source_length(), beat(12));
    let p = c.source_playback().unwrap();
    let kept: Vec<_> = p
        .note_occurrences(beat(4), beat(5))
        .unwrap()
        .iter()
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].timeline_start(), beat(2));
    assert_eq!(kept[0].gate_duration(), beat(2));
    assert_eq!(p.note_occurrences(beat(6), beat(1)).unwrap().count(), 0);
    assert_eq!(p.note_occurrences(beat(10), beat(2)).unwrap().count(), 0);
    let once = e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(once.clone()).unwrap().snapshot(),
        once
    );
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), captured);
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), ordinary);
    assert!(e.redo());
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), once);
}

#[test]
fn ordinary_numeric_loop_off_retains_source_end_and_invalid_bounded_edit_is_atomic() {
    let (mut e, clip) = fixture();
    e.execute(source(clip, true)).unwrap();
    e.execute(source(clip, false)).unwrap();
    let c = e.project().midi_clip(clip).unwrap();
    assert_eq!(
        c.source().one_shot_boundary(),
        MidiOneShotBoundary::SourceEnd
    );
    assert_eq!(
        c.source_playback()
            .unwrap()
            .note_occurrences(beat(10), beat(2))
            .unwrap()
            .count(),
        1
    );
    capture(&mut e, clip);
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(12),
            source_start: beat(6),
            loop_start: beat(2),
            loop_end: beat(13),
            looping: false,
            length: beat(20)
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(e.undo());
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn copied_session_source_preserves_once_policy_and_cow_numeric_edit_is_independent() {
    let (mut e, clip) = fixture();
    capture(&mut e, clip);
    e.execute(source(clip, false)).unwrap();
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(copy) = e
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    let copied = e.project().session_midi_clip(copy).unwrap().source();
    assert_eq!(copied.one_shot_boundary(), MidiOneShotBoundary::LoopEnd);
    assert_eq!(
        copied
            .source_playback(beat(20))
            .unwrap()
            .note_occurrences(beat(10), beat(2))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(copied.notes().len(), 3);
    assert_ne!(
        copied.notes()[0].id(),
        e.project().midi_clip(clip).unwrap().notes()[0].id()
    );
    let before = e.project().snapshot();
    e.execute(Command::SetSessionMidiClipSource {
        clip: copy,
        source_length: beat(12),
        source_start: beat(2),
        loop_start: beat(2),
        loop_end: beat(5),
        looping: false,
    })
    .unwrap();
    assert_eq!(
        e.project()
            .session_midi_clip(copy)
            .unwrap()
            .source()
            .effective_end(),
        beat(5)
    );
    assert_eq!(
        e.project()
            .midi_clip(clip)
            .unwrap()
            .source()
            .effective_end(),
        beat(6)
    );
    assert_eq!(
        e.project()
            .session_midi_clip(copy)
            .unwrap()
            .source()
            .notes()
            .len(),
        3
    );
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
    let mut invalid = before;
    let hex_project::SessionCellContentSnapshot::Midi(cell) = &mut invalid.session.cells[0].content
    else {
        panic!("cell")
    };
    cell.source.loop_end_raw = cell.source.source_length_raw + 1;
    assert!(Project::from_snapshot(invalid).is_err());
}

#[test]
fn session_numeric_consumed_cursor_retains_full_source_and_atomic_history() {
    let (mut e, clip) = fixture();
    capture(&mut e, clip);
    e.execute(source(clip, false)).unwrap();
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(copy) = e
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    let original = e.project().snapshot();
    let notes = e
        .project()
        .session_midi_clip(copy)
        .unwrap()
        .source()
        .notes()
        .to_vec();
    for cursor in [beat(6), beat(20), BeatTime::from_raw(u64::MAX)] {
        let before = e.project().snapshot();
        e.execute(Command::SetSessionMidiClipSource {
            clip: copy,
            source_length: beat(12),
            source_start: cursor,
            loop_start: beat(2),
            loop_end: beat(6),
            looping: false,
        })
        .unwrap();
        let retained = e.project().session_midi_clip(copy).unwrap().source();
        assert_eq!(retained.source_start(), cursor);
        assert_eq!(retained.notes(), notes);
        let mapping = retained.source_playback(beat(20)).unwrap();
        assert!(!mapping.has_initial_pass());
        assert_eq!(mapping.initial_remaining(), BeatTime::ZERO);
        for note in retained.notes() {
            assert_eq!(
                mapping
                    .note_occurrences(note.start(), note.duration())
                    .unwrap()
                    .count(),
                0
            );
        }
        // Session COW editing does not change the Arrangement owner's cursor or notes.
        assert_eq!(e.project().midi_clip(clip).unwrap().source_start(), beat(2));
        let after = e.project().snapshot();
        assert_eq!(
            Project::from_snapshot(after.clone()).unwrap().snapshot(),
            after
        );
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), after);
    }
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::SetSessionMidiClipSource {
            clip: copy,
            source_length: beat(12),
            source_start: beat(20),
            loop_start: beat(2),
            loop_end: beat(6),
            looping: true,
        })
        .is_err(),
        "ordinary command must not normalize an invalid Repeat cursor"
    );
    assert_eq!(e.project().snapshot(), before);
    assert_eq!(e.project().midi_clip(clip).unwrap().source_start(), beat(2));
    assert_ne!(before, original);
}

#[test]
fn silence_tail_bounded_once_accepts_exhausted_cursor_without_revealing_notes() {
    for cursor in [beat(6), beat(20), BeatTime::from_raw(u64::MAX)] {
        let (mut e, clip) = fixture();
        capture(&mut e, clip);
        e.execute(source(clip, false)).unwrap();
        let before = e.project().snapshot();
        let notes = e.project().midi_clip(clip).unwrap().notes().to_vec();
        e.execute(Command::SetMidiClipSource {
            clip, source_length: beat(12), source_start: cursor,
            loop_start: beat(2), loop_end: beat(6), looping: false, length: beat(20),
        }).expect("a captured Once source can consume its complete cycle and retain silent outer duration");
        let c = e.project().midi_clip(clip).unwrap();
        assert_eq!(c.source_start(), cursor);
        assert_eq!(c.notes(), notes);
        let playback = c.source_playback().unwrap();
        for note in c.notes() {
            assert_eq!(
                playback
                    .note_occurrences(note.start(), note.duration())
                    .unwrap()
                    .count(),
                0
            );
        }
        let after = e.project().snapshot();
        assert_eq!(
            Project::from_snapshot(after.clone()).unwrap().snapshot(),
            after
        );
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), after);
    }
}

#[test]
fn silence_tail_left_trim_advances_unwrapped_cursor_and_extend_back_recovers_source() {
    let (mut e, clip) = fixture();
    capture(&mut e, clip);
    e.execute(source(clip, false)).unwrap();
    let original = e.project().snapshot();
    let notes = e.project().midi_clip(clip).unwrap().notes().to_vec();
    let preview = e
        .project()
        .midi_clip(clip)
        .unwrap()
        .preview_edge_trim(hex_project::TrimEdge::Left, beat(7))
        .unwrap();
    assert_eq!(
        preview.start(),
        beat(7),
        "valid trim inside the silent outer tail must not clamp to the playback end"
    );
    e.execute(Command::TrimMidiClipEdge {
        clip,
        edge: hex_project::TrimEdge::Left,
        beat: beat(7),
    })
    .unwrap();
    let c = e.project().midi_clip(clip).unwrap();
    assert_eq!(c.start(), beat(7));
    assert_eq!(c.source_start(), beat(8));
    assert_eq!(c.length(), beat(14));
    assert_eq!(c.notes(), notes);
    assert_eq!(
        i128::from(c.start().raw()) - i128::from(c.source_start().raw()),
        -(1_i128 << 32)
    );
    for note in c.notes() {
        assert_eq!(
            c.source_playback()
                .unwrap()
                .note_occurrences(note.start(), note.duration())
                .unwrap()
                .count(),
            0
        );
    }
    e.execute(Command::TrimMidiClipEdge {
        clip,
        edge: hex_project::TrimEdge::Left,
        beat: beat(1),
    })
    .unwrap();
    assert_eq!(e.project().snapshot(), original);
}

#[test]
fn silence_tail_split_keeps_full_source_with_fresh_note_ids_and_one_history_step() {
    let (mut e, clip) = fixture();
    capture(&mut e, clip);
    e.execute(source(clip, false)).unwrap();
    let before = e.project().snapshot();
    let original = e.project().midi_clip(clip).unwrap().clone();
    let Created::Clip(right) = e
        .execute(Command::SplitMidiClip {
            clip,
            beat: beat(7),
        })
        .unwrap()
    else {
        panic!("right clip")
    };
    let left = e.project().midi_clip(clip).unwrap();
    let right_clip = e.project().midi_clip(right).unwrap();
    assert_eq!(left.notes(), original.notes());
    assert_eq!(
        right_clip.source().one_shot_boundary(),
        MidiOneShotBoundary::LoopEnd
    );
    assert_eq!(right_clip.source_length(), original.source_length());
    assert_eq!(
        (right_clip.loop_start(), right_clip.loop_end()),
        (original.loop_start(), original.loop_end())
    );
    assert_eq!(right_clip.source_start(), beat(8));
    assert_eq!(
        (right_clip.start(), right_clip.length()),
        (beat(7), beat(14))
    );
    assert_eq!(
        right_clip.notes().len(),
        original.notes().len(),
        "silent right fragment retains hidden authored notes"
    );
    for (old, new) in original.notes().iter().zip(right_clip.notes()) {
        assert_ne!(old.id(), new.id());
        assert_eq!(
            (
                old.start(),
                old.duration(),
                old.pitch(),
                old.channel(),
                old.velocity()
            ),
            (
                new.start(),
                new.duration(),
                new.pitch(),
                new.channel(),
                new.velocity()
            )
        );
        assert_eq!(
            right_clip
                .source_playback()
                .unwrap()
                .note_occurrences(new.start(), new.duration())
                .unwrap()
                .count(),
            0
        );
    }
    let after = e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap().snapshot(),
        after
    );
    assert!(e.undo());
    let mut restored = before.clone();
    restored.next_id = after.next_id; // Canonical history keeps the stable-ID high-water mark.
    assert_eq!(e.project().snapshot(), restored);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn silence_tail_overwrite_retains_hidden_notes_and_unique_ids_in_right_fragment() {
    let (mut e, clip) = fixture();
    capture(&mut e, clip);
    e.execute(source(clip, false)).unwrap();
    let before = e.project().snapshot();
    let original = e.project().midi_clip(clip).unwrap().clone();
    let track = e.project().tracks()[0].id();
    let Created::Clip(painted) = e
        .execute(Command::PlaceMidiClipWithOverwrite {
            track,
            start: beat(6),
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("painted")
    };
    let right = e
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .iter()
        .find(|c| c.id() != clip && c.id() != painted)
        .unwrap();
    assert_eq!(right.source_start(), beat(8));
    assert_eq!(right.source_length(), beat(12));
    assert_eq!(
        right.source().one_shot_boundary(),
        MidiOneShotBoundary::LoopEnd
    );
    assert_eq!(right.notes().len(), original.notes().len());
    for (old, new) in original.notes().iter().zip(right.notes()) {
        assert_ne!(old.id(), new.id());
        assert_eq!(
            (old.start(), old.duration(), old.pitch()),
            (new.start(), new.duration(), new.pitch())
        );
    }
    let after = e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap().snapshot(),
        after
    );
    assert!(e.undo());
    let mut restored = before;
    restored.next_id = after.next_id;
    assert_eq!(e.project().snapshot(), restored);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn silence_tail_split_capacity_and_cursor_overflow_refuse_without_ids_or_history_changes() {
    let (mut e, clip) = fixture();
    capture(&mut e, clip);
    e.execute(source(clip, false)).unwrap();
    let mut snapshot = e.project().snapshot();
    let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut snapshot.tracks[0].content
    else {
        panic!("hybrid")
    };
    let template = midi_clips[0].notes[0].clone();
    let base = snapshot.next_id;
    midi_clips[0].notes = (0..hex_project::MAX_NOTES / 2 + 1)
        .map(|i| {
            let mut n = template.clone();
            n.id = base + i as u64;
            n
        })
        .collect();
    snapshot.next_id = base + midi_clips[0].notes.len() as u64;
    let mut e = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    e.execute(Command::TrimMidiClipEdge {
        clip,
        edge: hex_project::TrimEdge::Right,
        beat: beat(22),
    })
    .unwrap();
    let redo = e.project().snapshot();
    assert!(e.undo());
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::SplitMidiClip {
            clip,
            beat: beat(7)
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    let track = e.project().tracks()[0].id();
    assert!(
        e.execute(Command::PlaceMidiClipWithOverwrite {
            track,
            start: beat(6),
            length: beat(1),
        })
        .is_err(),
        "retaining hidden notes in both overwrite fragments must obey the final note cap"
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), redo);

    let (mut e, clip) = fixture();
    capture(&mut e, clip);
    e.execute(source(clip, false)).unwrap();
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(12),
        source_start: BeatTime::from_raw(u64::MAX),
        loop_start: beat(2),
        loop_end: beat(6),
        looping: false,
        length: beat(20),
    })
    .unwrap();
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::SplitMidiClip {
            clip,
            beat: beat(7)
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn silence_tail_explicit_repeat_activation_uses_common_transition_and_keeps_intro() {
    let (mut e, clip) = fixture();
    capture(&mut e, clip);
    e.execute(source(clip, false)).unwrap();
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(12),
        source_start: beat(20),
        loop_start: beat(2),
        loop_end: beat(6),
        looping: false,
        length: beat(20),
    })
    .unwrap();
    let before = e.project().snapshot();
    let c = e.project().midi_clip(clip).unwrap();
    let notes = c.notes().to_vec();
    let start = c
        .source()
        .one_shot_boundary()
        .loop_activation_start(c.source_start(), c.loop_start(), c.loop_end())
        .unwrap();
    assert_eq!(start, beat(4));
    assert_eq!(
        MidiOneShotBoundary::LoopEnd
            .loop_activation_start(beat(1), beat(2), beat(6))
            .unwrap(),
        beat(1)
    );
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(12),
        source_start: start,
        loop_start: beat(2),
        loop_end: beat(6),
        looping: true,
        length: beat(20),
    })
    .unwrap();
    let c = e.project().midi_clip(clip).unwrap();
    assert_eq!(c.notes(), notes);
    assert!(c.source_playback().unwrap().has_initial_pass());
    assert!(
        c.source_playback()
            .unwrap()
            .note_occurrences(beat(4), beat(5))
            .unwrap()
            .count()
            > 1
    );
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
    // Ordinary commands must not silently normalize a malformed Repeat cursor.
    assert!(
        e.execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(12),
            source_start: beat(20),
            loop_start: beat(2),
            loop_end: beat(6),
            looping: true,
            length: beat(20)
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
}
