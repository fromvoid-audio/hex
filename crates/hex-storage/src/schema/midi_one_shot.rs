//! Schema39 adds a complete source-policy inventory over the frozen schema38
//! envelope. Older formats supply SourceEnd. The policy retains authored notes
//! beyond a nonrepeating audible boundary; it never truncates source metadata.
use super::{Project, StorageError};
use hex_project::{MidiOneShotBoundary, SessionCellContent, SessionCellContentSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    SourceEnd,
    LoopEnd,
}
impl From<MidiOneShotBoundary> for Boundary {
    fn from(value: MidiOneShotBoundary) -> Self {
        match value {
            MidiOneShotBoundary::SourceEnd => Self::SourceEnd,
            MidiOneShotBoundary::LoopEnd => Self::LoopEnd,
        }
    }
}
impl From<Boundary> for MidiOneShotBoundary {
    fn from(value: Boundary) -> Self {
        match value {
            Boundary::SourceEnd => Self::SourceEnd,
            Boundary::LoopEnd => Self::LoopEnd,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    clip: u64,
    boundary: Boundary,
}
#[derive(Deserialize)]
struct Policies(#[serde(deserialize_with = "bounded_policies")] Vec<Policy>);
fn bounded_policies<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Policy>, D::Error> {
    struct Rows;
    impl<'de> serde::de::Visitor<'de> for Rows {
        type Value = Vec<Policy>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded midi one-shot policies")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == hex_project::MAX_CLIPS {
                    return Err(serde::de::Error::custom(
                        "midi one-shot policy limit reached",
                    ));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}

pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    super::clip_activation::require_legacy_compatible(project)?;
    let mut value = super::fade_curve_positions::encode(project)?;
    let policies: Vec<_> = project
        .tracks()
        .iter()
        .flat_map(|track| track.midi_clips())
        .map(|clip| Policy {
            clip: clip.id().value(),
            boundary: clip.source().one_shot_boundary().into(),
        })
        .chain(
            project
                .session_cells()
                .iter()
                .filter_map(|cell| match cell.content() {
                    SessionCellContent::Midi(clip) => Some(Policy {
                        clip: clip.id().value(),
                        boundary: clip.source().one_shot_boundary().into(),
                    }),
                    _ => None,
                }),
        )
        .collect();
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid midi policy envelope"))?;
    object.insert("version".into(), 39.into());
    object.insert(
        "midi_one_shot_boundaries".into(),
        serde_json::to_value(policies)?,
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
        .ok_or(StorageError::Invalid("invalid midi policy envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(39) {
        return Err(StorageError::Invalid("invalid midi policy version"));
    }
    let policies: Policies = serde_json::from_value(
        object
            .remove("midi_one_shot_boundaries")
            .ok_or(StorageError::Invalid("midi one-shot policies are missing"))?,
    )?;
    object.insert("version".into(), 38.into());
    let project = super::fade_curve_positions::decode_value(value)?;
    let mut rows = BTreeMap::new();
    for row in policies.0 {
        if rows.insert(row.clip, row.boundary.into()).is_some() {
            return Err(StorageError::Invalid("duplicate midi one-shot policy"));
        }
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            hex_project::TrackContentSnapshot::Instrument(clips) => clips,
            hex_project::TrackContentSnapshot::Audio(_) => continue,
        };
        for clip in clips {
            clip.one_shot_boundary = rows
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("midi one-shot policy is missing"))?;
        }
    }
    for cell in &mut snapshot.session.cells {
        if let SessionCellContentSnapshot::Midi(clip) = &mut cell.content {
            clip.source.one_shot_boundary = rows
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("midi one-shot policy is missing"))?;
        }
    }
    if !rows.is_empty() {
        return Err(StorageError::Invalid(
            "midi one-shot policy references a missing midi clip",
        ));
    }
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, ClipFade, Command, Created, Editor};
    use serde_json::{Value, json};
    fn b(n: u64) -> BeatTime {
        BeatTime::from_quarters(n).unwrap()
    }
    fn fixture(boundary: MidiOneShotBoundary) -> Project {
        let mut e = Editor::new(Project::new(48000).unwrap(), 32).unwrap();
        let Created::Imported {
            track, clip: audio, ..
        } = e
            .execute(Command::ImportAudioTrack {
                name: "Original Case".into(),
                path: "Media/Missing.WAV".into(),
                sample_rate: 48000,
                frames: 1000,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        e.execute(Command::SetClipFadeEnvelope {
            clip: audio,
            fade: ClipFade::new(1000, 0, 100, 200)
                .unwrap()
                .with_shape(0, 0, 300, -400)
                .unwrap()
                .with_curve_positions(250, 750)
                .unwrap(),
        })
        .unwrap();
        let Created::Clip(clip) = e
            .execute(Command::PlaceMidiClip {
                track,
                start: b(4),
                length: b(12),
            })
            .unwrap()
        else {
            panic!("midi")
        };
        e.execute(Command::AddMidiNote {
            clip,
            start: b(10),
            duration: b(2),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
        e.execute(Command::SetMidiClipSource {
            clip,
            source_length: b(12),
            source_start: b(2),
            loop_start: b(2),
            loop_end: b(6),
            looping: false,
            length: b(20),
        })
        .unwrap();
        e.execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: hex_project::MidiClipVolume::new(
                vec![
                    hex_project::MidiClipVolumePoint::linear(
                        b(0),
                        hex_project::MidiClipGain::new(0.25).unwrap(),
                    ),
                    hex_project::MidiClipVolumePoint::linear(
                        b(12),
                        hex_project::MidiClipGain::new(1.5).unwrap(),
                    ),
                ],
                b(12),
            )
            .unwrap(),
        })
        .unwrap();
        let Created::Scene(scene) = e
            .execute(Command::AddScene {
                name: "Original Scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        e.execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap();
        let mut s = e.project().snapshot();
        let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut s.tracks[0].content
        else {
            panic!("hybrid")
        };
        midi_clips[0].one_shot_boundary = boundary;
        let SessionCellContentSnapshot::Midi(clip) = &mut s.session.cells[0].content else {
            panic!("midi")
        };
        clip.source.one_shot_boundary = boundary;
        Project::from_legacy_snapshot(s).unwrap()
    }
    #[test]
    fn policy39_roundtrip_preserves_retained_notes_and_all_frozen38_fields() {
        for boundary in [MidiOneShotBoundary::SourceEnd, MidiOneShotBoundary::LoopEnd] {
            let project = fixture(boundary);
            let bytes = crate::encode_document(&encode(&project).unwrap()).unwrap();
            assert_eq!(crate::decode(&bytes).unwrap(), project);
            let mut value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["version"], 39);
            let policies = value
                .as_object_mut()
                .unwrap()
                .remove("midi_one_shot_boundaries")
                .unwrap();
            assert_eq!(policies.as_array().unwrap().len(), 2);
            value["version"] = 38.into();
            assert_eq!(
                value,
                super::super::fade_curve_positions::encode(&project).unwrap()
            );
            assert_eq!(
                project.tracks()[0].midi_clips()[0].notes()[0].start(),
                b(10)
            );
            let dir = std::env::temp_dir()
                .join(format!("hex-policy39-{}-{boundary:?}", std::process::id()));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("retained.hex");
            crate::save_new(&path, &project).unwrap();
            assert_eq!(crate::load(&path).unwrap(), project);
            assert_eq!(
                std::fs::read(&path).unwrap(),
                crate::encode(&project).unwrap()
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn frozen38_defaults_both_source_owners_without_rewriting_original_bytes() {
        let project = fixture(MidiOneShotBoundary::SourceEnd);
        let old =
            crate::encode_document(&super::super::fade_curve_positions::encode(&project).unwrap())
                .unwrap();
        assert_eq!(crate::decode(&old).unwrap(), project);
        let dir = std::env::temp_dir().join(format!("hex-policy38-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("original.hex");
        std::fs::write(&path, &old).unwrap();
        let (loaded, token) = crate::load_with_token(&path).unwrap();
        assert_eq!(loaded, project);
        assert!(token.requires_format_upgrade());
        assert_eq!(std::fs::read(&path).unwrap(), old);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn legacy20_and22_writers_refuse_loop_end_without_restricting_source_end() {
        let mut e = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let Created::Track(track) = e
            .execute(Command::AddTrack {
                name: "legacy".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        e.execute(Command::SetTrackColor {
            track,
            color: hex_project::ElementColor::Gray,
        })
        .unwrap();
        e.execute(Command::PlaceMidiClip {
            track,
            start: b(0),
            length: b(12),
        })
        .unwrap();
        let source_end = e.project().clone();
        assert_eq!(
            crate::decode(&crate::encode(&source_end).unwrap()).unwrap(),
            source_end
        );
        for (version, writer) in [
            (
                20,
                crate::encode_schema20 as fn(&Project) -> Result<Vec<u8>, StorageError>,
            ),
            (22, crate::encode_schema22),
        ] {
            let bytes = writer(&source_end).unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
                version
            );
            let mut snapshot = source_end.snapshot();
            let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
                &mut snapshot.tracks[0].content
            else {
                panic!("hybrid")
            };
            midi_clips[0].one_shot_boundary = MidiOneShotBoundary::LoopEnd;
            let loop_end = Project::from_legacy_snapshot(snapshot).unwrap();
            assert!(matches!(
                writer(&loop_end),
                Err(StorageError::Invalid(
                    "older schema cannot save midi one-shot boundaries"
                ))
            ));
        }
    }
    #[test]
    fn policy39_refuses_missing_unknown_duplicate_and_invalid_boundaries() {
        let original = crate::encode(&fixture(MidiOneShotBoundary::LoopEnd)).unwrap();
        let value: Value = serde_json::from_slice(&original).unwrap();
        for case in 0..10 {
            let mut bad = value.clone();
            match case {
                0 => {
                    bad.as_object_mut()
                        .unwrap()
                        .remove("midi_one_shot_boundaries");
                }
                1 => bad["midi_one_shot_boundaries"] = Value::Null,
                2 => bad["midi_one_shot_boundaries"] = json!([]),
                3 => bad["midi_one_shot_boundaries"][0]["boundary"] = json!("unknown"),
                4 => bad["midi_one_shot_boundaries"][0]["clip"] = json!(u64::MAX),
                5 => {
                    let row = bad["midi_one_shot_boundaries"][0].clone();
                    bad["midi_one_shot_boundaries"]
                        .as_array_mut()
                        .unwrap()
                        .push(row);
                }
                6 => bad["midi_one_shot_boundaries"][0]["unknown"] = json!(true),
                7 => {
                    bad["midi_one_shot_boundaries"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("boundary");
                }
                8 => bad["tracks"][0]["midi_clips"][0]["source_start_raw"] = json!(b(6).raw()),
                _ => {
                    bad["midi_one_shot_boundaries"] =
                        json!(vec![
                            value["midi_one_shot_boundaries"][0].clone();
                            hex_project::MAX_CLIPS + 1
                        ])
                }
            }
            assert!(
                crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "case {case}"
            );
        }
        let raw = String::from_utf8(original).unwrap();
        let duplicate = format!("{{\"midi_one_shot_boundaries\":[],{}", &raw[1..]);
        assert!(crate::decode(duplicate.as_bytes()).is_err());
        let duplicate = raw.replacen(
            "\"boundary\":\"loop_end\"",
            "\"boundary\":\"source_end\",\"boundary\":\"loop_end\"",
            1,
        );
        assert_ne!(duplicate, raw);
        assert!(crate::decode(duplicate.as_bytes()).is_err());
    }
}
