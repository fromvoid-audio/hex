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
    assert_eq!(value["version"], 25);
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
