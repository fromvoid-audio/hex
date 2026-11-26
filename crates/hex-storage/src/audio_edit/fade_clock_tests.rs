use super::*;
use crate::{decode, encode, schema_version_for};

const SCHEMA64: &[u8] = include_bytes!("../../tests/fixtures/warp-resampler/schema64.json");

#[test]
fn historical_domains_including_retained_roots_keep_legacy_law_and_complete_state() {
    let project = decode(SCHEMA64).unwrap();
    let mut count = 0;
    for clip in project.tracks().iter().flat_map(|track| track.clips()) {
        for clip in std::iter::once(clip).chain(clip.retained_audio_roots().map(|root| &root.clip))
        {
            if let Some(edit) = clip.audio_edit() {
                assert_eq!(edit.domain.fade_clock, AudioClipFadeClock::LegacyRoot);
                let dto = AudioEditDto::try_from(edit.clone()).unwrap();
                // Events are owned by their separate wrapper, not AudioEditDto.
                let restored = AudioClipEdit::try_from(dto).unwrap();
                assert_eq!(restored.domain.fade_clock, AudioClipFadeClock::LegacyRoot);
                count += 1;
            }
        }
    }
    assert!(count >= 5, "fixture must retain nested processing roots");
    let bytes = encode(&project).unwrap();
    assert_eq!(schema_version_for(&project), 64);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        serde_json::from_slice::<serde_json::Value>(SCHEMA64).unwrap()
    );
    assert_eq!(decode(&bytes).unwrap(), project);
}

#[test]
fn dto_encoder_refuses_outer_clock_before_it_can_drop_the_law() {
    let project = decode(SCHEMA64).unwrap();
    let edit = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .find_map(|clip| clip.audio_edit())
        .unwrap();
    for neutral in [false, true] {
        let mut edit = edit.clone();
        let domain = Arc::make_mut(&mut edit.domain);
        domain.fade_clock = AudioClipFadeClock::Outer;
        if neutral {
            domain.fade = ClipFade::neutral(domain.fade.span()).unwrap();
        }
        assert!(matches!(
            AudioEditDto::try_from(edit),
            Err(StorageError::Invalid(
                "outer clip fades are not supported yet"
            ))
        ));
    }
}

#[test]
fn versioned_outer_fade_preserves_exact_state() {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Imported { clip, .. } = editor
        .execute(hex_project::Command::ImportAudioTrack {
            name: "fade".into(),
            path: "fade.wav".into(),
            sample_rate: 48_000,
            frames: 16_000,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    editor
        .execute(hex_project::Command::SetAudioClipPitch { clip, cents: 300 })
        .unwrap();
    let plain = editor.project();
    let original = plain.clip(clip).unwrap();
    let fade = ClipFade::new(original.duration(), 0, 2_000, 1_000)
        .unwrap()
        .with_shape(33, 29, 270, -75)
        .unwrap()
        .with_curve_positions(320, 730)
        .unwrap()
        .with_curve_laws([
            hex_project::FadeCurveLaw::SmoothStep,
            hex_project::FadeCurveLaw::Linear,
        ]);
    let authored = original
        .with_authored_outer_fade(plain.tempo_map(), fade)
        .unwrap();
    let mut snapshot = plain.snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0] = authored.retained_snapshot();
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        schema_version_for(&project),
        crate::CAPABILITY_MANIFEST_SCHEMA_VERSION
    );
    let bytes = encode(&project).unwrap();
    assert_eq!(decode(&bytes).unwrap(), project);
    let legacy_outer_bytes = crate::encode_stacked_legacy(&project).unwrap();
    assert_eq!(decode(&legacy_outer_bytes).unwrap(), project);
    assert_ne!(
        schema_version_for(plain),
        crate::CAPABILITY_MANIFEST_SCHEMA_VERSION
    );
    assert!(crate::encode_schema20(&project).is_err());

    let legacy_bytes = encode(plain).unwrap();
    let dir = std::env::temp_dir().join(format!("hex-schema122-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let old_path = dir.join("original.hex");
    let new_path = dir.join("save-as.hex");
    std::fs::write(&old_path, &legacy_bytes).unwrap();
    let (old_project, old_token) = crate::load_with_token(&old_path).unwrap();
    assert_eq!(&old_project, plain);
    assert!(!old_token.requires_save_confirmation(plain).unwrap());
    let prepared = crate::prepare_replacement_within(
        &old_path,
        &old_token,
        &project,
        crate::LEGACY_READER_DOCUMENT_BYTES,
    )
    .unwrap();
    assert!(prepared.requires_confirmation());
    drop(prepared);
    crate::save_new(&new_path, &project).unwrap();
    let (reopened, new_token) = crate::load_with_token(&new_path).unwrap();
    assert_eq!(reopened, project);
    assert!(!new_token.requires_save_confirmation(&project).unwrap());
    assert_eq!(std::fs::read(&old_path).unwrap(), legacy_bytes);
    std::fs::remove_dir_all(&dir).unwrap();

    let saved: serde_json::Value = serde_json::from_slice(&legacy_outer_bytes).unwrap();
    let mut malformed = saved.clone();
    malformed["fades"][0]["clip"] = u64::MAX.into();
    assert!(decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    let mut malformed = saved.clone();
    malformed["fades"]
        .as_array_mut()
        .unwrap()
        .push(saved["fades"][0].clone());
    assert!(decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    let mut malformed = saved.clone();
    malformed["fades"][0]["fade"]["prefix"] = 120.into();
    malformed["fades"][0]["fade"]["offset"] = 1.into();
    assert!(decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    let mut malformed = saved.clone();
    malformed["fades"][0]["fade"]["future"] = true.into();
    assert!(decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    let mut malformed = saved;
    malformed["base"]["version"] = crate::OUTER_CLIP_FADE_SCHEMA_VERSION.into();
    assert!(decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
}

#[test]
fn schema122_preserves_musical_trim_context_inside_group_and_alias() {
    use hex_project::{BeatTime, Command, Created, Editor, Project, Tempo, WarpMode};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 12).unwrap();
    let Created::Imported { clip, track, asset } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let project = editor.project();
    let settings = hex_project::default_audio_warp(
        project.clip(clip).unwrap(),
        project.tempo_map(),
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
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    let original = editor.project().clip(clip).unwrap();
    let musical = original
        .with_authored_outer_fade(
            editor.project().tempo_map(),
            ClipFade::new(original.duration(), 0, 900, 350).unwrap(),
        )
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let mut row = musical.retained_snapshot();
    Arc::make_mut(&mut row.edit.as_mut().unwrap().domain)
        .shift_outer_fade_displacement(-i128::from(BeatTime::ONE.raw() / 8))
        .unwrap();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0] = row;
    editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 12).unwrap();
    let start = editor.project().clip(clip).unwrap().duration() + 321;
    let Created::Clip(other) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start,
            duration: 4321,
            source_offset: 0,
            source_length: 4321,
            looping: false,
        })
        .unwrap()
    else {
        panic!("second clip")
    };
    editor
        .execute(Command::GroupClips(vec![clip, other]))
        .unwrap();
    let group = editor.project().tracks()[0].clips()[0].id();
    let Created::Clips { first: alias, .. } = editor
        .execute(Command::DuplicateAliasClips(vec![group]))
        .unwrap()
    else {
        panic!("alias")
    };
    let project = editor.project();
    let mut snapshot = project.snapshot();
    for row in snapshot.tracks[0].audio_clips_mut().unwrap() {
        let live = project.tracks()[0]
            .clips()
            .iter()
            .find(|clip| clip.id().value() == row.id)
            .unwrap();
        let fade_in = if live.id() == alias { 900 } else { 400 };
        *row = live
            .with_authored_outer_fade(
                project.tempo_map(),
                ClipFade::new(live.duration(), 0, fade_in, 500).unwrap(),
            )
            .unwrap()
            .retained_snapshot();
    }
    let project = Project::from_snapshot(snapshot).unwrap();
    let bytes = encode(&project).unwrap();
    let restored = decode(&bytes).unwrap();
    assert_eq!(restored.snapshot(), project.snapshot());
    assert_eq!(encode(&restored).unwrap(), bytes);
    assert!(restored.clip_alias(group).is_some());
    assert_eq!(restored.clip_alias(group), restored.clip_alias(alias));
    let source = restored
        .clip(group)
        .unwrap()
        .retained_audio_roots()
        .next()
        .unwrap();
    let domain = &source.clip.audio_edit().unwrap().domain;
    assert_eq!(domain.fade_clock, AudioClipFadeClock::Outer);
    assert_eq!(domain.fade_start, BeatTime::ONE.raw() / 8);

    let legacy_bytes = crate::encode_stacked_legacy(&project).unwrap();
    assert_eq!(
        decode(&legacy_bytes).unwrap().snapshot(),
        project.snapshot()
    );
    let mut bad: serde_json::Value = serde_json::from_slice(&legacy_bytes).unwrap();
    let rows = bad["fades"].as_array_mut().unwrap();
    let member_row = rows
        .iter()
        .position(|r| r["clip"] == alias.value() && !r["sources"].as_array().unwrap().is_empty())
        .unwrap();
    rows.remove(member_row);
    assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
}
