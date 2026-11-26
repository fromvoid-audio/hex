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
    let json = crate::encode(editor.project()).unwrap();
    let mut current: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(
        current["version"],
        crate::CAPABILITY_MANIFEST_SCHEMA_VERSION
    );
    let capabilities = current["capabilities"].as_array().unwrap();
    for feature in ["outer_clip_fades", "warp_resampler"] {
        assert!(
            capabilities.contains(&serde_json::json!(feature)),
            "{feature}"
        );
    }
    let edit = &mut current["project"]["tracks"][0]["audio_clips"][0]["edit"];
    assert!(edit["view_end"].as_u64().is_some());
    edit["view_end"] = 0.into();
    assert!(crate::decode(&serde_json::to_vec(&current).unwrap()).is_err());

    let legacy = crate::encode_stacked_legacy(editor.project()).unwrap();
    assert_eq!(crate::decode(&legacy).unwrap(), *editor.project());
    let mut value: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
    // Destination fades wrap the carrier that records the v2 warp resampler.
    assert_eq!(value["version"], crate::OUTER_CLIP_FADE_SCHEMA_VERSION);
    assert_eq!(
        value["base"]["version"],
        crate::WARP_RESAMPLER_SCHEMA_VERSION
    );
    assert_eq!(crate::decode(&json).unwrap(), *editor.project());
    let text = String::from_utf8(json).unwrap();
    assert!(text.contains("complex_pro"));
    // Canonical validation rejects inverted crop data rather than losing transform semantics.
    let edit = &mut value["base"]["audio_clip_state"][0]["audio_edit"];
    assert!(edit.is_object(), "canonical audio edit field is present");
    {
        edit["view_end"] = serde_json::json!(0);
        assert!(crate::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(editor.undo());
    assert!(editor.redo());
    assert_eq!(
        crate::decode(&crate::encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
}

#[test]
fn current_shapes_preserve_events_appearance_names_and_strict_validation() {
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
        for (edge, frame) in [
            (hex_project::TrimEdge::Left, 4000),
            (hex_project::TrimEdge::Right, 44_000),
        ] {
            editor
                .execute(Command::TrimClipFadeEdge { clip, edge, frame })
                .unwrap();
        }
        editor
            .execute(Command::SplitClip {
                clip,
                frame: 24_000,
            })
            .unwrap();
        let bytes = crate::encode(editor.project()).unwrap();
        assert_eq!(crate::decode(&bytes).unwrap(), *editor.project());
        assert!(crate::encode_schema22(editor.project()).is_err());
        let mut current: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            current["version"],
            crate::CAPABILITY_MANIFEST_SCHEMA_VERSION
        );
        let domain = &mut current["project"]["tracks"][0]["audio_clips"][0]["edit"]["domain"];
        assert!(domain["fade"]["bends"][0].is_i64());
        domain["fade"]["bends"][0] = 1001.into();
        assert!(crate::decode(&serde_json::to_vec(&current).unwrap()).is_err());

        // The retained writer still exercises schema 122's envelope and strict field checks.
        let legacy = crate::encode_stacked_legacy(editor.project()).unwrap();
        assert_eq!(crate::decode(&legacy).unwrap(), *editor.project());
        let value: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
        assert_eq!(value["version"], crate::OUTER_CLIP_FADE_SCHEMA_VERSION);
        assert!(
            value["fades"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty())
        );
        assert!(value["base"]["audio_event_state"].is_array());
        for mutation in 0..7 {
            let mut bad = value.clone();
            match mutation {
                0 => {
                    bad.as_object_mut().unwrap().remove("fades");
                }
                1 => bad["fades"][0]["fade"]["curve"]["bends"][0] = 1001.into(),
                2 => bad["fades"][0]["fade"]["curve"]["in_start"] = u64::MAX.into(),
                3 => bad["fades"][0]["clip"] = 999_999.into(),
                4 => {
                    let first = bad["fades"][0].clone();
                    bad["fades"].as_array_mut().unwrap().push(first);
                }
                5 => {
                    bad["base"]
                        .as_object_mut()
                        .unwrap()
                        .remove("audio_event_state");
                }
                _ => bad["fades"] = serde_json::Value::Null,
            }
            assert!(
                crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "mutation {mutation}"
            );
        }
        let raw = std::str::from_utf8(&legacy).unwrap();
        for key in ["fades", "audio_event_state"] {
            let field = format!("\"{key}\":");
            assert_eq!(raw.matches(&field).count(), 1);
            let duplicate = raw.replacen(&field, &format!("{field}[],{field}"), 1);
            assert!(
                crate::decode(duplicate.as_bytes()).is_err(),
                "duplicate {key} cannot be hidden by adaptation"
            );
        }
    }
}
