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
    // Ordinary processed clips also require an explicit null, rather than a
    // serde Option default silently accepting an older incomplete document.
    let mut snapshot = project.snapshot();
    let clip = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    clip.edit.as_mut().unwrap().placement = None;
    clip.duration = 24_000;
    clip.fade = ClipFade::neutral(24_000).unwrap();
    let ordinary = Project::from_snapshot(snapshot).unwrap();
    let mut json: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(&ordinary).unwrap()).unwrap();
    let object = placement_object(&mut json).unwrap();
    assert!(object["placement"].is_null());
    object.remove("placement");
    assert!(hex_storage::decode(&serde_json::to_vec(&json).unwrap()).is_err());
}

#[test]
fn exhausted_once_consumed_cursor_and_signed_boundary_roundtrip_exactly() {
    for musical in [false, true] {
        let (project, clip) = fixture(AudioViewMode::Once);
        let mut s = project.snapshot();
        s.tracks[0].audio_clips_mut().unwrap()[0]
            .edit
            .as_mut()
            .unwrap()
            .placement
            .as_mut()
            .unwrap()
            .cycle_phase = 60_000;
        let mut e = Editor::new(Project::from_snapshot(s).unwrap(), 8).unwrap();
        if musical {
            e.execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        }
        let bytes = hex_storage::encode(e.project()).unwrap();
        let decoded = hex_storage::decode(&bytes).unwrap();
        assert_eq!(decoded.snapshot(), e.project().snapshot());
        let p = AudioViewProjection::new(decoded.clip(clip).unwrap(), decoded.tempo_map()).unwrap();
        assert_eq!(
            (p.logical_first_boundary(), p.occurrence_count()),
            (-36_000, 0)
        );
        assert!(p.occurrence(0).is_err());
    }
}

#[test]
fn current32_placement_keeps_required_tuning_manifest_and_refuses_old31() {
    use hex_tuning::{Audition, ProjectTuning, Scale, Waveform};
    let (project, _) = fixture(AudioViewMode::Once);
    let mut e = Editor::new(project, 8).unwrap();
    e.execute(Command::SetTuning(
        ProjectTuning::new(
            true,
            Scale::new(7, *Scale::default().cents(), 57, 432.0, 3, 1300.0).unwrap(),
            Audition::new(Waveform::Saw, 0.7, 15.0, 550.0).unwrap(),
        )
        .unwrap(),
    ))
    .unwrap();
    let bytes = hex_storage::encode(e.project()).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(original["version"], 32);
    assert!(original.get("device_assets").is_some());
    assert_eq!(original["tuning"]["enabled"], true);
    assert_eq!(
        hex_storage::decode(&bytes).unwrap().snapshot(),
        e.project().snapshot()
    );
    let mut old = original.clone();
    old["version"] = 31.into();
    let error = hex_storage::decode(&serde_json::to_vec(&old).unwrap()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported alpha project format 31")
    );
    for field in ["tuning", "device_assets"] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(hex_storage::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    }
}
