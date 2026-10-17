use hex_project::{
    BeatTime, Command, Created, Editor, Project, SourceGainPoint, Tempo, WarpMarker, WarpMode,
    default_audio_warp,
};
#[test]
fn musical_root_crop_warp_envelope_and_frozen_fade_roundtrip_and_malformed_bounds() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "original".into(),
            path: "sound.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 1_337,
            fade_out: 2_911,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let mut settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::ComplexPro {
            formants: 72,
            envelope: 192,
        },
    )
    .unwrap();
    settings.markers.insert(
        1,
        WarpMarker {
            source_frame: 13_000,
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
        },
    );
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 0,
                },
                SourceGainPoint {
                    frame: 17_193,
                    gain_q16: 81_920,
                },
                SourceGainPoint {
                    frame: 48_000,
                    gain_q16: 65_536,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SplitClip {
            clip,
            frame: 17_123,
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let json = hex_storage::encode(editor.project()).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(value["version"], 29);
    assert_eq!(hex_storage::decode(&json).unwrap(), *editor.project());
    let text = String::from_utf8(json).unwrap();
    assert!(text.contains("complex_pro"));
    // Canonical validation rejects inverted crop data rather than losing transform semantics.
    let edit = &mut value["audio_clip_state"][0]["audio_edit"];
    assert!(edit.is_object(), "canonical audio edit field is present");
    {
        edit["view_end"] = serde_json::json!(0);
        assert!(hex_storage::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(editor.undo());
    assert!(editor.redo());
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
}

#[test]
fn schema29_shapes_preserve_events_appearance_names_and_strict_migrations() {
    use hex_project::{AudioEventEdit, ClipFade, ClipFlavor, ElementColor};
    for musical in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Imported { clip, track, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "fade".into(),
                path: "sound.wav".into(),
                sample_rate: 48_000,
                frames: 48_000,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::RenameClip {
                clip,
                name: "shaped name".into(),
            })
            .unwrap();
        editor
            .execute(Command::SetTrackColor {
                track,
                color: ElementColor::Cyan,
            })
            .unwrap();
        editor
            .execute(Command::SetClipFlavor {
                clip,
                flavor: ClipFlavor::Accent,
            })
            .unwrap();
        if musical {
            editor
                .execute(Command::SetAudioClipMusical {
                    clip,
                    enabled: true,
                })
                .unwrap();
        }
        let root_length = editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_event_root()
            .domain
            .clock
            .length();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split {
                    at: root_length / 3,
                },
            })
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Move {
                    event: 1,
                    start: root_length / 6,
                },
            })
            .unwrap();
        let fade = ClipFade::new(48_000, 0, 12_000, 10_000)
            .unwrap()
            .with_shape(1000, 2000, 750, -650)
            .unwrap();
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        editor
            .execute(Command::SplitClip {
                clip,
                frame: 24_000,
            })
            .unwrap();
        let bytes = hex_storage::encode(editor.project()).unwrap();
        assert_eq!(hex_storage::decode(&bytes).unwrap(), *editor.project());
        assert!(hex_storage::encode_schema22(editor.project()).is_err());
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for mutation in [0, 1, 2, 3, 4, 5, 6] {
            let mut bad = value.clone();
            match mutation {
                0 => {
                    bad.as_object_mut().unwrap().remove("fade_shapes");
                }
                1 => bad["fade_shapes"][0]["in_curve"] = 1001.into(),
                2 => bad["fade_shapes"][0]["in_start"] = u64::MAX.into(),
                3 => bad["fade_shapes"][0]["clip"] = 999_999.into(),
                5 => {
                    bad.as_object_mut().unwrap().remove("audio_event_state");
                }
                6 => {
                    bad["fade_shapes"] = serde_json::Value::Null;
                }
                _ => {
                    let first = bad["fade_shapes"][0].clone();
                    bad["fade_shapes"].as_array_mut().unwrap().push(first);
                }
            }
            assert!(
                hex_storage::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "mutation {mutation}"
            );
        }
        assert_eq!(value["version"], 29);
        let raw = String::from_utf8(bytes.clone()).unwrap();
        for key in ["fade_shapes", "audio_event_state"] {
            let duplicate = format!("{{\"{key}\":[],{}", &raw[1..]);
            assert!(
                hex_storage::decode(duplicate.as_bytes()).is_err(),
                "duplicate {key} cannot be hidden by adaptation"
            );
        }
        let mut events28 = value.clone();
        events28["version"] = 28.into();
        assert!(
            hex_storage::decode(&serde_json::to_vec(&events28).unwrap()).is_err(),
            "frozen 28 refuses fade shapes"
        );
        events28.as_object_mut().unwrap().remove("fade_shapes");
        let migrated28 = hex_storage::decode(&serde_json::to_vec(&events28).unwrap()).unwrap();
        assert_eq!(
            migrated28.clip(clip).unwrap().audio_events(),
            editor.project().clip(clip).unwrap().audio_events()
        );
        assert_eq!(migrated28.track(track).unwrap().color(), ElementColor::Cyan);
        assert_eq!(migrated28.clip_flavor(clip), Some(ClipFlavor::Accent));
        assert_eq!(migrated28.clip_name(clip), Some("shaped name"));
        let mut appearance = value.clone();
        appearance["version"] = 27.into();
        assert!(
            hex_storage::decode(&serde_json::to_vec(&appearance).unwrap()).is_err(),
            "frozen 27 refuses shape fields"
        );
        appearance.as_object_mut().unwrap().remove("fade_shapes");
        appearance
            .as_object_mut()
            .unwrap()
            .remove("audio_event_state");
        let migrated27 = hex_storage::decode(&serde_json::to_vec(&appearance).unwrap()).unwrap();
        assert_eq!(migrated27.track(track).unwrap().color(), ElementColor::Cyan);
        assert_eq!(migrated27.clip_flavor(clip), Some(ClipFlavor::Accent));
        assert_eq!(migrated27.clip_name(clip), Some("shaped name"));
        assert!(
            migrated27
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .all(|c| !c.retained_fade().has_shape())
        );
        assert_eq!(
            hex_storage::decode(&hex_storage::encode(&migrated27).unwrap()).unwrap(),
            migrated27
        );
        let mut legacy = value;
        legacy["version"] = 26.into();
        assert!(
            hex_storage::decode(&serde_json::to_vec(&legacy).unwrap()).is_err(),
            "26 must refuse the new shape field"
        );
        legacy.as_object_mut().unwrap().remove("fade_shapes");
        legacy.as_object_mut().unwrap().remove("audio_event_state");
        legacy.as_object_mut().unwrap().remove("track_colors");
        legacy.as_object_mut().unwrap().remove("accented_clips");
        let migrated = hex_storage::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(migrated.clip_name(clip), Some("shaped name"));
        assert!(
            migrated
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .all(|c| !c.retained_fade().has_shape())
        );
    }
}
