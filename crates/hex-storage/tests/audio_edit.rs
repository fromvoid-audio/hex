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
    // Warped content renders with the v2 warp resampler.
    assert_eq!(value["version"], hex_storage::WARP_RESAMPLER_SCHEMA_VERSION);
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
        assert_eq!(value["version"], hex_storage::CURRENT_SCHEMA_VERSION);
        let raw = String::from_utf8(bytes.clone()).unwrap();
        for key in ["fade_shapes", "audio_event_state"] {
            let duplicate = format!("{{\"{key}\":[],{}", &raw[1..]);
            assert!(
                hex_storage::decode(duplicate.as_bytes()).is_err(),
                "duplicate {key} cannot be hidden by adaptation"
            );
        }
    }
}

#[test]
fn frozen_tempo_map_roundtrips_and_rejects_malformed_current_timing() {
    use hex_project::{AudioClipTiming, AudioEventEdit};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "frozen".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split {
                at: BeatTime::ONE.raw() / 2,
            },
        })
        .unwrap();
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(3).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap();
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(240_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 36_000);
    let AudioClipTiming::Frozen { tempo, .. } = &editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .timing
    else {
        panic!("frozen")
    };
    assert_eq!(tempo.points().len(), 2);
    assert_eq!(editor.project().tempo_map().points().len(), 1);
    let bytes = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap(), *editor.project());
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for mutation in 0..11 {
        let mut bad = value.clone();
        let timing = &mut bad["audio_clip_state"][0]["audio_edit"]["timing"];
        match mutation {
            0 => timing["sample_rate"] = 44_100.into(),
            1 => timing["reference_start_raw"] = i128::MAX.to_string().into(),
            2 => timing["reference_start_raw"] = "not a number".into(),
            3 => timing["tempo_points"] = serde_json::json!([]),
            4 => timing["tempo_points"][0]["beat_raw"] = 1.into(),
            5 => timing["tempo_points"][1]["beat_raw"] = 0.into(),
            6 => timing["kind"] = "unknown".into(),
            7 => timing["unexpected"] = true.into(),
            8 => bad["audio_clip_state"][0]["audio_edit"]
                .as_object_mut()
                .unwrap()
                .remove("timing")
                .map(|_| ())
                .unwrap(),
            9 => bad["audio_clip_state"][0]["audio_edit"]["domain"]["warp_enabled"] = true.into(),
            _ => {
                timing["tempo_points"] = serde_json::json!(vec![
                    serde_json::json!({"beat_raw": 0, "micro_bpm": 120_000_000});
                    hex_project::MAX_TEMPO_POINTS + 1
                ])
            }
        }
        assert!(
            hex_storage::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "mutation {mutation}"
        );
    }
    let mut old = value;
    old["version"] = 35.into();
    assert!(hex_storage::decode(&serde_json::to_vec(&old).unwrap()).is_err());
}
