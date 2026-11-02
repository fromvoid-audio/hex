//! Schema66 directed view read laws: historical migration, conditional
//! writing, complete round trips, overwrite consent and strict refusal.
use super::*;
use crate::{FileToken, decode, encode, schema_version_for};
use hex_project::{AudioDomainClock, Command, Created, Editor, WarpResampler};

const SCHEMA65: &[u8] =
    include_bytes!("../../../tests/fixtures/content-view-resampler/schema65.json");

/// (musical clock, read law) of every directed audio view.
fn laws(project: &Project) -> Vec<(bool, WarpResampler)> {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter_map(hex_project::Clip::audio_edit)
        .filter_map(|edit| {
            let view = edit.content_view?;
            let musical = matches!(edit.domain.clock, AudioDomainClock::Beats(_));
            Some((musical, view.read_resampler))
        })
        .collect()
}

fn json_objects_sort_keys() -> bool {
    let mut map = serde_json::Map::new();
    map.insert("b".into(), Value::Null);
    map.insert("a".into(), Value::Null);
    serde_json::to_string(&map).unwrap() == r#"{"a":null,"b":null}"#
}

/// Every saved audio view object carrying the schema66 field.
fn explicit_views(value: &mut Value) -> Vec<&mut serde_json::Map<String, Value>> {
    value["content_views"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter_map(|row| row["view"]["view"].as_object_mut())
        .filter(|view| view.contains_key("read_resampler"))
        .collect()
}

/// The schema65 fixture plus a new musical-clock reversed clip.
fn with_v2_view() -> Project {
    let mut e = Editor::new(decode(SCHEMA65).unwrap(), 64).unwrap();
    let track = e.project().tracks()[0].id();
    let asset = e.project().tracks()[0].clips()[0].asset();
    let Created::Clip(clip) = e
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 1_440_000,
            duration: 24_000,
            source_offset: 0,
            source_length: 30_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("place")
    };
    e.execute(Command::SetAudioClipMusical {
        clip,
        enabled: true,
    })
    .unwrap();
    e.execute(Command::ReverseClips(vec![clip])).unwrap();
    e.project().clone()
}

#[test]
fn historical_views_migrate_to_their_implied_laws_and_save_unchanged() {
    let project = decode(SCHEMA65).unwrap();
    // The musical view rendered with frozen v1; the frame view copies
    // integer samples and takes the current default.
    assert_eq!(
        laws(&project),
        [(true, WarpResampler::V1), (false, WarpResampler::V2)]
    );
    assert!(!contains_explicit(&project));
    assert_eq!(schema_version_for(&project), 65);
    let token = FileToken::from_validated_bytes(SCHEMA65.into());
    assert!(!token.requires_save_confirmation(&project).unwrap());
    let saved = encode(&project).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&saved).unwrap(),
        serde_json::from_slice::<Value>(SCHEMA65).unwrap()
    );
    if json_objects_sort_keys() {
        assert_eq!(saved, SCHEMA65, "byte-identical to the historical file");
    }
    // The first directed-view layouts migrate the same way.
    for bytes in [
        &include_bytes!("../../../tests/fixtures/event-fades/schema41-audio-600ea714.json")[..],
        &include_bytes!("../../../tests/fixtures/grouped-audio/schema42-event-fades-31545bb9.json")
            [..],
    ] {
        let older = decode(bytes).unwrap();
        let laws = laws(&older);
        assert!(!laws.is_empty());
        for (musical, law) in laws {
            let implied = if musical {
                WarpResampler::V1
            } else {
                WarpResampler::V2
            };
            assert_eq!(law, implied);
        }
        assert!(!contains_explicit(&older));
    }
}

#[test]
fn new_musical_view_saves_as_schema66_and_round_trips() {
    let project = with_v2_view();
    assert_eq!(
        laws(&project),
        [
            (true, WarpResampler::V1),
            (false, WarpResampler::V2),
            (true, WarpResampler::V2)
        ]
    );
    assert!(contains_explicit(&project));
    assert_eq!(schema_version_for(&project), 66);
    let saved = encode(&project).unwrap();
    let mut value: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(value["version"], 66);
    let explicit = explicit_views(&mut value);
    assert_eq!(explicit.len(), 1, "migrated views keep their older layout");
    assert_eq!(explicit[0]["read_resampler"], 2);
    let loaded = decode(&saved).unwrap();
    assert_eq!(loaded.snapshot(), project.snapshot());
    assert_eq!(encode(&loaded).unwrap(), saved);
}

#[test]
fn explicit_v1_frame_view_round_trips() {
    let mut snapshot = decode(SCHEMA65).unwrap().snapshot();
    let hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } =
        &mut snapshot.tracks[0].content
    else {
        panic!("hybrid track")
    };
    let edit = audio_clips[1].edit.as_mut().unwrap();
    assert!(matches!(edit.domain.clock, AudioDomainClock::Frames(_)));
    edit.content_view.as_mut().unwrap().read_resampler = WarpResampler::V1;
    let project = Project::from_snapshot(snapshot).unwrap();
    let saved = encode(&project).unwrap();
    let mut value: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(value["version"], 66);
    let explicit = explicit_views(&mut value);
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0]["read_resampler"], 1);
    assert_eq!(decode(&saved).unwrap().snapshot(), project.snapshot());
}

#[test]
fn saving_a_v2_view_over_older_files_requires_consent() {
    let project = with_v2_view();
    let token = FileToken::from_validated_bytes(SCHEMA65.into());
    assert!(!token.requires_format_upgrade());
    assert!(token.requires_save_confirmation(&project).unwrap());
    let saved = FileToken::from_validated_bytes(encode(&project).unwrap().into());
    assert!(!saved.requires_format_upgrade());
    assert!(!saved.requires_save_confirmation(&project).unwrap());
    // Removing the v2 view saves back to the older-reader layout.
    assert!(
        !saved
            .requires_save_confirmation(&decode(SCHEMA65).unwrap())
            .unwrap()
    );
    for writer in [crate::encode_schema22, crate::encode_schema20] {
        assert!(writer(&project).is_err());
    }
}

#[test]
fn older_schemas_and_malformed_view_laws_refuse() {
    let saved = encode(&with_v2_view()).unwrap();
    let mutate = |change: &dyn Fn(&mut Value)| {
        let mut value: Value = serde_json::from_slice(&saved).unwrap();
        change(&mut value);
        decode(&serde_json::to_vec(&value).unwrap())
    };
    for older in [63, 64, 65] {
        assert!(mutate(&|v| v["version"] = older.into()).is_err(), "{older}");
    }
    assert!(mutate(&|v| v["version"] = (crate::NEWEST_SCHEMA_VERSION + 1).into()).is_err());
    for tag in [Value::from(0), 3.into(), "v2".into(), Value::Null] {
        let result = mutate(&|v| {
            explicit_views(v)[0].insert("read_resampler".into(), tag.clone());
        });
        assert!(result.is_err(), "{tag}");
    }
    // A musical view naming its implied law (v1) is not canonical.
    assert!(
        mutate(&|v| {
            explicit_views(v)[0].insert("read_resampler".into(), 1.into());
        })
        .is_err()
    );
    assert!(decode(&saved).is_ok());
}
