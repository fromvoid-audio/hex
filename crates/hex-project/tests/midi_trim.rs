use hex_project::{BeatTime, ClipId, Command, Created, Editor, Project, TrimEdge};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).expect("small beat")
}
fn fixture(looping: bool, source_start: u64) -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "hybrid".into(),
        })
        .expect("track")
    else {
        panic!("track id")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(10),
            length: beat(12),
        })
        .expect("clip")
    else {
        panic!("clip id")
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: beat(source_start),
            loop_start: beat(2),
            loop_end: beat(6),
            looping,
            length: beat(12),
        })
        .expect("source");
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(4),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("note");
    (editor, clip)
}

#[test]
fn midi_trim_preserves_notes_source_and_history_and_can_reveal_them_again() {
    let (mut editor, id) = fixture(false, 0);
    let original = editor.project().midi_clip(id).expect("clip").clone();
    let candidate = original
        .preview_edge_trim(TrimEdge::Left, beat(13))
        .expect("preview");
    assert_eq!(candidate.start(), beat(13));
    assert_eq!(candidate.length(), beat(9));
    assert_eq!(candidate.source_start(), beat(3));
    assert_eq!(editor.project().midi_clip(id), Some(&original));
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            beat: beat(13),
        })
        .expect("trim");
    let trimmed = editor.project().midi_clip(id).expect("clip").clone();
    assert_eq!(trimmed.notes(), original.notes());
    assert_eq!(trimmed.source_length(), original.source_length());
    assert_eq!(trimmed.loop_start(), original.loop_start());
    assert_eq!(trimmed.loop_end(), original.loop_end());
    assert!(editor.undo());
    assert_eq!(editor.project().midi_clip(id), Some(&original));
    assert!(editor.redo());
    assert_eq!(editor.project().midi_clip(id), Some(&trimmed));
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            beat: beat(10),
        })
        .expect("reveal");
    assert_eq!(editor.project().midi_clip(id), Some(&original));
    let restored = Project::from_snapshot(editor.project().snapshot()).expect("snapshot roundtrip");
    assert_eq!(restored.midi_clip(id), Some(&original));
}

#[test]
fn midi_trim_bounds_keep_nonempty_source_and_allow_silent_right_extension() {
    let (editor, id) = fixture(false, 3);
    let clip = editor.project().midi_clip(id).expect("clip");
    let earliest = clip
        .preview_edge_trim(TrimEdge::Left, BeatTime::ZERO)
        .expect("left bound");
    assert_eq!(earliest.start(), beat(7));
    assert_eq!(earliest.source_start(), BeatTime::ZERO);
    let latest = clip
        .preview_edge_trim(TrimEdge::Left, BeatTime::from_raw(u64::MAX))
        .expect("source bound");
    assert_eq!(latest.source_start().raw(), beat(8).raw() - 1);
    assert_eq!(latest.start().raw() + latest.length().raw(), beat(22).raw());
    let shortest = clip
        .preview_edge_trim(TrimEdge::Right, BeatTime::ZERO)
        .expect("minimum");
    assert_eq!(shortest.length().raw(), 1);
    let longest = clip
        .preview_edge_trim(TrimEdge::Right, BeatTime::from_raw(u64::MAX))
        .expect("maximum");
    assert_eq!(longest.start().raw() + longest.length().raw(), u64::MAX);
    assert_eq!(longest.source_start(), clip.source_start());
}

#[test]
fn midi_loop_left_trim_advances_intro_and_rewinds_loop_phase_without_note_expansion() {
    let (editor, id) = fixture(true, 1);
    let intro = editor.project().midi_clip(id).expect("clip");
    let advanced = intro
        .preview_edge_trim(TrimEdge::Left, beat(18))
        .expect("wrap intro");
    assert_eq!(advanced.source_start(), beat(5)); // 1 + 8 -> loop [2,6): 5
    let earliest = intro
        .preview_edge_trim(TrimEdge::Left, BeatTime::ZERO)
        .expect("intro bound");
    assert_eq!(earliest.start(), beat(9));
    assert_eq!(earliest.source_start(), beat(0));
    let (mut editor, id) = fixture(true, 3);
    let clip = editor.project().midi_clip(id).expect("clip").clone();
    let rewound = clip
        .preview_edge_trim(TrimEdge::Left, beat(8))
        .expect("rewind");
    assert_eq!(rewound.source_start(), beat(5));
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            beat: beat(8),
        })
        .expect("apply");
    let changed = editor.project().midi_clip(id).expect("clip");
    assert_eq!(changed.notes(), clip.notes());
    assert_eq!(
        changed.source_playback().expect("mapping").source_start(),
        beat(5)
    );
    assert_eq!(
        changed.start().raw() + changed.length().raw(),
        beat(22).raw()
    );
}

#[test]
fn midi_unchanged_resize_keeps_redo_and_source_identity() {
    let (mut editor, id) = fixture(true, 3);
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: id,
            edge: TrimEdge::Right,
            beat: beat(16),
        })
        .expect("resize");
    assert!(editor.undo());
    let before = editor
        .project()
        .midi_clip(id)
        .expect("clip")
        .source_identity();
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            beat: beat(10),
        })
        .expect("noop");
    assert!(editor.can_redo());
    assert_eq!(
        before,
        editor
            .project()
            .midi_clip(id)
            .expect("clip")
            .source_identity()
    );
}
