use hex_project::{BeatTime, Command, Created, Editor, Project};
use hex_storage::{decode, encode};

fn fixture() -> (
    Editor,
    hex_project::TrackId,
    hex_project::ClipId,
    hex_project::ClipId,
) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported {
        track, clip: audio, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "track".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(8).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi")
    };
    (editor, track, audio, midi)
}

#[test]
fn names_preserve_case_history_copies_and_storage_in_all_clip_domains() {
    let (mut editor, _track, audio, midi) = fixture();
    for id in [audio, midi] {
        let before = editor.project().snapshot();
        for invalid in [
            "".into(),
            " \t".into(),
            "line\nbreak".into(),
            "x".repeat(1025),
        ] {
            assert!(
                editor
                    .execute(Command::RenameClip {
                        clip: id,
                        name: invalid
                    })
                    .is_err()
            );
            assert_eq!(editor.project().snapshot(), before);
        }
        editor
            .execute(Command::RenameClip {
                clip: id,
                name: "Verse İ 🎹".into(),
            })
            .unwrap();
        assert_eq!(editor.project().clip_name(id), Some("Verse İ 🎹"));
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: midi,
            scene,
        })
        .unwrap()
    else {
        panic!("session")
    };
    editor
        .execute(Command::RenameClip {
            clip: session,
            name: "Session MIDI".into(),
        })
        .unwrap();
    assert_eq!(editor.project().clip_name(midi), Some("Verse İ 🎹"));
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session_audio) = editor
        .execute(Command::CopyArrangementAudioClipToSession {
            source: audio,
            scene,
        })
        .unwrap()
    else {
        panic!("audio session")
    };
    assert_eq!(
        editor.project().clip_name(session_audio),
        Some("Verse İ 🎹")
    );
    editor
        .execute(Command::RenameClip {
            clip: session_audio,
            name: "Session Audio".into(),
        })
        .unwrap();
    let Created::Clip(right) = editor
        .execute(Command::SplitClip {
            clip: audio,
            frame: 48_000,
        })
        .unwrap()
    else {
        panic!("split")
    };
    assert_eq!(editor.project().clip_name(right), Some("Verse İ 🎹"));
    let Created::Clip(right) = editor
        .execute(Command::SplitMidiClip {
            clip: midi,
            beat: BeatTime::from_raw(BeatTime::ONE.raw() * 8 + BeatTime::ONE.raw() / 2),
        })
        .unwrap()
    else {
        panic!("midi split")
    };
    assert_eq!(editor.project().clip_name(right), Some("Verse İ 🎹"));
    let saved = encode(editor.project()).unwrap();
    assert_eq!(
        decode(&saved).unwrap().snapshot(),
        editor.project().snapshot()
    );
    assert!(hex_storage::encode_schema22(editor.project()).is_err());
}

#[test]
fn schema26_requires_exact_valid_arrangement_name_inventory_and_migrates25() {
    let (editor, _, audio, _) = fixture();
    let value: serde_json::Value =
        serde_json::from_slice(&encode(editor.project()).unwrap()).unwrap();
    let mut unchanged = Editor::new(editor.project().clone(), 8).unwrap();
    unchanged
        .execute(Command::RenameClip {
            clip: audio,
            name: "clip".into(),
        })
        .unwrap();
    assert!(
        !unchanged.can_undo(),
        "unchanged label must not add history"
    );
    for invalid in ["".into(), "x".repeat(1025), "a\nb".into()] {
        let mut doc = value.clone();
        doc["clip_names"][0]["name"] = invalid.into();
        assert!(decode(&serde_json::to_vec(&doc).unwrap()).is_err());
    }
    let mut unknown = value.clone();
    unknown["clip_names"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"clip":9999,"name":"extra"}));
    assert!(decode(&serde_json::to_vec(&unknown).unwrap()).is_err());

    for bad in [
        serde_json::json!(null),
        serde_json::json!([]),
        serde_json::json!([{"clip": audio.value(), "name": ""}]),
        serde_json::json!([{"clip": audio.value(), "name": "a"},{"clip": audio.value(), "name": "a"}]),
        serde_json::json!([{"clip":9999,"name":"unknown"}]),
    ] {
        let mut doc = value.clone();
        doc["clip_names"] = bad;
        assert!(decode(&serde_json::to_vec(&doc).unwrap()).is_err());
    }
    let mut legacy = value.clone();
    legacy["version"] = 25.into();
    legacy.as_object_mut().unwrap().remove("track_colors");
    legacy.as_object_mut().unwrap().remove("accented_clips");
    legacy.as_object_mut().unwrap().remove("clip_names");
    let migrated = decode(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(migrated.clip_name(audio), Some("clip"));
    assert_eq!(migrated.snapshot(), editor.project().snapshot());
    let mut missing = value;
    missing.as_object_mut().unwrap().remove("clip_names");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
}
