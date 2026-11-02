//! Schema64 admits `SINC_CONVERSION_V2` asset conversion metadata beside the
//! schema63 layout. Its catalog rows carry each asset's actual conversion
//! version (0, 1 or 2). Every earlier schema could only describe identity and
//! sinc-v1 media, so they migrate unchanged and any v2 row inside them refuses.
//!
//! Only projects containing a v2 asset are written as schema64; all others stay
//! schema63 so older readers keep opening them. A schema64 document without a
//! v2 row is still read (it is the same layout) and is saved back as schema63.
//!
//! Embedded retained-root documents are produced by the frozen older encoders
//! from the same catalog and are checked against it while decoding; in those
//! internal documents a v2 asset appears with the v1 tag, which is restored from
//! the top-level catalog row. The catalog row is the single authority.
use crate::{Project, StorageError};
use hex_project::{SINC_CONVERSION_V1, SINC_CONVERSION_V2};
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) fn contains_v2(project: &Project) -> bool {
    project
        .assets()
        .any(|asset| asset.conversion_version() == SINC_CONVERSION_V2)
}

pub(crate) fn require_v1_conversions(project: &Project) -> Result<(), StorageError> {
    if contains_v2(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save sinc v2 media conversion; save with the current format",
        ));
    }
    Ok(())
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut v2 = BTreeSet::new();
    for asset in &mut snapshot.assets {
        if asset.conversion_version == SINC_CONVERSION_V2 {
            v2.insert(asset.id);
            asset.conversion_version = SINC_CONVERSION_V1;
        }
    }
    let neutral = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::track_name_origin::encode(&neutral)?;
    for row in catalog_rows(&mut value)? {
        let id = row.get("id").and_then(Value::as_u64);
        if id.is_some_and(|id| v2.contains(&id)) {
            row["conversion_version"] = SINC_CONVERSION_V2.into();
        }
    }
    value["version"] = crate::SINC_V2_SCHEMA_VERSION.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    if value.get("version").and_then(Value::as_u64) != Some(crate::SINC_V2_SCHEMA_VERSION.into()) {
        return Err(StorageError::Invalid("invalid media conversion schema"));
    }
    let mut v2 = BTreeSet::new();
    for row in catalog_rows(&mut value)? {
        if row.get("conversion_version").and_then(Value::as_u64)
            == Some(u64::from(SINC_CONVERSION_V2))
        {
            let id = row
                .get("id")
                .and_then(Value::as_u64)
                .ok_or(StorageError::Invalid("invalid asset catalog row"))?;
            v2.insert(id);
            row["conversion_version"] = SINC_CONVERSION_V1.into();
        }
    }
    value["version"] = crate::CURRENT_SCHEMA_VERSION.into();
    let project = super::track_name_origin::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = project.snapshot();
    for asset in &mut snapshot.assets {
        if v2.contains(&asset.id) {
            asset.conversion_version = SINC_CONVERSION_V2;
        }
    }
    // The model validates the restored version against each asset's rates.
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

fn catalog_rows(value: &mut Value) -> Result<&mut Vec<Value>, StorageError> {
    value
        .get_mut("assets")
        .and_then(Value::as_array_mut)
        .ok_or(StorageError::Invalid("asset catalog missing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    const SCHEMA63: &[u8] = include_bytes!("../../tests/fixtures/conversion-v2/schema63.json");

    fn rows(value: &Value) -> Vec<(u64, u64)> {
        value["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["id"].as_u64().unwrap(),
                    row["conversion_version"].as_u64().unwrap(),
                )
            })
            .collect()
    }

    fn with_v2_asset() -> (Project, u64) {
        let mut editor = Editor::new(crate::decode(SCHEMA63).unwrap(), 8).unwrap();
        let Created::Imported { asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "kaiser".into(),
                path: "Media/source-96000.wav".into(),
                source_sample_rate: 96_000,
                source_frames: 9_600,
                playback_frames: 4_800,
                conversion_version: SINC_CONVERSION_V2,
            })
            .unwrap()
        else {
            panic!("import")
        };
        (editor.project().clone(), asset.value())
    }

    #[test]
    fn historical_schema63_v1_and_identity_assets_open_and_save_unchanged() {
        let project = crate::decode(SCHEMA63).unwrap();
        let versions: Vec<_> = project.assets().map(|a| a.conversion_version()).collect();
        assert_eq!(versions, [1, 0]);
        assert_eq!(crate::schema_version_for(&project), 63);
        let token = crate::FileToken::from_validated_bytes(SCHEMA63.into());
        assert!(!token.requires_format_upgrade());
        assert!(!token.requires_save_confirmation(&project).unwrap());
        assert_saved_unchanged(&crate::encode(&project).unwrap());
        assert_eq!(
            crate::decode(&crate::encode(&project).unwrap())
                .unwrap()
                .snapshot(),
            project.snapshot()
        );
    }

    /// True when this build's serde_json writes object keys sorted. gpui turns
    /// on `preserve_order` through feature unification, so a workspace or app
    /// build writes struct field order while `-p hex-storage` sorts keys.
    fn json_objects_sort_keys() -> bool {
        let mut map = serde_json::Map::new();
        map.insert("b".into(), Value::Null);
        map.insert("a".into(), Value::Null);
        serde_json::to_string(&map).unwrap() == r#"{"a":null,"b":null}"#
    }

    /// The saved document is the historical schema63 file. Key order is the
    /// only serialization degree of freedom: JSON readers (including every
    /// older hex reader) look fields up by name, and duplicate keys refuse.
    /// Under sorted keys, the configuration the fixture was written with, the
    /// bytes must match exactly. Under `preserve_order` they must hold the same
    /// complete document, and saving again must be byte-stable.
    fn assert_saved_unchanged(saved: &[u8]) {
        let original: Value = serde_json::from_slice(SCHEMA63).unwrap();
        let written: Value = serde_json::from_slice(saved).unwrap();
        assert_eq!(written, original, "schema63 document content unchanged");
        if json_objects_sort_keys() {
            assert_eq!(saved, SCHEMA63, "byte-identical to the original file");
        }
        let resaved = crate::encode(&crate::decode(saved).unwrap()).unwrap();
        assert_eq!(resaved, saved, "saving again is byte-stable");
    }

    #[test]
    fn adding_v2_media_to_a_schema63_file_requires_overwrite_consent() {
        let (project, _) = with_v2_asset();
        assert_eq!(crate::schema_version_for(&project), 64);
        let token = crate::FileToken::from_validated_bytes(SCHEMA63.into());
        assert!(!token.requires_format_upgrade());
        assert!(token.requires_save_confirmation(&project).unwrap());
        // Once saved as schema64, later saves need no new consent; removing the
        // v2 media saves back to the older-reader layout without consent.
        let saved = crate::encode(&project).unwrap();
        let token = crate::FileToken::from_validated_bytes(saved.into());
        assert!(!token.requires_format_upgrade());
        assert!(!token.requires_save_confirmation(&project).unwrap());
        let v1_only = crate::decode(SCHEMA63).unwrap();
        assert!(!token.requires_save_confirmation(&v1_only).unwrap());
    }

    #[test]
    fn schema64_without_v2_rows_reads_and_saves_back_as_schema63() {
        let mut value: Value = serde_json::from_slice(SCHEMA63).unwrap();
        value["version"] = 64.into();
        let project = crate::decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_saved_unchanged(&crate::encode(&project).unwrap());
    }

    #[test]
    fn v2_assets_roundtrip_and_older_writers_and_readers_refuse_them() {
        let (project, v2) = with_v2_asset();
        let value: Value = serde_json::from_slice(&crate::encode(&project).unwrap()).unwrap();
        assert_eq!(value["version"], 64);
        assert_eq!(rows(&value), [(2, 1), (5, 0), (v2, 2)]);
        let reopened = crate::decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(reopened.snapshot(), project.snapshot());
        let restored = reopened.assets().find(|a| a.id().value() == v2).unwrap();
        assert_eq!(restored.conversion_version(), SINC_CONVERSION_V2);
        assert!(crate::encode_schema20(&project).is_err());
        assert!(crate::encode_schema22(&project).is_err());
        // A schema63 document never carried v2 metadata.
        let mut older = value.clone();
        older["version"] = 63.into();
        assert!(crate::decode(&serde_json::to_vec(&older).unwrap()).is_err());
    }

    #[test]
    fn unknown_or_rate_inconsistent_conversion_versions_refuse() {
        let (project, _) = with_v2_asset();
        let value = encode(&project).unwrap();
        // Row 2 is the v2 asset; row 1 is identity-rate, which admits only version 0.
        for (row, version) in [(2, 3), (2, u64::MAX), (1, 2), (1, 1)] {
            let mut bad = value.clone();
            bad["assets"][row]["conversion_version"] = version.into();
            let result = crate::decode(&serde_json::to_vec(&bad).unwrap());
            assert!(result.is_err(), "row {row} version {version} must refuse");
        }
        let mut newer = value;
        newer["version"] = (crate::NEWEST_SCHEMA_VERSION + 1).into();
        let error = crate::decode(&serde_json::to_vec(&newer).unwrap()).unwrap_err();
        assert!(error.to_string().contains("newer"));
    }
}
