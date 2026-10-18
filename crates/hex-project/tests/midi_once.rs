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
            loop_end: beat(6),
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
    cell.source.source_start_raw = cell.source.loop_end_raw;
    assert!(Project::from_snapshot(invalid).is_err());
}
