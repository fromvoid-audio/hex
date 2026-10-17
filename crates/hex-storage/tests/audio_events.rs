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
    editor
        .execute(Command::RenameClip {
            clip,
            name: "edited audio".into(),
        })
        .unwrap();
    editor
        .execute(Command::SetTrackColor {
            track: editor.project().tracks()[0].id(),
            color: hex_project::ElementColor::Cyan,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFlavor {
            clip,
            flavor: hex_project::ClipFlavor::Accent,
        })
        .unwrap();
    let bytes = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap(), *editor.project());
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], 30);
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
    // Both pre-integration schema27 lineages remain strict and distinguishable.
    let mut appearance27 = value.clone();
    appearance27.as_object_mut().unwrap().remove("tuning");
    appearance27.as_object_mut().unwrap().remove("fade_shapes");
    appearance27["version"] = 27.into();
    appearance27
        .as_object_mut()
        .unwrap()
        .remove("audio_event_state");
    let migrated = hex_storage::decode(&serde_json::to_vec(&appearance27).unwrap()).unwrap();
    assert_eq!(
        migrated.tracks()[0].color(),
        hex_project::ElementColor::Cyan
    );
    assert_eq!(
        migrated.clip_flavor(clip),
        Some(hex_project::ClipFlavor::Accent)
    );
    assert_eq!(migrated.clip(clip).unwrap().name(), "edited audio");
    assert_eq!(migrated.clip(clip).unwrap().audio_events().events.len(), 1);
    let mut events27 = value.clone();
    events27.as_object_mut().unwrap().remove("fade_shapes");
    events27["version"] = 27.into();
    events27.as_object_mut().unwrap().remove("tuning");
    events27.as_object_mut().unwrap().remove("track_colors");
    events27.as_object_mut().unwrap().remove("accented_clips");
    let migrated = hex_storage::decode(&serde_json::to_vec(&events27).unwrap()).unwrap();
    assert_eq!(
        migrated.clip(clip).unwrap().audio_events(),
        editor.project().clip(clip).unwrap().audio_events()
    );
    assert_eq!(migrated.clip(clip).unwrap().name(), "edited audio");
    assert_eq!(
        migrated.tracks()[0].color(),
        hex_project::ElementColor::Gray
    );
    assert_eq!(
        migrated.clip_flavor(clip),
        Some(hex_project::ClipFlavor::Muted)
    );
    let mut mixed27 = value.clone();
    mixed27.as_object_mut().unwrap().remove("fade_shapes");
    mixed27["version"] = 27.into();
    assert!(hex_storage::decode(&serde_json::to_vec(&mixed27).unwrap()).is_err());
    let original = String::from_utf8(bytes).unwrap();
    let duplicate = format!("{{\"audio_event_state\":[],{}", &original[1..]);
    assert!(hex_storage::decode(duplicate.as_bytes()).is_err());
    // A genuine schema26 document still migrates to one implicit event.
    let mut legacy = value;
    legacy.as_object_mut().unwrap().remove("tuning");
    legacy.as_object_mut().unwrap().remove("fade_shapes");
    legacy["version"] = 26.into();
    legacy.as_object_mut().unwrap().remove("audio_event_state");
    legacy.as_object_mut().unwrap().remove("track_colors");
    legacy.as_object_mut().unwrap().remove("accented_clips");
    let migrated = hex_storage::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(migrated.clip(clip).unwrap().audio_events().events.len(), 1);
}

#[test]
fn event_serialization_respects_the_existing_document_byte_limit() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48000,
            frames: 2048,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    let mut root = editor.project().clip(clip).unwrap().audio_event_root();
    std::sync::Arc::make_mut(&mut root.domain).events =
        Some(std::sync::Arc::new(hex_project::AudioEvents {
            next_id: 1025,
            events: (0..1024)
                .map(|i| hex_project::AudioEvent {
                    id: i + 1,
                    start: i * 2,
                    end: i * 2 + 1,
                    source_start: i * 2,
                })
                .collect(),
        }));
    let mut snapshot = editor.project().snapshot();
    let hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } =
        &mut snapshot.tracks[0].content
    else {
        panic!("hybrid")
    };
    let mut template = audio_clips[0].clone();
    template.edit = Some(root);
    *audio_clips = (0..256)
        .map(|i| {
            let mut clip = template.clone();
            clip.id = 1000 + i;
            clip.start = i * 2048;
            clip
        })
        .collect();
    snapshot.next_id = 2000;
    let project = Project::from_snapshot(snapshot).unwrap();
    assert!(matches!(
        hex_storage::encode(&project),
        Err(hex_storage::StorageError::Invalid(
            "project document exceeds byte limit"
        ))
    ));
}
