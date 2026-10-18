use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, MidiOneShotBoundary, Project, TrimEdge,
};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

fn fixture() -> (Editor, ClipId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
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
    e.execute(Command::SetMidiClipSourceWithOverwrite {
        clip,
        source_length: beat(12),
        source_start: beat(2),
        loop_start: beat(2),
        loop_end: beat(6),
        looping: false,
        length: beat(20),
    })
    .unwrap();
    (e, clip)
}

#[test]
fn outer_and_cycle_preview_project_actual_repeats_without_detaching_authored_notes() {
    let (mut e, clip) = fixture();
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(12),
        source_start: beat(7),
        loop_start: beat(2),
        loop_end: beat(6),
        looping: false,
        length: beat(20),
    })
    .unwrap();
    let before = e.project().snapshot();
    let original = e.project().midi_clip(clip).unwrap();
    let identity = original.source_identity();
    let notes = original.notes().to_vec();
    let outer = original.preview_loop_outer_resize(beat(13)).unwrap();
    assert_eq!(
        (outer.start(), outer.length(), outer.source_start()),
        (beat(1), beat(12), beat(3))
    );
    assert_eq!((outer.loop_start(), outer.loop_end()), (beat(2), beat(6)));
    assert_eq!(outer.one_shot_boundary(), MidiOneShotBoundary::LoopEnd);
    assert!(outer.looping());
    let onsets: Vec<_> = outer
        .source_playback()
        .unwrap()
        .note_occurrences(beat(4), beat(5))
        .unwrap()
        .iter()
        .map(|event| (event.timeline_start(), event.gate_duration()))
        .collect();
    assert_eq!(
        onsets,
        vec![(beat(1), beat(2)), (beat(5), beat(2)), (beat(9), beat(2))]
    );
    let command = original.loop_outer_resize_command(beat(13)).unwrap();
    assert_eq!(original.source_identity(), identity);
    assert_eq!(e.project().snapshot(), before);
    e.execute(command).unwrap();
    let canonical = e.project().midi_clip(clip).unwrap();
    assert_eq!(
        outer.source_playback().unwrap(),
        canonical.source_playback().unwrap()
    );
    assert_eq!(canonical.notes(), notes);
    let cycle = canonical.preview_loop_cycle_resize(beat(6)).unwrap();
    assert_eq!(
        cycle.length(),
        beat(12),
        "first-marker editing keeps outer occupancy fixed"
    );
    assert_eq!(cycle.loop_end(), beat(8));
    let onsets: Vec<_> = cycle
        .source_playback()
        .unwrap()
        .note_occurrences(beat(6), beat(1))
        .unwrap()
        .iter()
        .map(|event| event.timeline_start())
        .collect();
    assert_eq!(onsets, vec![beat(3), beat(9)]);
    let command = canonical.loop_cycle_resize_command(beat(6)).unwrap();
    e.execute(command).unwrap();
    assert_eq!(
        cycle.source_playback().unwrap(),
        e.project()
            .midi_clip(clip)
            .unwrap()
            .source_playback()
            .unwrap()
    );
    assert!(e.undo());
    assert_eq!(
        outer.source_playback().unwrap(),
        e.project()
            .midi_clip(clip)
            .unwrap()
            .source_playback()
            .unwrap()
    );
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn ordinary_trim_preview_retains_once_policy_and_recovers_silence_without_source_copy() {
    let (mut e, clip) = fixture();
    let original = e.project().midi_clip(clip).unwrap();
    let identity = original.source_identity();
    let preview = original.preview_edge_trim(TrimEdge::Left, beat(7)).unwrap();
    assert_eq!(preview.source_start(), beat(8));
    assert_eq!(preview.source_length(), beat(12));
    assert!(!preview.source_playback().unwrap().has_initial_pass());
    assert_eq!(original.source_identity(), identity);
    assert!(original.preview_loop_cycle_resize(beat(4)).is_err());
    e.execute(Command::TrimMidiClipEdge {
        clip,
        edge: TrimEdge::Left,
        beat: beat(7),
    })
    .unwrap();
    let canonical = e.project().midi_clip(clip).unwrap();
    assert_eq!(
        canonical.source_playback().unwrap(),
        preview.source_playback().unwrap()
    );
    let restored = canonical
        .preview_edge_trim(TrimEdge::Left, beat(1))
        .unwrap();
    let events: Vec<_> = restored
        .source_playback()
        .unwrap()
        .note_occurrences(beat(4), beat(5))
        .unwrap()
        .iter()
        .map(|event| (event.timeline_start(), event.gate_duration()))
        .collect();
    assert_eq!(events, vec![(beat(2), beat(2))]);
    assert_eq!(restored.one_shot_boundary(), MidiOneShotBoundary::LoopEnd);
    assert_eq!(
        (restored.loop_start(), restored.loop_end()),
        (beat(2), beat(6))
    );
}
