//! The new overlap semantics use a fresh format; pre50 remains strictly disjoint.
use hex_project::{Command, Created, Editor, Project, TimeRange};

fn fixture() -> (Editor, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "metadata-only.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("fixture import")
    };
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase { clip, phase: 16 })
        .unwrap();
    editor
        .execute(Command::CopyClipTo {
            clip,
            track,
            start: 96_000,
        })
        .unwrap();
    (editor, track)
}
#[test]
fn current_crossfade_roundtrip_and_historical_disjoint_migration_are_complete() {
    let (mut editor, track) = fixture();
    let before = editor.project().snapshot();
    let disjoint = hex_storage::encode(editor.project()).unwrap();
    // Schema50 changes only overlap admission; the retained frozen49 writer
    // has the identical field layout. This disjoint document remains valid49.
    let mut old: serde_json::Value = serde_json::from_slice(&disjoint).unwrap();
    old["version"] = 49.into();
    let bytes = serde_json::to_vec(&old).unwrap();
    let saved_bytes = bytes.clone();
    let migrated = hex_storage::decode(&bytes).unwrap();
    assert_eq!(migrated.snapshot(), before);
    assert_eq!(bytes, saved_bytes);
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(&migrated).unwrap())
            .unwrap()
            .snapshot(),
        before
    );
    editor
        .execute(Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 92_000, 100_000).unwrap(),
        })
        .unwrap();
    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["version"], 50);
    assert_eq!(hex_storage::decode(&encoded).unwrap().snapshot(), after);
    // Relabeling cannot smuggle the newly valid overlap into an old schema.
    value["version"] = 49.into();
    assert!(
        hex_storage::decode(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .to_string()
            .contains("older schema")
    );
    assert!(hex_storage::encode_schema22(editor.project()).is_err());
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}
