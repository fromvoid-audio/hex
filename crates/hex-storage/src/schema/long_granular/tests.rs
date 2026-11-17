use super::*;
use hex_project::{Command, Created, Editor, WarpMode};
fn fixture() -> (Project, Project) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    for (index, mode) in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ]
    .into_iter()
    .enumerate()
    {
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: format!("long-{index}"),
                path: format!("long-{index}.wav").into(),
                sample_rate: 48_000,
                frames: 20_000_000,
            })
            .unwrap()
        else {
            panic!("import fixture")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let warp = hex_project::default_audio_warp(
            editor.project().clip(clip).unwrap(),
            editor.project().tempo_map(),
            mode,
        )
        .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 300 })
            .unwrap();
        let start = editor.project().clip(clip).unwrap().start();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                frame: start + 1000,
            })
            .unwrap();
    }
    let expected = editor.project().clone();
    let mut snapshot = expected.snapshot();
    for track in &mut snapshot.tracks {
        if let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content {
            for clip in audio_clips {
                clip.edit = None;
            }
        }
    }
    (Project::from_snapshot(snapshot).unwrap(), expected)
}
#[test]
fn complete_long_roots_round_trip_over_valid_prior_document_and_warn_before_overwrite() {
    let (prior, project) = fixture();
    let old = crate::encode(&prior).unwrap();
    let bytes = crate::encode(&project).unwrap();
    assert_eq!(
        crate::schema_version_for(&project),
        LONG_GRANULAR_SCHEMA_VERSION
    );
    let dto: Document<'_> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(dto.base.get().as_bytes(), old);
    assert_eq!(crate::decode(&old).unwrap(), prior);
    assert_eq!(crate::decode(&bytes).unwrap(), project);
    assert_eq!(
        crate::encode(&crate::decode(&bytes).unwrap()).unwrap(),
        bytes
    );
    let token = crate::FileToken::from_validated_bytes(old.into());
    assert!(token.requires_save_confirmation(&project).unwrap());
    let token = crate::FileToken::from_validated_bytes(bytes.into());
    assert!(!token.requires_save_confirmation(&project).unwrap());
}
#[test]
fn malformed_unknown_and_old_schema_long_states_are_refused() {
    let (_, project) = fixture();
    let original: serde_json::Value =
        serde_json::from_slice(&crate::encode(&project).unwrap()).unwrap();
    for case in 0..11 {
        let mut value = original.clone();
        match case {
            0 => value["version"] = (LONG_GRANULAR_SCHEMA_VERSION + 1).into(),
            1 => value["base"]["version"] = LONG_GRANULAR_SCHEMA_VERSION.into(),
            2 => value["clips"] = serde_json::json!([]),
            3 => value["clips"][1]["clip"] = value["clips"][0]["clip"].clone(),
            4 => value["clips"][0]["clip"] = u64::MAX.into(),
            5 => {
                value["clips"][0]["edit"]["domain"]["warp"]["mode"] =
                    serde_json::json!({"kind":"complex"})
            }
            6 => value["clips"][0]["unexpected"] = true.into(),
            7 => value["clips"][0]["alias_source"] = 0.into(),
            8 => {
                value["clips"][0].as_object_mut().unwrap().remove("edit");
            }
            9 => {
                value["clips"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("alias_source");
            }
            _ => value["clips"][0]["edit"] = serde_json::Value::Null,
        }
        assert!(
            crate::decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn long_granular_aliases_round_trip_shared_content() {
    let (_, project) = fixture();
    let mut editor = Editor::new(project, 16).unwrap();
    let clip = editor.project().tracks()[0].clips()[0].id();
    editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap();
    let expected = editor.project();
    let bytes = crate::encode(expected).unwrap();
    assert_eq!(&crate::decode(&bytes).unwrap(), expected);
}

#[test]
fn historical_project_and_full_long_save_reopen_preserve_original_bytes() {
    let old = include_bytes!("../../../tests/fixtures/migrations/schema35-audio-8d2e61ab.json");
    let migrated = crate::decode(old).unwrap();
    assert_eq!(
        crate::decode(&crate::encode(&migrated).unwrap()).unwrap(),
        migrated
    );
    let (prior, project) = fixture();
    let path =
        std::env::temp_dir().join(format!("hex-long-granular-save-{}.hex", std::process::id()));
    let new_path = path.with_extension("new.hex");
    crate::save_new_with_token(&path, &prior).unwrap();
    let original = std::fs::read(&path).unwrap();
    let (loaded, token) = crate::load_with_token(&path).unwrap();
    assert_eq!(loaded, prior);
    assert!(token.requires_save_confirmation(&project).unwrap());
    crate::save_new_with_token(&new_path, &project).unwrap();
    assert_eq!(crate::load(&new_path).unwrap(), project);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(new_path).unwrap();
}

#[test]
fn prior_flat_schema_cannot_smuggle_long_granular_states() {
    let (_, project) = fixture();
    let older = super::super::content_view_resampler::encode(&project).unwrap();
    let error = crate::decode(&serde_json::to_vec(&older).unwrap()).unwrap_err();
    assert!(error.to_string().contains("long granular"), "{error}");
}
