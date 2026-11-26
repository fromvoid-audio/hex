use hex_project::{
    BeatTime, Command, Created, Editor, Project, Tempo, WarpMode, default_audio_warp,
};
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
