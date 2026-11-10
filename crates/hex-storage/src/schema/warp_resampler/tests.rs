//! Schema65 warp resampler laws: historical migration, conditional writing,
//! complete round trips, overwrite consent and strict refusal.
use super::*;
use crate::{FileToken, decode, encode, schema_version_for};
use hex_project::{ClipId, Command, Created, Editor, WarpMode, WarpResampler, default_audio_warp};

const SCHEMA64: &[u8] = include_bytes!("../../../tests/fixtures/warp-resampler/schema64.json");

/// True when some content renders with the v2 law where v1 would differ.
fn uses_v2(project: &Project) -> bool {
    contents(project).any(|content| {
        content.warp_resampler == WarpResampler::V2 && content.has_explicit_warp_resampler()
    })
}

/// (pitch, warp enabled, law) of every content, alias members included.
fn laws(project: &Project) -> Vec<(i16, bool, WarpResampler)> {
    contents(project)
        .map(|c| (c.pitch_cents, c.warp_enabled, c.warp_resampler))
        .collect()
}

fn json_objects_sort_keys() -> bool {
    let mut map = serde_json::Map::new();
    map.insert("b".into(), Value::Null);
    map.insert("a".into(), Value::Null);
    serde_json::to_string(&map).unwrap() == r#"{"a":null,"b":null}"#
}

/// Every domain object carrying the schema65 field, at any depth.
fn explicit_domains(value: &mut Value) -> Vec<&mut serde_json::Map<String, Value>> {
    if value.get("warp_resampler").is_some() {
        return value.as_object_mut().into_iter().collect();
    }
    match value {
        Value::Object(map) => map.values_mut().flat_map(explicit_domains).collect(),
        Value::Array(items) => items.iter_mut().flat_map(explicit_domains).collect(),
        _ => Vec::new(),
    }
}

fn place(e: &mut Editor, start: u64, cents: i16) -> ClipId {
    let clip = e.project().tracks()[0].clips()[0].clone();
    let Created::Clip(id) = e
        .execute(Command::PlaceClip {
            track: e.project().tracks()[0].id(),
            asset: clip.asset(),
            start,
            duration: 48_000,
            source_offset: 0,
            source_length: 48_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("place")
    };
    e.execute(Command::SetAudioClipPitch { clip: id, cents })
        .unwrap();
    id
}

/// The schema64 fixture plus new v2 content in every location: a pitched
/// clip and its alias, a warped clip and a group retaining pitched roots.
fn with_v2_content() -> Project {
    let mut e = Editor::new(decode(SCHEMA64).unwrap(), 64).unwrap();
    let pitched = place(&mut e, 2_000_000, 1_200);
    e.execute(Command::DuplicateAliasClips(vec![pitched]))
        .unwrap();
    let warped = place(&mut e, 2_400_000, 0);
    let clip = e.project().clip(warped).unwrap().clone();
    let settings = default_audio_warp(&clip, e.project().tempo_map(), WarpMode::RePitch).unwrap();
    e.execute(Command::SetAudioClipWarp {
        clip: warped,
        settings: Some(settings),
    })
    .unwrap();
    let roots = vec![
        place(&mut e, 2_800_000, 500),
        place(&mut e, 2_900_000, -500),
    ];
    e.execute(Command::GroupClips(roots)).unwrap();
    e.project().clone()
}

#[test]
fn historical_fractional_content_migrates_to_v1_and_saves_unchanged() {
    let project = decode(SCHEMA64).unwrap();
    let laws = laws(&project);
    // Pitched clip and alias, warped clip and two retained roots read v1;
    // the gain-only clip and the group root read integer positions.
    let v1 = laws.iter().filter(|l| l.2 == WarpResampler::V1).count();
    assert_eq!(v1, 5, "{laws:?}");
    for (pitch, warp, law) in &laws {
        assert_eq!(*law, WarpResampler::implied(*pitch, *warp));
    }
    assert!(laws.iter().any(|l| l.1));
    assert!(!uses_v2(&project));
    assert_eq!(schema_version_for(&project), 64);
    let token = FileToken::from_validated_bytes(SCHEMA64.into());
    assert!(!token.requires_save_confirmation(&project).unwrap());
    let saved = encode(&project).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&saved).unwrap(),
        serde_json::from_slice::<Value>(SCHEMA64).unwrap()
    );
    if json_objects_sort_keys() {
        assert_eq!(saved, SCHEMA64, "byte-identical to the historical file");
    }
    // Older layouts with warp and pitch migrate the same way.
    for bytes in [
        &include_bytes!("../../../tests/fixtures/grouped-audio/schema42-event-fades-31545bb9.json")
            [..],
        &include_bytes!("../../../tests/fixtures/migrations/schema36-audio-97e885eb.json")[..],
    ] {
        let older = decode(bytes).unwrap();
        let laws = super::tests::laws(&older);
        assert!(laws.iter().any(|l| l.2 == WarpResampler::V1), "{laws:?}");
        assert!(!contains_explicit(&older));
    }
}

#[test]
fn v2_content_saves_as_schema65_and_round_trips_in_every_location() {
    let project = with_v2_content();
    let laws = laws(&project);
    let v2 = laws
        .iter()
        .filter(|l| l.2 == WarpResampler::V2 && (l.0 != 0 || l.1))
        .count();
    assert_eq!(v2, 5, "pitched, alias, warped and two roots: {laws:?}");
    assert!(uses_v2(&project));
    assert_eq!(schema_version_for(&project), 65);
    let saved = encode(&project).unwrap();
    let mut value: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(value["version"], 65);
    // Pitched representative, warped clip and two nested roots; v1 content
    // and alias payloads stay in their predecessor layout.
    let explicit = explicit_domains(&mut value);
    assert_eq!(explicit.len(), 4);
    assert!(explicit.iter().all(|d| d["warp_resampler"] == 2));
    let loaded = decode(&saved).unwrap();
    assert_eq!(loaded.snapshot(), project.snapshot());
    assert_eq!(encode(&loaded).unwrap(), saved);
}

#[test]
fn saving_v2_content_over_older_files_requires_consent() {
    let project = with_v2_content();
    let token = FileToken::from_validated_bytes(SCHEMA64.into());
    assert!(!token.requires_format_upgrade());
    assert!(token.requires_save_confirmation(&project).unwrap());
    let saved = FileToken::from_validated_bytes(encode(&project).unwrap().into());
    assert!(!saved.requires_format_upgrade());
    assert!(!saved.requires_save_confirmation(&project).unwrap());
    assert!(
        !saved
            .requires_save_confirmation(&decode(SCHEMA64).unwrap())
            .unwrap()
    );
    for writer in [crate::encode_schema22, crate::encode_schema20] {
        assert!(writer(&project).is_err());
    }
}

#[test]
fn v1_content_keeps_its_law_after_pitch_is_removed() {
    let mut e = Editor::new(decode(SCHEMA64).unwrap(), 8).unwrap();
    let pitched = e
        .project()
        .tracks()
        .iter()
        .flat_map(|t| t.clips())
        .find(|c| c.audio_edit().is_some_and(|a| a.domain.pitch_cents == 700))
        .unwrap()
        .id();
    e.execute(Command::SetAudioClipPitch {
        clip: pitched,
        cents: 0,
    })
    .unwrap();
    let project = e.project().clone();
    assert!(!uses_v2(&project));
    assert_eq!(schema_version_for(&project), 65);
    let saved = encode(&project).unwrap();
    let mut value: Value = serde_json::from_slice(&saved).unwrap();
    let explicit = explicit_domains(&mut value);
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0]["warp_resampler"], 1);
    let loaded = decode(&saved).unwrap();
    let edit = loaded.clip(pitched).unwrap().audio_edit().unwrap();
    assert_eq!(edit.domain.warp_resampler, WarpResampler::V1);
    assert_eq!(loaded.snapshot(), project.snapshot());
}

#[test]
fn older_schemas_and_malformed_laws_refuse() {
    let saved = encode(&with_v2_content()).unwrap();
    let mutate = |change: &dyn Fn(&mut Value)| {
        let mut value: Value = serde_json::from_slice(&saved).unwrap();
        change(&mut value);
        decode(&serde_json::to_vec(&value).unwrap())
    };
    for older in [63, 64] {
        assert!(mutate(&|v| v["version"] = older.into()).is_err());
    }
    assert!(mutate(&|v| v["version"] = (crate::NEWEST_SCHEMA_VERSION + 1).into()).is_err());
    for tag in [Value::from(0), 3.into(), "v2".into(), Value::Null] {
        let result = mutate(&|v| {
            explicit_domains(v)[0].insert("warp_resampler".into(), tag.clone());
        });
        assert!(result.is_err(), "{tag}");
    }
    // A pitched domain naming its implied law (v1) is not canonical.
    assert!(
        mutate(&|v| {
            explicit_domains(v)[0].insert("warp_resampler".into(), 1.into());
        })
        .is_err()
    );
    assert!(decode(&saved).is_ok());
}
