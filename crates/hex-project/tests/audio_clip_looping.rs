use hex_project::{Command, Created, Editor, Project};
use std::path::PathBuf;

#[test]
fn disabling_audio_loop_resets_phase_and_undo_redo_restore_exact_state() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 12).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "hybrid".into(),
        })
        .expect("track")
    else {
        panic!("track")
    };
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: PathBuf::from("sample.wav"),
            sample_rate: 48_000,
            frames: 64,
        })
        .expect("asset")
    else {
        panic!("asset")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 20,
            source_offset: 4,
            source_length: 8,
            looping: true,
        })
        .expect("looping clip")
    else {
        panic!("clip")
    };
    let Created::Clip(right) = editor
        .execute(Command::SplitClip { clip, frame: 3 })
        .expect("split")
    else {
        panic!("right clip")
    };
    let before = editor.project().clip(right).expect("right before").clone();
    assert!(before.looping());
    assert_eq!(before.source_phase(), 3);

    editor
        .execute(Command::SetClipLooping {
            clip: right,
            looping: false,
        })
        .expect("disable loop");
    let disabled = editor
        .project()
        .clip(right)
        .expect("right disabled")
        .clone();
    assert!(!disabled.looping());
    assert_eq!(disabled.source_phase(), 0);
    assert_eq!(disabled.source_offset(), before.source_offset());
    assert_eq!(disabled.source_length(), before.source_length());
    assert!(editor.undo());
    assert_eq!(editor.project().clip(right), Some(&before));
    assert!(editor.redo());
    assert_eq!(editor.project().clip(right), Some(&disabled));

    let restored = Project::from_snapshot(editor.project().snapshot()).expect("roundtrip");
    assert_eq!(restored.clip(right), Some(&disabled));
}

#[test]
fn unchanged_or_stale_loop_edits_preserve_project_and_redo() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("track")
    else {
        panic!("track")
    };
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: PathBuf::from("sample.wav"),
            sample_rate: 48_000,
            frames: 8,
        })
        .expect("asset")
    else {
        panic!("asset")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 4,
            source_offset: 0,
            source_length: 4,
            looping: false,
        })
        .expect("clip")
    else {
        panic!("clip")
    };
    editor
        .execute(Command::Gain { track, gain: 0.5 })
        .expect("history edit");
    assert!(editor.undo());
    let before = editor.project().clone();
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: false,
        })
        .expect("unchanged loop state");
    assert!(editor.can_redo());
    assert_eq!(editor.project(), &before);

    let Created::Clip(stale) = editor
        .execute(Command::DuplicateClip(clip))
        .expect("create stale id")
    else {
        panic!("duplicate")
    };
    assert!(editor.undo());
    assert!(
        editor
            .execute(Command::SetClipLooping {
                clip: stale,
                looping: true,
            })
            .is_err()
    );
    assert_eq!(editor.project().tracks(), before.tracks());
    assert_eq!(
        editor.project().assets().cloned().collect::<Vec<_>>(),
        before.assets().cloned().collect::<Vec<_>>()
    );
    assert!(editor.can_redo());
}
