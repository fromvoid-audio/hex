use hex_project::{
    BeatTime, Command, Created, Editor, MAX_NAME_BYTES, Project, SessionCellContent,
    SessionMidiClip, TrackId,
};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).expect("valid quarter-note beat")
}

fn add_track(editor: &mut Editor, name: &str) -> TrackId {
    match editor
        .execute(Command::AddMidiTrack { name: name.into() })
        .expect("add instrument track")
    {
        Created::Track(id) => id,
        result => panic!("unexpected track result: {result:?}"),
    }
}

fn add_scene(editor: &mut Editor, name: &str) -> hex_project::SceneId {
    match editor
        .execute(Command::AddScene { name: name.into() })
        .expect("add scene")
    {
        Created::Scene(id) => id,
        result => panic!("unexpected scene result: {result:?}"),
    }
}

fn add_session_clip(
    editor: &mut Editor,
    track: TrackId,
    scene: hex_project::SceneId,
    name: &str,
    source_length: BeatTime,
) -> hex_project::ClipId {
    match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: name.into(),
            source_length,
        })
        .expect("create Session MIDI clip")
    {
        Created::SessionClip(id) => id,
        result => panic!("unexpected clip result: {result:?}"),
    }
}

fn add_note(
    editor: &mut Editor,
    clip: hex_project::ClipId,
    start: BeatTime,
    duration: BeatTime,
    pitch: u8,
    channel: u8,
    velocity: u8,
) -> hex_project::NoteId {
    match editor
        .execute(Command::AddMidiNote {
            clip,
            start,
            duration,
            pitch,
            channel,
            velocity,
        })
        .expect("add MIDI note")
    {
        Created::Note(id) => id,
        result => panic!("unexpected note result: {result:?}"),
    }
}

fn midi_cell(project: &Project, track: TrackId, scene: hex_project::SceneId) -> &SessionMidiClip {
    match project.session_cell(track, scene).expect("populated cell") {
        SessionCellContent::Midi(clip) => clip,
        SessionCellContent::Audio(_) => panic!("MIDI cell expected"),
        SessionCellContent::Stop => panic!("MIDI cell expected"),
    }
}

fn scene_order(editor: &Editor) -> Vec<hex_project::SceneId> {
    editor
        .project()
        .scenes()
        .iter()
        .map(|scene| scene.id())
        .collect()
}

#[test]
fn duplicate_scene_copies_cells_with_fresh_ids_and_independent_sources() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let track_a = add_track(&mut editor, "keys a");
    let track_b = add_track(&mut editor, "keys b");
    let track_c = add_track(&mut editor, "keys c");
    let track_empty = add_track(&mut editor, "keys empty");
    let before = add_scene(&mut editor, "before");
    let scene_name = "s".repeat(MAX_NAME_BYTES);
    let source_scene = add_scene(&mut editor, &scene_name);
    let after = add_scene(&mut editor, "after");

    let source_a = add_session_clip(&mut editor, track_a, source_scene, "lead source", beat(8));
    let note_a = add_note(
        &mut editor,
        source_a,
        beat(2),
        BeatTime::from_ratio(1, 2).unwrap(),
        73,
        3,
        91,
    );
    editor
        .execute(Command::SetSessionMidiClipSource {
            clip: source_a,
            source_length: beat(8),
            source_start: beat(1),
            loop_start: beat(2),
            loop_end: beat(6),
            looping: true,
        })
        .unwrap();
    let source_b = add_session_clip(&mut editor, track_b, source_scene, "bass source", beat(4));
    let note_b = add_note(&mut editor, source_b, beat(1), beat(1), 48, 1, 108);
    editor
        .execute(Command::SetSessionStopCell {
            track: track_c,
            scene: source_scene,
        })
        .unwrap();
    // track_empty deliberately has no cell in source_scene.

    let source_project = editor.project().clone();
    let original_identity = midi_cell(&source_project, track_a, source_scene).source_identity();
    let duplicate = match editor
        .execute(Command::DuplicateScene(source_scene))
        .unwrap()
    {
        Created::Scene(id) => id,
        result => panic!("unexpected scene duplication result: {result:?}"),
    };
    assert_eq!(
        scene_order(&editor),
        vec![before, source_scene, duplicate, after]
    );
    assert_eq!(editor.project().scenes()[2].name(), scene_name);

    let duplicate_a = midi_cell(editor.project(), track_a, duplicate);
    let duplicate_b = midi_cell(editor.project(), track_b, duplicate);
    let original_a = midi_cell(editor.project(), track_a, source_scene);
    let original_b = midi_cell(editor.project(), track_b, source_scene);
    assert_eq!(duplicate_a.name(), original_a.name());
    assert_eq!(duplicate_b.name(), original_b.name());
    assert_ne!(duplicate_a.id(), source_a);
    assert_ne!(duplicate_b.id(), source_b);
    assert_ne!(duplicate_a.source_identity(), original_identity);
    assert_ne!(duplicate_a.source_identity(), original_a.source_identity());
    assert_eq!(duplicate_a.source().source_length(), beat(8));
    assert_eq!(duplicate_a.source().source_start(), beat(1));
    assert_eq!(duplicate_a.source().loop_start(), beat(2));
    assert_eq!(duplicate_a.source().loop_end(), beat(6));
    assert!(duplicate_a.source().looping());
    assert_eq!(duplicate_b.source().source_length(), beat(4));
    assert_eq!(duplicate_b.source().source_start(), BeatTime::ZERO);
    assert!(!duplicate_b.source().looping());
    assert_eq!(
        editor.project().session_cell(track_c, duplicate),
        Some(&SessionCellContent::Stop)
    );
    assert_eq!(editor.project().session_cell(track_empty, duplicate), None);

    let copied_note_a = duplicate_a.source().notes()[0].id();
    let copied_note_b = duplicate_b.source().notes()[0].id();
    let duplicate_b_id = duplicate_b.id();
    assert_ne!(copied_note_a, note_a);
    assert_ne!(copied_note_b, note_b);
    for (copied, original) in [
        (
            duplicate_a.source().notes()[0].clone(),
            original_a.source().notes()[0].clone(),
        ),
        (
            duplicate_b.source().notes()[0].clone(),
            original_b.source().notes()[0].clone(),
        ),
    ] {
        assert_eq!(copied.start(), original.start());
        assert_eq!(copied.duration(), original.duration());
        assert_eq!(copied.pitch(), original.pitch());
        assert_eq!(copied.channel(), original.channel());
        assert_eq!(copied.velocity(), original.velocity());
    }
    let duplicated_project = editor.project().clone();

    editor
        .execute(Command::UpdateMidiNote {
            note: copied_note_a,
            start: beat(3),
            duration: beat(1),
            pitch: 77,
            channel: 5,
            velocity: 99,
        })
        .expect("edit duplicated note independently");
    editor
        .execute(Command::SetSessionMidiClipSource {
            clip: duplicate_b_id,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: beat(1),
            loop_end: beat(3),
            looping: true,
        })
        .expect("edit duplicated source independently");
    assert_eq!(editor.project().midi_note(note_a).unwrap().pitch(), 73);
    assert_eq!(editor.project().midi_note(note_b).unwrap().pitch(), 48);
    assert_eq!(
        midi_cell(editor.project(), track_a, source_scene)
            .source()
            .notes()[0]
            .start(),
        beat(2)
    );
    assert!(
        !midi_cell(editor.project(), track_b, source_scene)
            .source()
            .looping()
    );

    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(editor.project(), &duplicated_project);
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert!(editor.redo());
    let edited_project = editor.project().clone();
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(editor.project(), &duplicated_project);
    assert!(editor.undo());
    let mut restored_snapshot = editor.project().snapshot();
    let original_snapshot = source_project.snapshot();
    // Undo preserves the allocator high-water mark so IDs are never reused.
    restored_snapshot.next_id = original_snapshot.next_id;
    assert_eq!(restored_snapshot, original_snapshot);
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project(), &duplicated_project);
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(editor.project(), &edited_project);
}

#[test]
fn move_scene_uses_stable_targets_and_invalid_moves_preserve_redo() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = add_track(&mut editor, "instrument");
    let first = add_scene(&mut editor, "first");
    let middle = add_scene(&mut editor, "middle");
    let third = add_scene(&mut editor, "third");
    let last = add_scene(&mut editor, "last");
    let clip = add_session_clip(&mut editor, track, middle, "held", beat(4));
    let note = add_note(&mut editor, clip, BeatTime::ZERO, beat(1), 60, 0, 100);
    let original_project = editor.project().clone();

    assert!(matches!(
        editor
            .execute(Command::MoveScene {
                scene: first,
                before: Some(third)
            })
            .unwrap(),
        Created::None
    ));
    assert_eq!(scene_order(&editor), vec![middle, first, third, last]);
    editor
        .execute(Command::MoveScene {
            scene: last,
            before: Some(middle),
        })
        .unwrap();
    assert_eq!(scene_order(&editor), vec![last, middle, first, third]);
    editor
        .execute(Command::MoveScene {
            scene: middle,
            before: None,
        })
        .unwrap();
    assert_eq!(scene_order(&editor), vec![last, first, third, middle]);
    let before_self = editor.project().clone();
    let can_undo_before_self = editor.can_undo();
    editor
        .execute(Command::MoveScene {
            scene: third,
            before: Some(third),
        })
        .unwrap();
    assert_eq!(editor.project(), &before_self);
    assert_eq!(editor.can_undo(), can_undo_before_self);

    assert_eq!(
        editor.project().session_cell(track, middle),
        Some(&SessionCellContent::Midi(
            midi_cell(&original_project, track, middle).clone()
        ))
    );
    assert_eq!(editor.project().midi_note(note).unwrap().id(), note);
    assert!(editor.undo());
    assert_eq!(scene_order(&editor), vec![last, middle, first, third]);
    assert!(editor.redo());
    assert_eq!(scene_order(&editor), vec![last, first, third, middle]);

    let stale = add_scene(&mut editor, "stale");
    assert!(editor.undo());
    let current = editor.project().clone();
    assert!(editor.can_redo());
    editor
        .execute(Command::MoveScene {
            scene: first,
            before: Some(first),
        })
        .expect("self move is a no-op");
    assert_eq!(editor.project(), &current);
    assert!(editor.can_redo());
    assert!(editor.execute(Command::DuplicateScene(stale)).is_err());
    assert_eq!(editor.project(), &current);
    assert!(editor.can_redo());
    assert!(
        editor
            .execute(Command::MoveScene {
                scene: stale,
                before: Some(first),
            })
            .is_err()
    );
    assert_eq!(editor.project(), &current);
    assert!(editor.can_redo());
    assert!(
        editor
            .execute(Command::MoveScene {
                scene: first,
                before: Some(stale),
            })
            .is_err()
    );
    assert_eq!(editor.project(), &current);
    assert!(editor.can_redo());
}
