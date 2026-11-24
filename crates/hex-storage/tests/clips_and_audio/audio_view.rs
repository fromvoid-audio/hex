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
    // Its musical content renders with the v2 warp resampler.
    assert_eq!(
        original["version"],
        hex_storage::WARP_RESAMPLER_SCHEMA_VERSION
    );
    assert!(original.get("device_assets").is_some());
    assert_eq!(original["tuning"]["enabled"], true);
    assert_eq!(
        hex_storage::decode(&bytes).unwrap().snapshot(),
        e.project().snapshot()
    );
    let mut old = original.clone();
    old["version"] = 31.into();
    old.as_object_mut().unwrap().remove("main_devices");
    // Relabelling a current envelope does not produce an original31 layout.
    assert!(hex_storage::decode(&serde_json::to_vec(&old).unwrap()).is_err());
    for field in ["tuning", "device_assets"] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(hex_storage::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    }
}

#[test]
fn once_consumed_cursor_and_selected_root_offset_convert_below_single_signed_range() {
    use hex_project::{BeatTime, audio_projection_frame};
    let (project, clip) = fixture(AudioViewMode::Once);
    // The retained source/root remains bounded (480k frames). Once may have
    // consumed a much larger cursor and now project only silence.
    let phase = project
        .tempo_map()
        .beat_to_frame(BeatTime::from_raw(u64::MAX))
        .unwrap()
        - 2;
    let mut snapshot = project.snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0]
        .edit
        .as_mut()
        .unwrap()
        .placement
        .as_mut()
        .unwrap()
        .cycle_phase = phase;
    let mut model = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    model
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let edit = model.project().clip(clip).unwrap().audio_edit().unwrap();
    let projection = edit.domain.fade_projection.as_ref().unwrap();
    assert!(projection.origin_beat_raw < -i128::from(u64::MAX));
    assert!(projection.origin_beat_raw >= -2 * i128::from(u64::MAX));
    assert!(audio_projection_frame(&projection.tempo, projection.origin_beat_raw).is_ok());
    let projected = AudioViewProjection::new(
        model.project().clip(clip).unwrap(),
        model.project().tempo_map(),
    )
    .unwrap();
    assert_eq!(
        projected.occurrence_count(),
        0,
        "consumed Once remains silence, not a repeated source"
    );
    let bytes = hex_storage::encode(model.project()).unwrap();
    let reopened = hex_storage::decode(&bytes).unwrap();
    assert_eq!(reopened.snapshot(), model.project().snapshot());
}

#[test]
fn projection_codec_matches_signed_model_endpoints_and_refuses_malformed_origins() {
    use std::sync::Arc;
    let (project, clip) = fixture(AudioViewMode::Once);
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let minimum = -2 * i128::from(u64::MAX);
    let maximum = i128::from(u64::MAX);
    for origin in [minimum, minimum + 1, -maximum - 1, -maximum, 0, maximum] {
        let mut snapshot = editor.project().snapshot();
        let edit = snapshot.tracks[0].audio_clips_mut().unwrap()[0]
            .edit
            .as_mut()
            .unwrap();
        let domain = Arc::make_mut(&mut edit.domain);
        domain.fade_projection.as_mut().unwrap().origin_beat_raw = origin;
        let valid = Project::from_snapshot(snapshot).unwrap();
        let bytes = hex_storage::encode(&valid).unwrap();
        assert_eq!(
            hex_storage::decode(&bytes).unwrap().snapshot(),
            valid.snapshot(),
            "origin={origin}"
        );
    }
    let original: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(editor.project()).unwrap()).unwrap();
    for origin in [
        (minimum - 1).to_string(),
        (maximum + 1).to_string(),
        "170141183460469231731687303715884105728".into(),
        "-170141183460469231731687303715884105729".into(),
        "nan".into(),
        "1.5".into(),
    ] {
        let mut value = original.clone();
        let edit = placement_object(&mut value).unwrap();
        edit["domain"]["fade_projection"]["origin_beat_raw"] = origin.clone().into();
        assert!(
            hex_storage::decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "origin={origin}"
        );
    }
}

#[test]
fn phased_musical_fade_preserves_authored_age_across_tempo_and_current_codec() {
    use hex_project::{BeatTime, Tempo};
    const Q: u64 = 1 << 32;
    for mode in [AudioViewMode::Once, AudioViewMode::Repeat] {
        let (project, clip) = fixture(mode);
        let mut editor = Editor::new(project, 16).unwrap();
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_raw(Q / 4),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let authored = editor.project().tempo_map().clone();
        let fade = ClipFade::new(
            editor.project().clip(clip).unwrap().duration(),
            0,
            48_000,
            0,
        )
        .unwrap();
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        for changed in [false, true] {
            if changed {
                editor
                    .execute(Command::SetTempoPoint {
                        beat: BeatTime::from_raw(Q / 8),
                        tempo: Tempo::new(90_000_000).unwrap(),
                    })
                    .unwrap();
            }
            let bytes = hex_storage::encode(editor.project()).unwrap();
            let loaded = hex_storage::decode(&bytes).unwrap();
            assert_eq!(loaded.snapshot(), editor.project().snapshot());
            let placed = loaded.clip(clip).unwrap();
            let edit = placed.audio_edit().unwrap();
            assert!(edit.placement.unwrap().cycle_phase > 0);
            let author = edit.domain.fade_projection.as_ref().unwrap();
            assert_eq!(author.tempo, authored);
            assert_eq!((author.origin_beat_raw, author.local_start_raw), (0, 0));

            // The clip starts on beat zero. The loaded tempo locates the output
            // sample, while the captured tempo gives the age of its fade.
            let projected = placed
                .projected_outer_fade(loaded.tempo_map())
                .unwrap()
                .unwrap();
            for elapsed in [0, Q / 8, Q / 4, Q / 2, Q, 3 * Q / 2] {
                let beat = BeatTime::from_raw(elapsed);
                let frame = loaded.tempo_map().beat_to_frame(beat).unwrap();
                let age = authored.beat_to_frame(beat).unwrap();
                assert_eq!(
                    projected.domain_position_at_frame(frame).unwrap(),
                    Some(age)
                );
                assert_eq!(projected.gain_at(frame).unwrap(), fade.gain_at(age));
            }
        }
    }
}
