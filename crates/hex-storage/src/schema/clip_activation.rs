//! Schema40 adds a complete clip-activation inventory and elapsed MIDI playback
//! windows over frozen schema39. Decoding supported older schemas defaults all
//! clips active and elapsed windows to zero; explicit older writers refuse state they cannot represent.
//! Decoding is in-memory only; existing atomic save custody preserves originals.
use super::{Project, StorageError};
use hex_project::{SessionCellContent, SessionCellContentSnapshot, Snapshot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
    clip: u64,
    active: bool,
    /// Always zero for audio, elapsed quarter-note beat bits for MIDI.
    playback_offset_raw: u64,
}
#[derive(Deserialize)]
struct Inventory(#[serde(deserialize_with = "bounded")] Vec<Activation>);
fn bounded<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Activation>, D::Error> {
    struct Rows;
    impl<'de> serde::de::Visitor<'de> for Rows {
        type Value = Vec<Activation>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded clip activation inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == hex_project::MAX_CLIPS {
                    return Err(serde::de::Error::custom(
                        "clip activation inventory limit reached",
                    ));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}
fn visit(
    snapshot: &mut Snapshot,
    mut action: impl FnMut(u64, &mut bool, Option<&mut u64>) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    for track in &mut snapshot.tracks {
        match &mut track.content {
            TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } => {
                for clip in audio_clips {
                    action(clip.id, &mut clip.active, None)?;
                }
                for clip in midi_clips {
                    action(
                        clip.id,
                        &mut clip.active,
                        Some(&mut clip.playback_offset_raw),
                    )?;
                }
            }
            TrackContentSnapshot::Audio(clips) => {
                for clip in clips {
                    action(clip.id, &mut clip.active, None)?;
                }
            }
            TrackContentSnapshot::Instrument(clips) => {
                for clip in clips {
                    action(
                        clip.id,
                        &mut clip.active,
                        Some(&mut clip.playback_offset_raw),
                    )?;
                }
            }
        }
    }
    for cell in &mut snapshot.session.cells {
        match &mut cell.content {
            SessionCellContentSnapshot::Audio(clip) => action(clip.id, &mut clip.active, None)?,
            SessionCellContentSnapshot::Midi(clip) => action(
                clip.id,
                &mut clip.active,
                Some(&mut clip.playback_offset_raw),
            )?,
            SessionCellContentSnapshot::Stop => (),
        }
    }
    Ok(())
}
pub(crate) fn require_legacy_compatible(project: &Project) -> Result<(), StorageError> {
    let incompatible = project.tracks().iter().any(|track| {
        track.clips().iter().any(|clip| !clip.active())
            || track
                .midi_clips()
                .iter()
                .any(|clip| !clip.active() || clip.playback_offset() != hex_project::BeatTime::ZERO)
    }) || project
        .session_cells()
        .iter()
        .any(|cell| match cell.content() {
            SessionCellContent::Audio(clip) => !clip.active(),
            SessionCellContent::Midi(clip) => {
                !clip.active() || clip.playback_offset() != hex_project::BeatTime::ZERO
            }
            SessionCellContent::Stop => false,
        });
    if incompatible {
        Err(StorageError::Invalid(
            "older schema cannot save clip activation or midi playback windows",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    super::grouped_audio::require_legacy_compatible(project)?;
    super::content_views::require_legacy_compatible(project)?;
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    visit(&mut snapshot, |clip, active, offset| {
        rows.push(Activation {
            clip,
            active: *active,
            playback_offset_raw: offset.as_deref().copied().unwrap_or(0),
        });
        *active = true;
        if let Some(offset) = offset {
            *offset = 0;
        }
        Ok(())
    })?;
    let legacy =
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let base = crate::encode_document(&super::midi_one_shot::encode(&legacy)?)?;
    let mut value: serde_json::Value = serde_json::from_slice(&base)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid clip activation envelope"))?;
    object.insert("version".into(), 40.into());
    object.insert("clip_activation".into(), serde_json::to_value(rows)?);
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: serde_json::Value) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid clip activation envelope"))?;
    if object.get("version").and_then(|version| version.as_u64()) != Some(40) {
        return Err(StorageError::Invalid("invalid clip activation version"));
    }
    let inventory: Inventory = serde_json::from_value(object.remove("clip_activation").ok_or(
        StorageError::Invalid("clip activation inventory is missing"),
    )?)?;
    object.insert("version".into(), 39.into());
    let project = super::midi_one_shot::decode_value(value)?;
    let mut rows = BTreeMap::new();
    for row in inventory.0 {
        if rows.insert(row.clip, row).is_some() {
            return Err(StorageError::Invalid("duplicate clip activation identity"));
        }
    }
    let mut snapshot = project.snapshot();
    visit(&mut snapshot, |clip, active, offset| {
        let row = rows
            .remove(&clip)
            .ok_or(StorageError::Invalid("clip activation row is missing"))?;
        *active = row.active;
        if let Some(offset) = offset {
            *offset = row.playback_offset_raw;
        } else if row.playback_offset_raw != 0 {
            return Err(StorageError::Invalid(
                "audio clip cannot have a midi playback window",
            ));
        }
        Ok(())
    })?;
    if !rows.is_empty() {
        return Err(StorageError::Invalid(
            "clip activation references a missing clip",
        ));
    }
    Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, Editor};
    use serde_json::{Value, json};
    fn b(n: u64) -> BeatTime {
        BeatTime::from_quarters(n).unwrap()
    }
    fn fixture() -> Project {
        let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Imported { track, clip, .. } = e
            .execute(Command::ImportAudioTrack {
                name: "Original Name".into(),
                path: "Media/Original.WAV".into(),
                sample_rate: 48_000,
                frames: 1_000,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        let Created::Scene(scene) = e
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        e.execute(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        })
        .unwrap();
        let Created::Track(midi_track) = e
            .execute(Command::AddTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(midi) = e
            .execute(Command::PlaceMidiClip {
                track: midi_track,
                start: b(1),
                length: b(8),
            })
            .unwrap()
        else {
            panic!("midi")
        };
        e.execute(Command::CopyArrangementMidiToSession {
            source: midi,
            scene,
        })
        .unwrap();
        assert_eq!(e.project().track(track).unwrap().clips().len(), 1);
        let mut snapshot = e.project().snapshot();
        visit(&mut snapshot, |_, active, offset| {
            *active = false;
            if let Some(offset) = offset {
                *offset = b(12).raw();
            }
            Ok(())
        })
        .unwrap();
        Project::from_legacy_snapshot(snapshot).unwrap()
    }
    #[test]
    fn activation40_complete_roundtrip_and_historical39_defaults() {
        let project = fixture();
        let bytes = crate::encode_document(&encode(&project).unwrap()).unwrap();
        assert_eq!(crate::decode(&bytes).unwrap(), project);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], 40);
        assert_eq!(value["clip_activation"].as_array().unwrap().len(), 4);
        assert!(super::super::midi_one_shot::encode(&project).is_err());
        assert!(crate::encode_schema22(&project).is_err());
        assert!(crate::encode_schema20(&project).is_err());
        let mut neutral = project.snapshot();
        visit(&mut neutral, |_, active, offset| {
            *active = true;
            if let Some(offset) = offset {
                *offset = 0;
            }
            Ok(())
        })
        .unwrap();
        let neutral = Project::from_legacy_snapshot(neutral).unwrap();
        let old = crate::encode_document(&super::super::midi_one_shot::encode(&neutral).unwrap())
            .unwrap();
        assert_eq!(crate::decode(&old).unwrap(), neutral);
    }
    #[test]
    fn activation40_refuses_nonbijective_malformed_and_exhausted_windows() {
        let bytes = crate::encode(&fixture()).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        for case in 0..9 {
            let mut bad = value.clone();
            match case {
                0 => {
                    bad.as_object_mut().unwrap().remove("clip_activation");
                }
                1 => bad["clip_activation"] = json!([]),
                2 => {
                    let row = bad["clip_activation"][0].clone();
                    bad["clip_activation"].as_array_mut().unwrap().push(row);
                }
                3 => bad["clip_activation"][0]["clip"] = json!(u64::MAX),
                4 => bad["clip_activation"][0]["active"] = json!("false"),
                5 => bad["clip_activation"][0]["unknown"] = json!(0),
                6 => bad["clip_activation"][0]["playback_offset_raw"] = json!(1),
                7 => bad["clip_activation"][1]["playback_offset_raw"] = json!(u64::MAX),
                _ => {
                    bad["clip_activation"] = json!(vec![
                        value["clip_activation"][0].clone();
                        hex_project::MAX_CLIPS + 1
                    ])
                }
            }
            assert!(
                crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "case {case}"
            );
        }
        let raw = String::from_utf8(bytes).unwrap();
        let duplicate = format!("{{\"clip_activation\":[],{}", &raw[1..]);
        assert!(crate::decode(duplicate.as_bytes()).is_err());
    }
}
