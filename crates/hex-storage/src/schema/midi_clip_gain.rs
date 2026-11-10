//! Schema62 stores the independent MIDI clip scalar beside the schema61
//! source-beat curves. Earlier supported schemas migrate at exact unity.
use crate::{Project, StorageError};
use hex_project::{ClipGain, SessionCellContentSnapshot, Snapshot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    millidb: i32,
}

fn visit(snapshot: &mut Snapshot, mut f: impl FnMut(u64, &mut hex_project::MidiClipVolume)) {
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(_) => continue,
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        };
        for clip in clips {
            f(clip.id, &mut clip.volume);
        }
    }
    for cell in &mut snapshot.session.cells {
        if let SessionCellContentSnapshot::Midi(clip) = &mut cell.content {
            f(clip.id, &mut clip.volume);
        }
    }
}

pub(crate) fn require_unity(project: &Project) -> Result<(), StorageError> {
    let mut snapshot = project.snapshot();
    let mut nonunity = false;
    visit(&mut snapshot, |_, volume| {
        nonunity |= volume.clip_gain() != ClipGain::UNITY
    });
    if nonunity {
        return Err(StorageError::Invalid(
            "older schema cannot save midi clip gain",
        ));
    }
    Ok(())
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    visit(&mut snapshot, |clip, volume| {
        if volume.clip_gain() != ClipGain::UNITY {
            rows.push(Row {
                clip,
                millidb: volume.clip_gain().millidb(),
            });
            *volume = volume.clone().with_clip_gain(ClipGain::UNITY);
        }
    });
    rows.sort_unstable_by_key(|row| row.clip);
    let neutral = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::event_gain::encode(&neutral)?;
    value["midi_clip_gains"] = serde_json::to_value(rows)?;
    value["version"] = 62.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: Value) -> Result<Project, StorageError> {
    if value.get("version").and_then(Value::as_u64) != Some(62) {
        return Err(StorageError::Invalid("invalid midi clip gain schema"));
    }
    let raw = value
        .as_object_mut()
        .and_then(|v| v.remove("midi_clip_gains"))
        .ok_or(StorageError::Invalid("midi clip gain inventory missing"))?;
    value["version"] = 61.into();
    let project = super::event_gain::decode_value(value)?;
    let mut snapshot = project.snapshot();
    let mut clip_count = 0usize;
    visit(&mut snapshot, |_, _| clip_count += 1);
    let rows = raw
        .as_array()
        .ok_or(StorageError::Invalid("invalid midi clip gain inventory"))?;
    if rows.len() > clip_count {
        return Err(StorageError::Invalid(
            "midi clip gain inventory exceeds clip count",
        ));
    }
    let rows: Vec<Row> = serde_json::from_value(raw)?;
    let mut remaining = BTreeMap::new();
    for row in rows {
        let gain = ClipGain::new(row.millidb).map_err(|e| StorageError::Invalid(e.0))?;
        if row.clip == 0 || gain == ClipGain::UNITY || remaining.insert(row.clip, gain).is_some() {
            return Err(StorageError::Invalid("invalid or duplicate midi clip gain"));
        }
    }
    visit(&mut snapshot, |clip, volume| {
        if let Some(gain) = remaining.remove(&clip) {
            *volume = volume.clone().with_clip_gain(gain);
        }
    });
    if !remaining.is_empty() {
        return Err(StorageError::Invalid("midi clip gain owner missing"));
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, Editor};

    #[test]
    fn historical_schema61_writer_migrates_complete_state_at_unity() {
        let bytes = include_bytes!("../../tests/fixtures/midi-clip-gain/schema61.json");
        let project = crate::decode(bytes).unwrap();
        let mut snapshot = project.snapshot();
        let mut owners = 0;
        visit(&mut snapshot, |_, volume| {
            assert_eq!(volume.clip_gain(), ClipGain::UNITY);
            assert_eq!(volume.points().len(), 1);
            assert_eq!(volume.points()[0].value.linear(), 0.5);
            assert_eq!(volume.points()[0].incoming.linear(), 0.5);
            owners += 1;
        });
        assert_eq!(owners, 2);
        let midi_notes: Vec<_> = project
            .tracks()
            .iter()
            .flat_map(|track| track.midi_clips())
            .flat_map(|clip| clip.notes())
            .collect();
        assert_eq!(midi_notes.len(), 1);
        assert_eq!(midi_notes[0].velocity(), 90);
        let SessionCellContentSnapshot::Midi(session_clip) = &snapshot.session.cells[0].content
        else {
            panic!("expected historical session midi clip");
        };
        assert_eq!(session_clip.source.notes[0].velocity, 90);
        let audio_clips: Vec<_> = project
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .collect();
        assert_eq!(audio_clips.len(), 1);
        let events = audio_clips[0].audio_events();
        assert_eq!(events.events.len(), 1);
        assert_eq!(events.events[0].gain.millidb(), -6_000);
        let reloaded = crate::decode(&crate::encode(&project).unwrap()).unwrap();
        assert_eq!(reloaded.snapshot(), project.snapshot());
    }

    fn fixture() -> (Editor, hex_project::ClipId, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(arrangement) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("arrangement clip")
        };
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(session) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "session".into(),
                source_length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        (editor, arrangement, session)
    }

    #[test]
    fn distinct_arrangement_and_session_gains_roundtrip_and_inventory_is_strict() {
        let (mut editor, arrangement, session) = fixture();
        let old = super::super::event_gain::encode(editor.project()).unwrap();
        let migrated = crate::decode(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(migrated.snapshot(), editor.project().snapshot());
        for (clip, db) in [(arrangement, -6_000), (session, 3_000)] {
            editor
                .execute(Command::SetMidiClipGain {
                    clip,
                    gain: ClipGain::new(db).unwrap(),
                })
                .unwrap();
        }
        let project = editor.project();
        assert!(super::super::event_gain::encode(project).is_err());
        assert!(crate::encode_schema20(project).is_err());
        let value = encode(project).unwrap();
        assert_eq!(value["midi_clip_gains"].as_array().unwrap().len(), 2);
        assert_eq!(
            crate::decode(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .snapshot(),
            project.snapshot()
        );
        let mut duplicate = value.clone();
        duplicate["midi_clip_gains"].as_array_mut().unwrap()[1] =
            duplicate["midi_clip_gains"][0].clone();
        assert!(crate::decode(&serde_json::to_vec(&duplicate).unwrap()).is_err());
        let mut unknown = value.clone();
        unknown["midi_clip_gains"][0]["clip"] = u64::MAX.into();
        assert!(crate::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
        let mut unity = value.clone();
        unity["midi_clip_gains"][0]["millidb"] = 0.into();
        assert!(crate::decode(&serde_json::to_vec(&unity).unwrap()).is_err());
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("midi_clip_gains");
        assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
        let mut out_of_range = value;
        out_of_range["midi_clip_gains"][0]["millidb"] = 24_001.into();
        assert!(crate::decode(&serde_json::to_vec(&out_of_range).unwrap()).is_err());
    }
}
