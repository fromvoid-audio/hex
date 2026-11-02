//! Schema38 adds independent horizontal fade controls. Schema37 remains a strict
//! decoder/encoder boundary; its model constructors supply centered positions,
//! preserving the original rational law exactly when an old document is opened.
use super::{Project, StorageError};
use hex_project::TrackContentSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    clip: u64,
    in_position: u16,
    out_position: u16,
}

#[derive(Deserialize)]
struct Manifest(#[serde(deserialize_with = "bounded_positions")] Vec<Position>);
fn bounded_positions<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Position>, D::Error> {
    struct Positions;
    impl<'de> serde::de::Visitor<'de> for Positions {
        type Value = Vec<Position>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded arrangement fade curve positions")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut positions = Vec::new();
            while let Some(row) = seq.next_element()? {
                if positions.len() == hex_project::MAX_CLIPS {
                    return Err(serde::de::Error::custom("fade position limit reached"));
                }
                positions.push(row);
            }
            Ok(positions)
        }
    }
    d.deserialize_seq(Positions)
}

pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    super::clip_activation::require_legacy_compatible(project)?;
    // Bound the prior envelope before constructing an adaptation tree, just as
    // decode bounds source bytes. Large valid models must not bypass the document
    // limit by allocating their entire JSON tree before serialization refuses.
    let base = crate::encode_document(&super::tuning::DocumentV37::from_project(project)?)?;
    let mut value: serde_json::Value = serde_json::from_slice(&base)?;
    let positions: Vec<_> = project
        .tracks()
        .iter()
        .flat_map(|t| t.clips())
        .map(|clip| {
            let fade = clip.retained_fade();
            Position {
                clip: clip.id().value(),
                in_position: fade.in_position(),
                out_position: fade.out_position(),
            }
        })
        .collect();
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid fade position envelope"))?;
    object.insert("version".into(), 38.into());
    object.insert(
        "fade_curve_positions".into(),
        serde_json::to_value(positions)?,
    );
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: serde_json::Value) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid fade position envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(38) {
        return Err(StorageError::Invalid("invalid fade position version"));
    }
    let positions: Manifest = serde_json::from_value(
        object
            .remove("fade_curve_positions")
            .ok_or(StorageError::Invalid("fade curve positions are missing"))?,
    )?;
    object.insert("version".into(), 37.into());
    let project = super::tuning::decode_version_value(value, 37)?;
    let mut rows = BTreeMap::new();
    for row in positions.0 {
        if rows.insert(row.clip, row).is_some() {
            return Err(StorageError::Invalid("duplicate fade curve position"));
        }
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content else {
            continue;
        };
        for clip in audio_clips {
            let row = rows
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("fade curve position is missing"))?;
            let fade = if let Some(edit) = &mut clip.edit {
                &mut std::sync::Arc::make_mut(&mut edit.domain).fade
            } else {
                &mut clip.fade
            };
            *fade = fade
                .with_curve_positions(row.in_position, row.out_position)
                .map_err(|e| StorageError::Invalid(e.0))?;
        }
    }
    if !rows.is_empty() {
        return Err(StorageError::Invalid(
            "fade curve position references a non-arrangement audio clip",
        ));
    }
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{ClipFade, Command, Created, Editor};

    fn fixture(edited: bool, position: u16) -> Project {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "Case Preserved".into(),
                path: "Original/Source.WAV".into(),
                sample_rate: 48_000,
                frames: 1001,
            })
            .unwrap()
        else {
            panic!("import");
        };
        if edited {
            editor
                .execute(Command::SetAudioClipPitch { clip, cents: 137 })
                .unwrap();
        }
        let fade = ClipFade::new(1001, 0, 401, 301)
            .unwrap()
            .with_shape(10, 20, 700, -600)
            .unwrap()
            .with_curve_positions(position, 1000 - position)
            .unwrap();
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        editor.project().clone()
    }
    #[test]
    fn two_axis_current_roundtrip_and_strict_manifest_validation() {
        for edited in [false, true] {
            let project = fixture(edited, 250);
            let bytes = crate::encode(&project).unwrap();
            assert_eq!(crate::decode(&bytes).unwrap(), project);
            let directory =
                std::env::temp_dir().join(format!("hex-fade38-{}-{edited}", std::process::id()));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("curved.hex");
            crate::save_new(&path, &project).unwrap();
            assert_eq!(crate::load(&path).unwrap(), project);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            std::fs::remove_dir_all(directory).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            // The edited fixture's pitch renders with the v2 warp resampler.
            let version = if edited {
                crate::WARP_RESAMPLER_SCHEMA_VERSION
            } else {
                crate::CURRENT_SCHEMA_VERSION
            };
            assert_eq!(value["version"], version);
            for mutation in 0..9 {
                let mut bad = value.clone();
                match mutation {
                    0 => {
                        bad.as_object_mut().unwrap().remove("fade_curve_positions");
                    }
                    1 => bad["fade_curve_positions"] = serde_json::Value::Null,
                    2 => bad["fade_curve_positions"] = serde_json::json!([]),
                    3 => bad["fade_curve_positions"][0]["clip"] = 999999.into(),
                    4 => {
                        let row = bad["fade_curve_positions"][0].clone();
                        bad["fade_curve_positions"]
                            .as_array_mut()
                            .unwrap()
                            .push(row);
                    }
                    5 => bad["fade_curve_positions"][0]["in_position"] = 0.into(),
                    6 => bad["fade_curve_positions"][0]["out_position"] = 1000.into(),
                    7 => bad["fade_curve_positions"][0]["unknown"] = true.into(),
                    _ => {
                        bad["fade_curve_positions"][0]
                            .as_object_mut()
                            .unwrap()
                            .remove("in_position");
                    }
                }
                assert!(
                    crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
                    "mutation {mutation}"
                );
            }
            let raw = std::str::from_utf8(&bytes).unwrap();
            let duplicate = format!("{{\"fade_curve_positions\":[],{}", &raw[1..]);
            assert!(crate::decode(duplicate.as_bytes()).is_err());
        }
    }
    #[test]
    fn actual_schema37_encoder_opens_with_sample_exact_centered_defaults() {
        for edited in [false, true] {
            // A schema37-era project rendered its pitch with the v1 law.
            let project = super::super::warp_resampler::with_implied_laws(&fixture(edited, 500));
            // Frozen genuine prior encoder, rather than changing a current version tag.
            let old = crate::encode_document(
                &super::super::tuning::DocumentV37::from_project(&project).unwrap(),
            )
            .unwrap();
            let old_value: serde_json::Value = serde_json::from_slice(&old).unwrap();
            assert_eq!(old_value["version"], 37);
            assert!(old_value.get("fade_curve_positions").is_none());
            let migrated = crate::decode(&old).unwrap();
            assert_eq!(migrated, project);
            let fade = migrated.tracks()[0].clips()[0].retained_fade();
            assert_eq!((fade.in_position(), fade.out_position()), (500, 500));
            for frame in 0..1001 {
                assert_eq!(
                    fade.gain_at(frame),
                    project.tracks()[0].clips()[0]
                        .retained_fade()
                        .gain_at(frame)
                );
            }
            assert_eq!(
                crate::decode(&crate::encode(&migrated).unwrap()).unwrap(),
                project
            );
            let mut illicit = old_value;
            illicit["fade_curve_positions"] = serde_json::json!([]);
            assert!(crate::decode(&serde_json::to_vec(&illicit).unwrap()).is_err());
        }
    }
}
