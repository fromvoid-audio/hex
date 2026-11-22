use hex_project::{Command, Created, Editor, Project};
#[test]
fn repeated_relative_edits_use_canonical_state_and_undo_individually() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let (track, clip) = match editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 1000,
        })
        .unwrap()
    {
        Created::Imported { track, clip, .. } => (track, clip),
        _ => panic!("import"),
    };
    for _ in 0..3 {
        editor
            .execute(Command::NudgeClip { clip, frames: 100 })
            .unwrap();
    }
    assert_eq!(editor.project().clip(clip).unwrap().start(), 300);
    editor.undo();
    assert_eq!(editor.project().clip(clip).unwrap().start(), 200);
    for _ in 0..2 {
        editor
            .execute(Command::TrimClipEndBy { clip, frames: -100 })
            .unwrap();
    }
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 800);
    assert_eq!(editor.project().clip(clip).unwrap().source_length(), 800);
    for _ in 0..2 {
        editor
            .execute(Command::GainBy {
                track,
                amount: -0.25,
            })
            .unwrap();
    }
    assert_eq!(editor.project().track(track).unwrap().gain(), 0.5);
    for _ in 0..2 {
        editor.execute(Command::ToggleMute(track)).unwrap();
    }
    assert!(!editor.project().track(track).unwrap().muted());
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::TrimClipEndBy { clip, frames: 1000 })
            .is_err()
    );
    assert!(
        editor
            .execute(Command::GainBy {
                track,
                amount: f32::NAN
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    editor
        .execute(Command::NudgeClip {
            clip,
            frames: i64::MIN,
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().start(), 0);
}
