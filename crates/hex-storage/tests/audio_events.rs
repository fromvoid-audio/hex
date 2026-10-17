use hex_project::{AudioEventEdit, Command, Created, Editor, Project};
#[test]
fn events_roundtrip_and_strict_complete_overlay_rejects_loss_and_bad_references() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 30 },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: 50,
            },
        })
        .unwrap();
    let bytes = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap(), *editor.project());
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], 26);
    for mutate in 0..4 {
        let mut bad = value.clone();
        match mutate {
            0 => {
                bad["audio_event_state"] = serde_json::json!([]);
            }
            1 => {
                bad["audio_event_state"][0]["clip"] = 999999.into();
            }
            2 => {
                bad["audio_event_state"][0]["events"][0]["source_start"] = u64::MAX.into();
            }
            _ => {
                bad["audio_event_state"][0]["unexpected"] = true.into();
            }
        }
        assert!(hex_storage::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    // A genuine schema25 document still migrates to one implicit event.
    let mut legacy = value;
    legacy["version"] = 25.into();
    legacy.as_object_mut().unwrap().remove("audio_event_state");
    let migrated = hex_storage::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(migrated.clip(clip).unwrap().audio_events().events.len(), 1);
}
