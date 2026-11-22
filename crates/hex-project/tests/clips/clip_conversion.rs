//! Unsupported transcription must never replace a source or consume history.
use hex_project::{Command, Created, Editor, Project};

#[test]
fn audio_to_midi_refuses_arrangement_session_and_missing_ids_without_history_changes() {
    let mut setup = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, clip, .. } = setup
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Scene(scene) = setup
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(cell) = setup
        .execute(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("cell")
    };
    let Created::Clip(missing) = setup.execute(Command::DuplicateClip(clip)).unwrap() else {
        panic!("duplicate")
    };
    setup.execute(Command::RemoveClip(missing)).unwrap();
    let mut editor = Editor::new(setup.project().clone(), 16).unwrap();
    let initial = editor.project().clone();
    for id in [clip, cell, missing] {
        let error = editor
            .execute(Command::ConvertAudioClipToMidi(id))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("audio-to-midi conversion is unavailable")
        );
        assert_eq!(editor.project(), &initial);
        assert!(!editor.can_undo() && !editor.can_redo());
    }
    editor.execute(Command::Gain { track, gain: 0.5 }).unwrap();
    let changed = editor.project().clone();
    assert!(editor.undo());
    let before = editor.project().clone();
    for id in [clip, cell, missing] {
        assert!(editor.execute(Command::ConvertAudioClipToMidi(id)).is_err());
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo() && editor.can_redo());
    }
    assert!(editor.redo());
    assert_eq!(editor.project(), &changed);
}
