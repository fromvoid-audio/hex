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
    assert_eq!(value["version"], 27);
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
fn schema27_shapes_roundtrip_plain_and_musical_split_domains_and_migrate26() {
    use hex_project::ClipFade;
    for musical in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Imported { clip, .. } = editor
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
        if musical {
            editor
                .execute(Command::SetAudioClipMusical {
                    clip,
                    enabled: true,
                })
                .unwrap();
        }
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
        for mutation in [0, 1, 2, 3, 4] {
            let mut bad = value.clone();
            match mutation {
                0 => {
                    bad.as_object_mut().unwrap().remove("fade_shapes");
                }
                1 => bad["fade_shapes"][0]["in_curve"] = 1001.into(),
                2 => bad["fade_shapes"][0]["in_start"] = u64::MAX.into(),
                3 => bad["fade_shapes"][0]["clip"] = 999_999.into(),
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
        let mut legacy = value;
        legacy["version"] = 26.into();
        assert!(
            hex_storage::decode(&serde_json::to_vec(&legacy).unwrap()).is_err(),
            "26 must refuse the new shape field"
        );
        legacy.as_object_mut().unwrap().remove("fade_shapes");
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
