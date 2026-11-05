use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, MAX_CLIPS, MAX_NOTES, MidiClipSnapshot, Project,
    Snapshot, TrackContentSnapshot, TrackSnapshot,
};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).expect("small beat")
}

fn editor_with_clip(
    looping: bool,
    source_start: u64,
    source_length: u64,
    arrangement_length: u64,
) -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
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
            length: beat(arrangement_length),
        })
        .expect("clip")
    else {
        panic!("clip id")
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(source_length),
            source_start: beat(source_start),
            loop_start: beat(2.min(source_length - 1)),
            loop_end: beat(source_length),
            looping,
            length: beat(arrangement_length),
        })
        .expect("source");
    let notes = if source_length >= 5 {
        [(1, 2, 60), (4, 2, 67)].as_slice()
    } else {
        [(0, 1, 60)].as_slice()
    };
    for &(start, duration, pitch) in notes {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: beat(start),
                duration: beat(duration),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .expect("note");
    }
    (editor, clip)
}

#[test]
fn one_shot_split_materializes_a_carried_gate_into_a_finite_right_source() {
    let (mut editor, clip) = editor_with_clip(false, 2, 8, 8);
    let original = editor.project().midi_clip(clip).expect("original").clone();
    let original_notes = original.notes().to_vec();
    let Created::Clip(right_id) = editor
        .execute(Command::SplitMidiClip {
            clip,
            beat: beat(12),
        })
        .expect("split")
    else {
        panic!("right clip id")
    };

    let left = editor.project().midi_clip(clip).expect("left");
    let right = editor.project().midi_clip(right_id).expect("right");
    assert_eq!(left.start(), beat(10));
    assert_eq!(left.length(), beat(2));
    assert_eq!(left.source_start(), beat(2));
    assert_eq!(left.notes(), original_notes);
    assert_eq!(right.start(), beat(12));
    assert_eq!(right.length(), beat(6));
    assert_eq!(right.source_start(), BeatTime::ZERO);
    assert_eq!(right.source_length(), beat(6));
    assert_eq!(right.loop_start(), BeatTime::ZERO);
    assert_eq!(right.loop_end(), beat(6));
    assert!(!right.looping());
    assert_eq!(right.notes().len(), 1);
    assert_eq!(right.notes()[0].start(), BeatTime::ZERO);
    assert_eq!(right.notes()[0].duration(), beat(2));
    assert_eq!(right.notes()[0].pitch(), 67);
    assert_ne!(right.notes()[0].id(), original_notes[1].id());
    assert_eq!(left.source_identity(), original.source_identity());
    assert_ne!(left.source_identity(), right.source_identity());

    let split_snapshot = editor.project().snapshot();
    assert!(editor.undo());
    assert_eq!(editor.project().midi_clip(clip), Some(&original));
    assert!(editor.project().midi_clip(right_id).is_none());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), split_snapshot);
    let restored = Project::from_snapshot(split_snapshot).expect("snapshot roundtrip");
    assert_eq!(restored, *editor.project());
}

#[test]
fn loop_split_materializes_carry_and_later_loop_onsets_without_repeating_the_carry() {
    let (mut intro, intro_clip) = editor_with_clip(true, 1, 8, 12);
    let Created::Clip(right) = intro
        .execute(Command::SplitMidiClip {
            clip: intro_clip,
            beat: beat(14),
        })
        .expect("split in intro then loop")
    else {
        panic!("right clip id")
    };
    let right = intro.project().midi_clip(right).expect("right");
    assert_eq!(right.source_start(), BeatTime::ZERO);
    assert_eq!(right.source_length(), beat(8));
    assert!(!right.looping());
    assert_eq!(right.notes().len(), 2);
    assert_eq!(right.notes()[0].start(), BeatTime::ZERO);
    assert_eq!(right.notes()[0].duration(), beat(1));
    assert_eq!(right.notes()[1].start(), beat(5));
    assert_eq!(right.notes()[1].duration(), beat(2));

    let (mut repeating, repeating_clip) = editor_with_clip(true, 3, 8, 16);
    let Created::Clip(right) = repeating
        .execute(Command::SplitMidiClip {
            clip: repeating_clip,
            beat: beat(18),
        })
        .expect("split after loop phase advance")
    else {
        panic!("right clip id")
    };
    let right = repeating.project().midi_clip(right).expect("right");
    assert_eq!(right.source_length(), beat(8));
    assert!(!right.looping());
    assert_eq!(right.notes().len(), 2);
    assert_eq!(right.notes()[0].start(), BeatTime::ZERO);
    assert_eq!(right.notes()[0].duration(), beat(1));
    assert_eq!(right.notes()[1].start(), beat(5));
    assert_eq!(right.notes()[1].duration(), beat(2));
}

#[test]
fn invalid_boundary_splits_preserve_project_and_redo() {
    let (mut editor, clip) = editor_with_clip(false, 2, 8, 12);
    editor
        .execute(Command::Gain {
            track: editor.project().tracks()[0].id(),
            gain: 0.5,
        })
        .expect("history edit");
    assert!(editor.undo());
    let before = editor.project().clone();
    for at in [beat(9), beat(10), beat(22), beat(23)] {
        assert!(
            editor
                .execute(Command::SplitMidiClip { clip, beat: at })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }
    let Created::Clip(right) = editor
        .execute(Command::SplitMidiClip {
            clip,
            beat: beat(19),
        })
        .expect("split silent extension")
    else {
        panic!("right clip id")
    };
    assert!(
        editor
            .project()
            .midi_clip(right)
            .expect("right")
            .notes()
            .is_empty()
    );
}

#[test]
fn split_refuses_project_clip_limit_and_note_duplication_limit() {
    let clips: Vec<_> = (0..MAX_CLIPS)
        .map(|index| MidiClipSnapshot {
            content_view: None,
            active: true,
            playback_offset_raw: 0,
            one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
            volume: hex_project::MidiClipVolume::default(),
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            id: 2 + index as u64,
            start_raw: beat(2 * index as u64).raw(),
            length_raw: beat(2).raw(),
            source_length_raw: beat(2).raw(),
            source_start_raw: 0,
            loop_start_raw: 0,
            loop_end_raw: beat(2).raw(),
            looping: false,
            notes: Vec::new(),
        })
        .collect();
    let full_project = Project::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        next_id: 2 + MAX_CLIPS as u64,
        assets: Vec::new(),
        tracks: vec![TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: 1,
            name: "full".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Hybrid {
                audio_clips: Vec::new(),
                midi_clips: clips,
            },
            sends: Vec::new(),
        }],
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![hex_project::TempoPoint::new(
            BeatTime::ZERO,
            hex_project::Tempo::DEFAULT,
        )],
        meter_points: vec![hex_project::MeterPoint::new(
            BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
        return_tracks: Vec::new(),
    })
    .expect("full clip project");
    let full_clip = full_project.tracks()[0].midi_clips()[0].id();
    let mut editor = Editor::new(full_project, 8).expect("editor");
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SplitMidiClip {
                clip: full_clip,
                beat: beat(1),
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);

    let (mut editor, clip) = editor_with_clip(false, 0, 2, 2);
    let notes = (0..(MAX_NOTES / 2 + 1))
        .map(|_| hex_project::MidiNoteInsert {
            active: true,
            start: BeatTime::ZERO,
            duration: beat(2),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .collect();
    editor
        .execute(Command::AddMidiNotes { clip, notes })
        .expect("fill note limit");
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SplitMidiClip {
                clip,
                beat: beat(1),
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
}

#[test]
fn exhausted_id_space_refuses_split_without_mutating_history() {
    let (editor, clip) = editor_with_clip(false, 0, 8, 8);
    let mut snapshot = editor.project().snapshot();
    snapshot.next_id = u64::MAX;
    let mut editor =
        Editor::new(Project::from_snapshot(snapshot).expect("project"), 8).expect("editor");
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SplitMidiClip {
                clip,
                beat: beat(12),
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(!editor.can_undo());
}
