//! Explicit multi-track removal uses canonical cleanup and a single history entry.
use crate::{BeatTime, Command, Created, Editor, Project, RoutingOwner, TrackId};
fn track(editor: &mut Editor, name: &str) -> TrackId {
    let Created::Track(id) = editor
        .execute(Command::AddMidiTrack { name: name.into() })
        .unwrap()
    else {
        panic!("track")
    };
    id
}
#[test]
fn remove_tracks_preserves_survivors_cleans_references_and_undo_restores_everything() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let a = track(&mut editor, "first");
    let b = track(&mut editor, "second");
    let survivor = track(&mut editor, "survivor");
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: a,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(a),
            destination: survivor,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            source: Some(b),
            destination: survivor,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::CreateSessionMidiClip {
            track: b,
            scene,
            name: "session".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap();
    let before = editor.project().clone();
    assert!(
        !before.track(a).unwrap().devices().is_empty()
            || before.track(a).unwrap().instrument().is_some()
    );
    editor.execute(Command::RemoveTracks(vec![b, a])).unwrap();
    let after = editor.project().clone();
    assert_eq!(after.tracks().len(), 1);
    assert_eq!(after.track(survivor), before.track(survivor));
    assert!(after.audio_feeds().is_empty());
    assert!(after.midi_routes().is_empty());
    assert!(after.session_cells().is_empty());
    assert_eq!(after.scenes(), before.scenes());
    assert_eq!(after.master_gain(), before.master_gain());
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
}
#[test]
fn remove_tracks_invalid_selection_is_atomic_and_preserves_redo() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let a = track(&mut editor, "first");
    let stale = track(&mut editor, "stale");
    assert!(editor.undo());
    let before = editor.project().clone();
    for ids in [
        vec![],
        vec![a, a],
        vec![a, stale],
        vec![a; crate::MAX_TRACKS + 1],
    ] {
        assert!(editor.execute(Command::RemoveTracks(ids)).is_err());
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }
    assert!(editor.redo());
    assert!(editor.project().track(stale).is_some());
}
