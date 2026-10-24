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
    assert_eq!(candidate.source_start(), original.source_start());
    assert_eq!(candidate.playback_offset(), beat(3));
    assert_eq!(editor.project().midi_clip(id), Some(&original));
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            beat: beat(13),
        })
        .expect("trim");
    let trimmed = editor.project().midi_clip(id).expect("clip").clone();
    assert_eq!(
        trimmed
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(BeatTime::ZERO),
        Some(beat(3))
    );
    let heard = trimmed
        .source_playback()
        .unwrap()
        .note_occurrences(beat(4), beat(1))
        .unwrap()
        .iter()
        .next()
        .unwrap();
    assert_eq!(
        (heard.timeline_start(), heard.gate_duration()),
        (beat(1), beat(1))
    );
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
    assert_eq!(earliest.start(), BeatTime::ZERO);
    assert_eq!(earliest.source_start(), BeatTime::ZERO);
    assert_eq!(earliest.source_prefix(), beat(7));
    let latest = clip
        .preview_edge_trim(TrimEdge::Left, BeatTime::from_raw(u64::MAX))
        .expect("source bound");
    assert_eq!(latest.source_start(), clip.source_start());
    assert_eq!(latest.length().raw(), 1);
    let silent = clip
        .source_playback()
        .unwrap()
        .with_playback_offset(latest.playback_offset())
        .unwrap();
    assert_eq!(silent.source_phase_at_elapsed(BeatTime::ZERO), None);
    assert_eq!(
        silent.note_occurrences(beat(4), beat(1)).unwrap().count(),
        0
    );
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
    assert_eq!(advanced.source_start(), intro.source_start());
    let audible = intro
        .source_playback()
        .unwrap()
        .with_playback_offset(advanced.playback_offset())
        .unwrap();
    assert_eq!(
        audible.source_phase_at_elapsed(BeatTime::ZERO),
        Some(beat(5))
    ); // 1 + 8 -> loop [2,6): 5
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

#[test]
fn one_shot_left_extension_prepends_blank_time_and_preserves_absolute_notes_and_history() {
    let (mut editor, id) = fixture(false, 3);
    let before = editor.project().snapshot();
    let original = editor.project().midi_clip(id).unwrap().clone();
    let old_note = &original.notes()[0];
    let absolute = i128::from(original.start().raw()) + i128::from(old_note.start().raw())
        - i128::from(original.source_start().raw());
    let preview = original.preview_edge_trim(TrimEdge::Left, beat(2)).unwrap();
    assert_eq!(
        (preview.start(), preview.length(), preview.source_prefix()),
        (beat(2), beat(20), beat(5))
    );
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            beat: beat(2),
        })
        .unwrap();
    let extended = editor.project().midi_clip(id).unwrap();
    assert_eq!(extended.source_start(), BeatTime::ZERO);
    assert_eq!(extended.source_length(), beat(13));
    assert_eq!(
        (extended.loop_start(), extended.loop_end()),
        (beat(7), beat(11))
    );
    assert_eq!(extended.notes().len(), 1);
    let note = &extended.notes()[0];
    assert_eq!(
        i128::from(extended.start().raw()) + i128::from(note.start().raw())
            - i128::from(extended.source_start().raw()),
        absolute
    );
    assert_eq!(
        (
            note.id(),
            note.duration(),
            note.pitch(),
            note.channel(),
            note.velocity()
        ),
        (
            old_note.id(),
            old_note.duration(),
            old_note.pitch(),
            old_note.channel(),
            old_note.velocity()
        )
    );
    assert_eq!(
        original.notes()[0].start(),
        beat(4),
        "old COW/history source unchanged"
    );
    let after = editor.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap().snapshot(),
        after
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
    // Hiding the prefix does not destructively normalize notes or consume IDs.
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            beat: beat(10),
        })
        .unwrap();
    let shrunk = editor.project().midi_clip(id).unwrap();
    assert_eq!(shrunk.source_start(), BeatTime::ZERO);
    assert_eq!(
        shrunk
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(BeatTime::ZERO),
        Some(beat(8))
    );
    assert_eq!(shrunk.notes()[0].id(), old_note.id());
    assert_eq!(
        shrunk.start().raw() + shrunk.notes()[0].start().raw()
            - shrunk.source_start().raw()
            - shrunk.playback_offset().raw(),
        absolute as u64
    );
}

#[test]
fn left_extension_small_domain_keeps_right_edge_and_source_note_timeline_invariant() {
    for source_start in 0..8 {
        for start in 0..8 {
            for requested in 0..=start {
                let (mut editor, id) = fixture(false, source_start);
                editor
                    .execute(Command::MoveMidiClip {
                        clip: id,
                        track: editor.project().tracks()[0].id(),
                        start: beat(start),
                    })
                    .unwrap();
                let original = editor.project().midi_clip(id).unwrap().clone();
                let preview = original
                    .preview_edge_trim(TrimEdge::Left, beat(requested))
                    .unwrap();
                assert_eq!(preview.start(), beat(requested));
                assert_eq!(
                    preview.start().raw() + preview.length().raw(),
                    original.start().raw() + original.length().raw()
                );
                editor
                    .execute(Command::TrimMidiClipEdge {
                        clip: id,
                        edge: TrimEdge::Left,
                        beat: beat(requested),
                    })
                    .unwrap();
                let actual = editor.project().midi_clip(id).unwrap();
                for (before, after) in original.notes().iter().zip(actual.notes()) {
                    assert_eq!(before.id(), after.id());
                    assert_eq!(
                        i128::from(original.start().raw()) + i128::from(before.start().raw())
                            - i128::from(original.source_start().raw()),
                        i128::from(actual.start().raw()) + i128::from(after.start().raw())
                            - i128::from(actual.source_start().raw())
                    );
                }
                assert_eq!(
                    actual.source_playback().unwrap().arrangement_duration(),
                    preview.length()
                );
            }
        }
    }
}

#[test]
fn left_extension_checked_source_capacity_and_refused_preparation_preserve_history() {
    let (mut editor, id) = fixture(false, 0);
    editor
        .execute(Command::SetMidiClipSource {
            clip: id,
            source_length: BeatTime::from_raw(u64::MAX - 3),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::from_raw(u64::MAX - 3),
            looping: false,
            length: beat(12),
        })
        .unwrap();
    let original = editor.project().midi_clip(id).unwrap();
    let preview = original
        .preview_edge_trim(TrimEdge::Left, BeatTime::ZERO)
        .unwrap();
    assert_eq!(preview.start().raw(), original.start().raw() - 3);
    assert_eq!(preview.source_prefix().raw(), 3);
    let before = editor.project().snapshot();
    let command = Command::TrimMidiClipEdge {
        clip: id,
        edge: TrimEdge::Left,
        beat: BeatTime::ZERO,
    };
    let result = editor.execute_validated(command.clone(), |candidate| {
        assert_eq!(
            candidate.midi_clip(id).unwrap().source_length().raw(),
            u64::MAX
        );
        Err::<(), _>(hex_project::Invalid("intentional preparation refusal"))
    });
    assert!(result.is_err());
    assert_eq!(editor.project().snapshot(), before);
    editor.execute(command).unwrap();
    let actual = editor.project().midi_clip(id).unwrap();
    assert_eq!(actual.source_length().raw(), u64::MAX);
    assert_eq!(actual.start().raw() + actual.length().raw(), beat(22).raw());
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
}
