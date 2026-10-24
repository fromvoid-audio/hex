use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, MidiViewResizeOperation as Resize, Project, TrackId,
};

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture(looping: bool, start: u64, length: u64) -> (Editor, TrackId, ClipId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "loop".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(10),
            length: beat(length),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(10),
        source_start: beat(start),
        loop_start: beat(1),
        loop_end: beat(6),
        looping,
        length: beat(length),
    })
    .unwrap();
    for (start, duration, pitch) in [(2, 2, 60), (3, 1, 64), (7, 1, 67)] {
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
    (e, track, clip)
}
fn resize(e: &mut Editor, clip: ClipId, operation: Resize) {
    e.execute(Command::ResizeMidiClipView { clip, operation })
        .unwrap();
}

#[test]
fn first_loop_captures_the_current_window_and_retains_trimmed_and_hidden_notes() {
    let (mut e, _, clip) = fixture(false, 2, 3);
    e.execute(Command::TrimMidiClipEdge {
        clip,
        edge: hex_project::TrimEdge::Left,
        beat: beat(11),
    })
    .unwrap();
    let original = e.project().midi_clip(clip).unwrap().clone();
    let identity = original.source_identity();
    let operation = Resize::RepeatOuterEnd { beat: beat(18) };
    let preview = original.preview_loop_resize(operation).unwrap();
    assert_eq!(preview.first_repeat_boundary(), Some(beat(13)));
    assert_eq!(
        original.source_identity(),
        identity,
        "preview retains the shared source"
    );
    resize(&mut e, clip, operation);
    let repeated = e.project().midi_clip(clip).unwrap();
    assert_eq!((repeated.start(), repeated.length()), (beat(11), beat(7)));
    assert_eq!(
        (repeated.loop_start(), repeated.loop_end()),
        (beat(3), beat(5))
    );
    assert_eq!(repeated.source_start(), original.source_start());
    assert_eq!(repeated.playback_offset(), original.playback_offset());
    assert_eq!(repeated.notes(), original.notes());
    let playback = repeated.source_playback().unwrap();
    let note = repeated.notes().iter().find(|n| n.pitch() == 64).unwrap();
    assert_eq!(
        playback
            .note_occurrences(note.start(), note.duration())
            .unwrap()
            .iter()
            .map(|n| (n.timeline_start(), n.gate_duration()))
            .collect::<Vec<_>>(),
        vec![
            (beat(0), beat(1)),
            (beat(2), beat(1)),
            (beat(4), beat(1)),
            (beat(6), beat(1))
        ]
    );
    let hidden = repeated.notes().iter().find(|n| n.pitch() == 67).unwrap();
    assert_eq!(
        playback
            .note_occurrences(hidden.start(), hidden.duration())
            .unwrap()
            .count(),
        0
    );
    // Window chase retains the sounding prefix; canonical loop boundaries still
    // require a note-on inside the loop rather than synthesizing new authored notes.
    let crossing = repeated.notes().iter().find(|n| n.pitch() == 60).unwrap();
    assert_eq!(
        playback
            .note_occurrences(crossing.start(), crossing.duration())
            .unwrap()
            .iter()
            .map(|n| (n.timeline_start(), n.gate_duration()))
            .collect::<Vec<_>>(),
        vec![(beat(0), beat(1))]
    );
    assert!(e.undo());
    assert_eq!(e.project().midi_clip(clip), Some(&original));
    assert!(e.redo());
    assert_eq!(
        e.project().midi_clip(clip).unwrap().first_repeat_boundary(),
        Some(beat(13))
    );
}

#[test]
fn cycle_resize_keeps_outer_occupancy_and_rebases_a_wrapped_window_at_the_same_phase() {
    let (mut e, _, clip) = fixture(true, 2, 20);
    e.execute(Command::TrimMidiClipEdge {
        clip,
        edge: hex_project::TrimEdge::Left,
        beat: beat(18),
    })
    .unwrap();
    let original = e.project().midi_clip(clip).unwrap().clone();
    assert_eq!(original.first_repeat_boundary(), Some(beat(19)));
    assert_eq!(
        original
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(beat(0)),
        Some(beat(5))
    );
    resize(&mut e, clip, Resize::CycleEnd { beat: beat(21) });
    let changed = e.project().midi_clip(clip).unwrap();
    assert_eq!(
        (changed.start(), changed.length()),
        (original.start(), original.length())
    );
    assert_eq!(changed.first_repeat_boundary(), Some(beat(21)));
    assert_eq!(
        (changed.loop_start(), changed.loop_end()),
        (beat(1), beat(8))
    );
    assert_eq!(changed.playback_offset(), beat(10));
    assert_eq!(
        changed
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(beat(0)),
        Some(beat(5))
    );
    assert_eq!(changed.notes(), original.notes());
    assert!(e.undo());
    assert_eq!(e.project().midi_clip(clip), Some(&original));
    assert!(e.redo());
    assert_eq!(
        e.project().midi_clip(clip).unwrap().first_repeat_boundary(),
        Some(beat(21))
    );
}

#[test]
fn captured_once_keeps_its_cycle_when_reactivated_from_a_silent_elapsed_window() {
    let (mut e, _, clip) = fixture(false, 1, 3);
    resize(&mut e, clip, Resize::RepeatOuterEnd { beat: beat(18) });
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(10),
        source_start: beat(1),
        loop_start: beat(2),
        loop_end: beat(4),
        looping: false,
        length: beat(8),
    })
    .unwrap();
    e.execute(Command::TrimMidiClipEdge {
        clip,
        edge: hex_project::TrimEdge::Left,
        beat: beat(16),
    })
    .unwrap();
    let original = e.project().midi_clip(clip).unwrap().clone();
    assert_eq!(
        original
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(beat(0)),
        None
    );
    resize(&mut e, clip, Resize::RepeatOuterEnd { beat: beat(22) });
    let repeated = e.project().midi_clip(clip).unwrap();
    assert_eq!(
        (repeated.loop_start(), repeated.loop_end()),
        (beat(2), beat(4))
    );
    assert_eq!(repeated.playback_offset(), original.playback_offset());
    assert_eq!(repeated.source_length(), original.source_length());
    assert_eq!(repeated.notes(), original.notes());
    assert_eq!(
        repeated
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(beat(0)),
        Some(beat(3))
    );
    assert_eq!(repeated.first_repeat_boundary(), Some(beat(17)));
}

#[test]
fn one_shot_silent_tail_is_part_of_the_captured_cycle_without_deleting_source_notes() {
    let (mut e, _, clip) = fixture(false, 2, 12);
    let original = e.project().midi_clip(clip).unwrap().clone();
    resize(&mut e, clip, Resize::RepeatOuterEnd { beat: beat(38) });
    let repeated = e.project().midi_clip(clip).unwrap();
    assert_eq!(
        (repeated.loop_start(), repeated.loop_end()),
        (beat(2), beat(14))
    );
    assert_eq!(repeated.source_length(), beat(14));
    assert_eq!(repeated.notes(), original.notes());
    let playback = repeated.source_playback().unwrap();
    assert_eq!(playback.source_phase_at_elapsed(beat(11)), Some(beat(13)));
    assert_eq!(playback.source_phase_at_elapsed(beat(12)), Some(beat(2)));
    assert!(e.undo());
    assert_eq!(e.project().midi_clip(clip), Some(&original));
}

#[test]
fn outer_growth_uses_shared_hybrid_occupancy_and_one_history_edge() {
    let (mut e, track, clip) = fixture(false, 2, 3);
    let Created::Asset(asset) = e
        .execute(Command::AddAsset {
            path: "fixture.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    e.execute(Command::PlaceClip {
        track,
        asset,
        start: 14 * 24_000,
        duration: 48_000,
        source_offset: 0,
        source_length: 48_000,
        looping: false,
    })
    .unwrap();
    let Created::Clip(victim) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(16),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("victim")
    };
    e.execute(Command::AddMidiNote {
        clip: victim,
        start: BeatTime::ZERO,
        duration: beat(4),
        pitch: 71,
        channel: 0,
        velocity: 90,
    })
    .unwrap();
    let before = e.project().track(track).unwrap().clone();
    resize(&mut e, clip, Resize::RepeatOuterEnd { beat: beat(18) });
    let after = e.project().track(track).unwrap().clone();
    assert!(after.clips().is_empty());
    let right = e.project().midi_clip(victim).unwrap();
    assert_eq!((right.start(), right.length()), (beat(18), beat(2)));
    let note = &right.notes()[0];
    assert_eq!((note.pitch(), note.velocity()), (71, 90));
    assert_eq!(
        right
            .source_playback()
            .unwrap()
            .note_occurrences(note.start(), note.duration())
            .unwrap()
            .iter()
            .map(|n| (n.timeline_start(), n.gate_duration()))
            .collect::<Vec<_>>(),
        vec![(beat(0), beat(2))],
        "the common occupancy owner retains the sounding right gate"
    );
    assert!(e.undo());
    assert_eq!(e.project().track(track), Some(&before));
    assert!(e.redo());
    assert_eq!(e.project().track(track), Some(&after));
}

#[test]
fn invalid_cycle_and_overflow_refuse_without_musical_or_history_changes() {
    let (mut e, track, clip) = fixture(true, 5, 20);
    e.execute(Command::TrimMidiClipEdge {
        clip,
        edge: hex_project::TrimEdge::Left,
        beat: beat(12),
    })
    .unwrap();
    // First pass was only one beat; this window is already inside a repeat.
    assert_eq!(
        e.project()
            .midi_clip(clip)
            .unwrap()
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(beat(0)),
        Some(beat(2))
    );
    let before = e.project().snapshot();
    for operation in [
        Resize::CycleEnd { beat: beat(13) },
        Resize::RepeatOuterEnd { beat: beat(12) },
    ] {
        assert!(
            e.execute(Command::ResizeMidiClipView { clip, operation })
                .is_err()
        );
        assert_eq!(e.project().snapshot(), before);
    }
    e.execute(Command::MoveMidiClip {
        clip,
        track,
        start: BeatTime::ZERO,
    })
    .unwrap();
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::ResizeMidiClipView {
            clip,
            operation: Resize::RepeatOuterEnd {
                beat: BeatTime::from_raw(u64::MAX)
            }
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
}
