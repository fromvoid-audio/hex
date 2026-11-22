use hex_project::{BeatTime, Command, Created, Editor, Project, SessionCellContent};

fn beats(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

#[test]
fn session_sources_are_independent_and_existing_note_commands_resolve_both_owners() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let arrangement = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beats(8),
            length: beats(4),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene a".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let session_clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session".into(),
            source_length: beats(4),
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("session clip"),
    };
    let arrangement_identity = editor
        .project()
        .midi_clip(arrangement)
        .unwrap()
        .source_identity();
    let session_identity = editor
        .project()
        .session_midi_clip(session_clip)
        .unwrap()
        .source_identity();
    assert_ne!(arrangement_identity, session_identity);

    let arrangement_note = match editor
        .execute(Command::AddMidiNote {
            clip: arrangement,
            start: BeatTime::ZERO,
            duration: beats(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note"),
    };
    assert_ne!(
        editor
            .project()
            .midi_clip(arrangement)
            .unwrap()
            .source_identity(),
        arrangement_identity
    );
    assert_eq!(
        editor
            .project()
            .session_midi_clip(session_clip)
            .unwrap()
            .source_identity(),
        session_identity
    );
    let session_note = match editor
        .execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ZERO,
            duration: beats(1),
            pitch: 64,
            channel: 0,
            velocity: 90,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note"),
    };
    assert_ne!(
        editor
            .project()
            .session_midi_clip(session_clip)
            .unwrap()
            .source_identity(),
        session_identity
    );

    editor
        .execute(Command::SetMidiClipSource {
            clip: arrangement,
            source_length: beats(3),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: beats(3),
            looping: false,
            length: beats(3),
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .midi_source(session_clip)
            .unwrap()
            .source_length(),
        beats(4)
    );
    editor
        .execute(Command::SetSessionMidiClipSource {
            clip: session_clip,
            source_length: beats(2),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: beats(2),
            looping: false,
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .midi_clip(arrangement)
            .unwrap()
            .source_length(),
        beats(3)
    );
    assert_eq!(
        editor
            .project()
            .midi_note(arrangement_note)
            .unwrap()
            .pitch(),
        60
    );
    assert_eq!(
        editor.project().midi_note(session_note).unwrap().pitch(),
        64
    );

    editor
        .execute(Command::UpdateMidiNote {
            note: session_note,
            start: beats(1),
            duration: BeatTime::from_raw(1),
            pitch: 65,
            channel: 2,
            velocity: 88,
        })
        .unwrap();
    assert_eq!(
        editor.project().midi_note(session_note).unwrap().pitch(),
        65
    );
    editor
        .execute(Command::RemoveMidiNote(arrangement_note))
        .unwrap();
    assert!(editor.project().midi_note(arrangement_note).is_none());
}

#[test]
fn session_copy_move_duplicate_stop_clear_and_deletes_preserve_ownership_rules() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "track".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let second_track = match editor
        .execute(Command::AddMidiTrack {
            name: "second".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let audio_track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let s1 = match editor
        .execute(Command::AddScene { name: "one".into() })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let s2 = match editor
        .execute(Command::AddScene { name: "two".into() })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let s3 = match editor
        .execute(Command::AddScene {
            name: "three".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let source = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beats(4),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip"),
    };
    let source_note = match editor
        .execute(Command::AddMidiNote {
            clip: source,
            start: BeatTime::ZERO,
            duration: beats(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    {
        Created::Note(id) => id,
        _ => panic!("note"),
    };
    let copied = match editor
        .execute(Command::CopyArrangementMidiToSession { source, scene: s1 })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("copy"),
    };
    let copied_note = editor.project().midi_source(copied).unwrap().notes()[0].id();
    assert_ne!(source_note, copied_note);
    assert_ne!(
        editor
            .project()
            .midi_clip(source)
            .unwrap()
            .source_identity(),
        editor
            .project()
            .session_midi_clip(copied)
            .unwrap()
            .source_identity()
    );
    let long_name = "x".repeat(hex_project::MAX_NAME_BYTES);
    editor
        .execute(Command::RenameSessionMidiClip {
            clip: copied,
            name: long_name.clone(),
        })
        .unwrap();
    let duplicated = match editor
        .execute(Command::DuplicateSessionMidiCell {
            track,
            from_scene: s1,
            to_scene: s2,
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("duplicate"),
    };
    assert_ne!(copied, duplicated);
    assert_ne!(
        editor
            .project()
            .session_midi_clip(copied)
            .unwrap()
            .source_identity(),
        editor
            .project()
            .session_midi_clip(duplicated)
            .unwrap()
            .source_identity()
    );
    assert_ne!(
        copied_note,
        editor.project().midi_source(duplicated).unwrap().notes()[0].id()
    );
    assert_eq!(
        editor
            .project()
            .session_midi_clip(duplicated)
            .unwrap()
            .name(),
        long_name
    );
    editor
        .execute(Command::MoveSessionCell {
            from_track: track,
            from_scene: s2,
            to_track: second_track,
            to_scene: s3,
        })
        .unwrap();
    assert!(editor.project().session_cell(track, s2).is_none());
    assert!(matches!(
        editor.project().session_cell(second_track, s3),
        Some(SessionCellContent::Midi(_))
    ));
    editor
        .execute(Command::SetSessionStopCell { track, scene: s1 })
        .unwrap();
    assert_eq!(
        editor.project().session_cell(track, s1),
        Some(&SessionCellContent::Stop)
    );
    editor
        .execute(Command::ClearSessionCell { track, scene: s1 })
        .unwrap();
    assert!(editor.project().session_cell(track, s1).is_none());
    editor
        .execute(Command::SetSessionStopCell {
            track: audio_track,
            scene: s1,
        })
        .unwrap();
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
    editor
        .execute(Command::MoveSessionCell {
            from_track: audio_track,
            from_scene: s1,
            to_track: track,
            to_scene: s1,
        })
        .unwrap();
    assert_eq!(
        editor.project().session_cell(track, s1),
        Some(&SessionCellContent::Stop)
    );
    editor.execute(Command::RemoveTrack(track)).unwrap();
    assert_eq!(editor.project().session_cells().len(), 1);
    editor.execute(Command::RemoveScene(s3)).unwrap();
    assert!(editor.project().session_cells().is_empty());
    editor.execute(Command::RemoveTrack(second_track)).unwrap();
    assert!(editor.project().session_cells().is_empty());
}

#[test]
fn invalid_occupied_cell_and_failed_source_shrink_leave_editor_state_unchanged() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "track".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "clip".into(),
            source_length: beats(4),
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beats(3),
            duration: beats(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::RenameScene {
            scene,
            name: "renamed".into(),
        })
        .unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());
    let before = editor.project().snapshot();
    let can_undo = editor.can_undo();
    assert!(
        editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "occupied".into(),
                source_length: beats(2)
            })
            .is_err()
    );
    assert!(
        editor
            .execute(Command::SetSessionMidiClipSource {
                clip,
                source_length: beats(2),
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: beats(2),
                looping: false
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.can_undo(), can_undo);
    assert!(editor.can_redo());
}

#[test]
fn note_batches_resolve_session_sources_and_snapshot_ids_are_global() {
    use hex_project::{MidiNoteInsert, SessionCellContentSnapshot};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "track".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "clip".into(),
            source_length: beats(4),
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("clip"),
    };
    let created = editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::ZERO,
                    duration: beats(1),
                    pitch: 60,
                    channel: 0,
                    velocity: 90,
                },
                MidiNoteInsert {
                    active: true,
                    start: beats(1),
                    duration: beats(1),
                    pitch: 61,
                    channel: 0,
                    velocity: 91,
                },
            ],
        })
        .unwrap();
    let (first, last) = match created {
        Created::Notes { first, last, .. } => (first, last),
        _ => panic!("notes"),
    };
    editor
        .execute(Command::UpdateMidiNotes {
            clip,
            notes: vec![hex_project::MidiNoteUpdate {
                note: first,
                start: BeatTime::ZERO,
                duration: beats(1),
                pitch: 67,
                channel: 1,
                velocity: 100,
            }],
        })
        .unwrap();
    editor
        .execute(Command::RemoveMidiNotes {
            clip,
            notes: vec![last],
        })
        .unwrap();
    assert_eq!(editor.project().midi_source(clip).unwrap().notes().len(), 1);

    let snapshot = editor.project().snapshot();
    let mut collision = snapshot.clone();
    let session_clip_id = match &collision.session.cells[0].content {
        SessionCellContentSnapshot::Midi(clip) => clip.id,
        SessionCellContentSnapshot::Audio(_) => unreachable!(),
        SessionCellContentSnapshot::Stop => unreachable!(),
    };
    collision.session.scenes[0].id = session_clip_id;
    assert!(Project::from_snapshot(collision).is_err());
    let mut track_collision = snapshot.clone();
    track_collision.session.scenes[0].id = track_collision.tracks[0].id;
    assert!(Project::from_snapshot(track_collision).is_err());
    let note_id = match &snapshot.session.cells[0].content {
        SessionCellContentSnapshot::Midi(clip) => clip.source.notes[0].id,
        SessionCellContentSnapshot::Audio(_) => panic!("midi cell expected"),
        SessionCellContentSnapshot::Stop => panic!("midi cell expected"),
    };
    let mut note_collision = snapshot.clone();
    note_collision.session.scenes[0].id = note_id;
    assert!(Project::from_snapshot(note_collision).is_err());

    let mut duplicate_key = snapshot.clone();
    duplicate_key
        .session
        .cells
        .push(duplicate_key.session.cells[0].clone());
    assert!(Project::from_snapshot(duplicate_key).is_err());

    let mut invalid_note = snapshot;
    if let SessionCellContentSnapshot::Midi(clip) = &mut invalid_note.session.cells[0].content {
        clip.source.notes[0].start_raw = 4_u64 << 32;
    }
    assert!(Project::from_snapshot(invalid_note).is_err());

    let mut foreign_track = editor.project().snapshot();
    foreign_track.session.cells[0].track = u64::MAX;
    assert!(Project::from_snapshot(foreign_track).is_err());
    let mut foreign_scene = editor.project().snapshot();
    foreign_scene.session.cells[0].scene = u64::MAX;
    assert!(Project::from_snapshot(foreign_scene).is_err());
}

#[test]
fn missing_session_source_cell_cannot_be_moved_to_itself() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "track".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::MoveSessionCell {
                from_track: track,
                from_scene: scene,
                to_track: track,
                to_scene: scene
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn arrangement_to_session_copy_respects_combined_clip_limit_atomically() {
    use hex_project::{ClipFade, ClipSnapshot, TrackContentSnapshot};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let audio = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "one.wav".into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset"),
    };
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "keys".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let first_scene = match editor
        .execute(Command::AddScene { name: "one".into() })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let second_scene = match editor
        .execute(Command::AddScene { name: "two".into() })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let source = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beats(1),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip"),
    };
    editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene: second_scene,
            name: "session".into(),
            source_length: beats(1),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let audio_snapshot = snapshot
        .tracks
        .iter_mut()
        .find(|item| item.id == audio.value())
        .unwrap();
    let TrackContentSnapshot::Hybrid {
        audio_clips: clips, ..
    } = &mut audio_snapshot.content
    else {
        panic!("audio track")
    };
    let mut id = snapshot.next_id;
    while clips.len() + 2 < hex_project::MAX_CLIPS {
        clips.push(ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id,
            asset: asset.value(),
            start: id,
            duration: 1,
            source_offset: 0,
            source_length: 1,
            looping: false,
            source_phase: 0,
            fade: ClipFade::neutral(1).unwrap(),
        });
        id += 1;
    }
    snapshot.next_id = id;
    let full = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        full.tracks()
            .iter()
            .map(|item| item.clips().len() + item.midi_clips().len())
            .sum::<usize>()
            + full
                .session_cells()
                .iter()
                .filter(|cell| matches!(cell.content(), SessionCellContent::Midi(_)))
                .count(),
        hex_project::MAX_CLIPS
    );
    let mut editor = Editor::new(full, 4).unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::CopyArrangementMidiToSession {
                source,
                scene: first_scene
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn arrangement_to_session_copy_respects_combined_note_limit_atomically() {
    use hex_project::{MidiNoteSnapshot, SessionCellContentSnapshot, TrackContentSnapshot};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "keys".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let first_scene = match editor
        .execute(Command::AddScene { name: "one".into() })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let second_scene = match editor
        .execute(Command::AddScene { name: "two".into() })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let source = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beats(4),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip"),
    };
    let session_clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene: second_scene,
            name: "session".into(),
            source_length: beats(4),
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("session clip"),
    };
    let mut snapshot = editor.project().snapshot();
    let mut id = snapshot.next_id;
    let TrackContentSnapshot::Hybrid {
        midi_clips: arrangement_clips,
        ..
    } = &mut snapshot.tracks[0].content
    else {
        panic!("instrument track")
    };
    let arrangement_notes = &mut arrangement_clips[0].notes;
    for index in 0..(hex_project::MAX_NOTES / 2) {
        arrangement_notes.push(MidiNoteSnapshot {
            id,
            active: true,
            start_raw: index as u64,
            duration_raw: 1,
            pitch: 60,
            channel: 0,
            velocity: 100,
        });
        id += 1;
    }
    let SessionCellContentSnapshot::Midi(session_clip_snapshot) =
        &mut snapshot.session.cells[0].content
    else {
        panic!("session midi")
    };
    for index in 0..(hex_project::MAX_NOTES / 2) {
        session_clip_snapshot.source.notes.push(MidiNoteSnapshot {
            id,
            active: true,
            start_raw: index as u64,
            duration_raw: 1,
            pitch: 61,
            channel: 0,
            velocity: 100,
        });
        id += 1;
    }
    snapshot.next_id = id;
    let full = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        full.midi_source(source).unwrap().notes().len()
            + full.midi_source(session_clip).unwrap().notes().len(),
        hex_project::MAX_NOTES
    );
    let mut editor = Editor::new(full, 4).unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::CopyArrangementMidiToSession {
                source,
                scene: first_scene
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn snapshot_rejects_session_caps_before_constructing_cells_or_scenes() {
    use hex_project::{SceneSnapshot, SessionCellContentSnapshot, SessionCellSnapshot};
    let mut snapshot = Project::new(48_000).unwrap().snapshot();
    snapshot
        .session
        .scenes
        .resize_with(hex_project::MAX_SCENES + 1, || SceneSnapshot {
            id: 1,
            name: String::new(),
        });
    assert!(Project::from_snapshot(snapshot).is_err());

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id.value(),
        _ => panic!("track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id.value(),
        _ => panic!("scene"),
    };
    let mut snapshot = editor.project().snapshot();
    snapshot
        .session
        .cells
        .resize_with(hex_project::MAX_SESSION_CELLS + 1, || SessionCellSnapshot {
            track,
            scene,
            content: SessionCellContentSnapshot::Stop,
        });
    assert!(Project::from_snapshot(snapshot).is_err());
}

#[test]
fn scene_and_session_ids_survive_undo_branches_without_reuse() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let first = match editor
        .execute(Command::AddScene {
            name: "first".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    editor.undo();
    let second = match editor
        .execute(Command::AddScene {
            name: "second".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    assert_ne!(first, second);
}
