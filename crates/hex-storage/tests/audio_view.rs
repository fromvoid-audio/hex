use hex_project::{
    AudioDomainClock, AudioViewMode, AudioViewPlacement, AudioViewProjection, ClipFade, Command,
    Created, Editor, Project,
};

fn fixture(mode: AudioViewMode) -> (Project, hex_project::ClipId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "processed crop".into(),
            path: "retained.wav".into(),
            sample_rate: 48_000,
            frames: 480_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    e.execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let mut s = e.project().snapshot();
    let c = &mut s.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = c.edit.as_mut().unwrap();
    edit.view_start = 12_000;
    edit.view_end = 36_000;
    edit.placement = Some(AudioViewPlacement {
        mode,
        outer_length: AudioDomainClock::Frames(48_000),
        cycle_phase: 6_000,
    });
    c.duration = 48_000;
    c.fade = ClipFade::neutral(c.duration).unwrap();
    (Project::from_snapshot(s).unwrap(), clip)
}

#[test]
fn current_format_audio_placement_roundtrips_once_and_repeat_without_saved_origin() {
    for mode in [AudioViewMode::Once, AudioViewMode::Repeat] {
        for musical in [false, true] {
            let (mut project, clip) = fixture(mode);
            if musical {
                let mut editor = Editor::new(project, 8).unwrap();
                editor
                    .execute(Command::SetAudioClipMusical {
                        clip,
                        enabled: true,
                    })
                    .unwrap();
                project = editor.project().clone();
            }
            let bytes = hex_storage::encode(&project).unwrap();
            let decoded = hex_storage::decode(&bytes).unwrap();
            assert_eq!(decoded.snapshot(), project.snapshot());
            let a =
                AudioViewProjection::new(project.clip(clip).unwrap(), project.tempo_map()).unwrap();
            let b =
                AudioViewProjection::new(decoded.clip(clip).unwrap(), decoded.tempo_map()).unwrap();
            assert_eq!(a.occurrence_count(), b.occurrence_count());
            for index in 0..a.occurrence_count() {
                assert_eq!(a.occurrence(index).unwrap(), b.occurrence(index).unwrap());
            }
            assert!(!String::from_utf8(bytes).unwrap().contains("root_origin"));
            assert_eq!(
                decoded
                    .clip(clip)
                    .unwrap()
                    .audio_edit()
                    .unwrap()
                    .domain
                    .pitch_cents,
                137
            );
        }
    }
}

fn placement_object(
    value: &mut serde_json::Value,
) -> Option<&mut serde_json::Map<String, serde_json::Value>> {
    match value {
        serde_json::Value::Object(map) => {
            if map.contains_key("placement") {
                Some(map)
            } else {
                map.values_mut().find_map(placement_object)
            }
        }
        serde_json::Value::Array(values) => values.iter_mut().find_map(placement_object),
        _ => None,
    }
}

#[test]
fn current_format_rejects_missing_placement_phase_clock_and_unknown_field() {
    let (project, _) = fixture(AudioViewMode::Repeat);
    let original = hex_storage::encode(&project).unwrap();
    for case in 0..4 {
        let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let object = placement_object(&mut json).unwrap();
        match case {
            0 => {
                object.remove("placement");
            }
            1 => {
                object["placement"]["cycle_phase"] = serde_json::json!(24_000);
            }
            2 => {
                object["placement"]["outer_length"] =
                    serde_json::json!({"kind":"beats","length_raw":1_u64<<32});
            }
            _ => {
                object["placement"]["root_origin"] = serde_json::json!(-18_000);
            }
        }
        assert!(
            hex_storage::decode(&serde_json::to_vec(&json).unwrap()).is_err(),
            "malformed case{case}"
        );
    }
}
