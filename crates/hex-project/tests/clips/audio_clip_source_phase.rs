use hex_project::{Command, Created, Editor, Project};
use std::path::PathBuf;

fn looping_clip() -> (Editor, hex_project::ClipId) {
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
        .expect("clip")
    else {
        panic!("clip")
    };
    (editor, clip)
}

#[test]
fn looping_source_phase_is_an_undoable_validated_edit() {
    let (mut editor, clip) = looping_clip();
    editor
        .execute(Command::SetClipSourcePhase { clip, phase: 5 })
        .expect("phase edit");
    assert_eq!(editor.project().clip(clip).expect("clip").source_phase(), 5);
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).expect("clip").source_phase(), 0);
    assert!(editor.redo());
    assert_eq!(editor.project().clip(clip).expect("clip").source_phase(), 5);
}

#[test]
fn source_phase_rejects_interval_end_and_non_looping_clip_atomically() {
    let (mut editor, clip) = looping_clip();
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetClipSourcePhase { clip, phase: 8 })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_undo());

    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: false,
        })
        .expect("disable loop");
    let one_shot = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetClipSourcePhase { clip, phase: 1 })
            .is_err()
    );
    assert_eq!(editor.project(), &one_shot);
    assert!(editor.can_undo());
}
