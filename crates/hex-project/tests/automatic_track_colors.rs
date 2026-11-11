use hex_project::{
    BeatTime, Command, Created, Editor, ElementColor, Project, TrackDeviceSlot, TrackId,
};

fn created_track(result: Created) -> TrackId {
    match result {
        Created::Track(id) | Created::Imported { track: id, .. } => id,
        other => panic!("expected new track: {other:?}"),
    }
}
fn execute(editor: &mut Editor, command: Command) -> TrackId {
    created_track(editor.execute(command).expect("valid creation"))
}
fn palette_counts(project: &Project) -> [usize; 9] {
    let mut counts = [0; 9];
    for track in project.tracks() {
        assert_ne!(
            track.color(),
            ElementColor::Gray,
            "new track must have a hue"
        );
        counts[track.color() as usize - 1] += 1;
    }
    counts
}

#[test]
fn all_creation_routes_choose_unused_hues_and_keep_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let audio = execute(
        &mut editor,
        Command::ImportAudioTrack {
            name: "audio".into(),
            path: "not-read.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        },
    );
    let clip = editor.project().track(audio).unwrap().clips()[0].id();
    let midi = execute(
        &mut editor,
        Command::AddMidiTrack {
            name: "midi".into(),
        },
    );
    let Created::Clip(midi_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: midi,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    let instrument = hex_project::default_instrument_state().unwrap();
    let commands = [
        Command::AddTrack {
            name: "command-t".into(),
        },
        Command::InsertTrack {
            name: "menu".into(),
            before_track: Some(audio),
        },
        Command::InsertTrackWithDevice {
            name: "device drop".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: instrument,
        },
        Command::ImportPreparedAudioTrack {
            name: "prepared".into(),
            path: "not-read2.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 48_000,
            playback_frames: 48_000,
            conversion_version: 0,
        },
        Command::ImportPreparedAudioTrackAt {
            name: "positioned".into(),
            before_track: Some(midi),
            start: 48_000,
            path: "not-read3.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 48_000,
            playback_frames: 48_000,
            conversion_version: 0,
        },
        Command::CopyClipToNewTrack {
            initial_gain: None,
            clip,
            name: "copied audio".into(),
            before_track: None,
            start: 0,
        },
        Command::CopyMidiClipToNewTrack {
            initial_gain: None,
            clip: midi_clip,
            name: "copied midi".into(),
            before_track: None,
            start: BeatTime::ZERO,
        },
    ];
    for command in commands {
        let track = execute(&mut editor, command);
        let color = editor.project().track(track).unwrap().color();
        let after = editor.project().snapshot();
        assert!(editor.undo());
        assert!(editor.project().track(track).is_none());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        assert_eq!(editor.project().track(track).unwrap().color(), color);
        assert_eq!(Project::from_snapshot(after).unwrap(), *editor.project());
        assert!(palette_counts(editor.project()).into_iter().all(|n| n <= 1));
    }
    assert_eq!(palette_counts(editor.project()), [1; 9]);
    // Exhausted palette remains balanced through actual move-to-new-track paths.
    for command in [
        Command::MoveClipToNewTrack {
            initial_gain: None,
            clip,
            name: "moved audio".into(),
            before_track: None,
            start: 0,
        },
        Command::MoveMidiClipToNewTrack {
            initial_gain: None,
            clip: midi_clip,
            name: "moved midi".into(),
            before_track: None,
            start: BeatTime::ZERO,
        },
    ] {
        execute(&mut editor, command);
        let counts = palette_counts(editor.project());
        assert!(counts.iter().max().unwrap() - counts.iter().min().unwrap() <= 1);
    }
}

#[test]
fn palette_exhaustion_balances_to_track_limit_without_recoloring() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    for i in 0..hex_project::MAX_TRACKS {
        let before: Vec<_> = editor
            .project()
            .tracks()
            .iter()
            .map(|t| (t.id(), t.color()))
            .collect();
        execute(
            &mut editor,
            Command::AddTrack {
                name: format!("track {i}"),
            },
        );
        for (id, color) in before {
            assert_eq!(editor.project().track(id).unwrap().color(), color);
        }
        let counts = palette_counts(editor.project());
        assert!(counts.iter().max().unwrap() - counts.iter().min().unwrap() <= 1);
    }
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::AddTrack {
                name: "limit".into()
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
}

#[test]
fn explicit_existing_gray_and_hues_are_preserved_and_ignored_only_for_assignment() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let gray = execute(
        &mut editor,
        Command::AddTrack {
            name: "existing gray".into(),
        },
    );
    editor
        .execute(Command::SetTrackColor {
            track: gray,
            color: ElementColor::Gray,
        })
        .unwrap();
    let red = execute(
        &mut editor,
        Command::AddTrack {
            name: "existing red".into(),
        },
    );
    editor
        .execute(Command::SetTrackColor {
            track: red,
            color: ElementColor::Red,
        })
        .unwrap();
    let before = editor.project().snapshot();
    let loaded = Project::from_snapshot(before.clone()).unwrap();
    assert_eq!(loaded.snapshot(), before);
    for i in 0..8 {
        let id = execute(
            &mut editor,
            Command::AddMidiTrack {
                name: format!("new {i}"),
            },
        );
        assert!(
            ![ElementColor::Gray, ElementColor::Red]
                .contains(&editor.project().track(id).unwrap().color())
        );
    }
    assert_eq!(
        editor.project().track(gray).unwrap().color(),
        ElementColor::Gray
    );
    assert_eq!(
        editor.project().track(red).unwrap().color(),
        ElementColor::Red
    );
}
