//! Schema63 retains finite per-note gain laws; schema62 migrates at unity.
use crate::{Project, StorageError};
use hex_project::{
    AliasContentSnapshot, BeatTime, ClipGain, MAX_MIDI_VOICE_GAIN_LAYERS, MAX_MIDI_VOLUME_POINTS,
    MAX_MIDI_VOLUME_POINTS_PER_CLIP, MidiClipGain, MidiClipVolume, MidiClipVolumePoint,
    MidiNoteSnapshot, MidiVoiceGain, MidiVoiceGainLayer, SessionCellContentSnapshot, Snapshot,
    TrackContentSnapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    beat: u64,
    incoming: f64,
    value: f64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Layer {
    start: u64,
    end: u64,
    millidb: i32,
    points: Vec<Point>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    note: u64,
    layers: Vec<Layer>,
}

fn visit(
    snapshot: &mut Snapshot,
    mut f: impl FnMut(&mut MidiNoteSnapshot, u64) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(_) => continue,
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        };
        for clip in clips {
            for note in &mut clip.notes {
                f(note, clip.source_length_raw)?;
            }
        }
    }
    for cell in &mut snapshot.session.cells {
        if let SessionCellContentSnapshot::Midi(clip) = &mut cell.content {
            for note in &mut clip.source.notes {
                f(note, clip.source.source_length_raw)?;
            }
        }
    }
    for source in &mut snapshot.aliases.sources {
        if let AliasContentSnapshot::Midi {
            source_length_raw,
            notes,
        } = &mut source.content
        {
            for note in notes {
                f(note, *source_length_raw)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn require_empty(project: &Project) -> Result<(), StorageError> {
    let mut snapshot = project.snapshot();
    visit(&mut snapshot, |note, _| {
        if !note.voice_gain.layers().is_empty() {
            return Err(StorageError::Invalid(
                "older schema cannot save midi voice gains",
            ));
        }
        Ok(())
    })
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    let mut total_points = 0usize;
    visit(&mut snapshot, |note, source_length| {
        if note.voice_gain.layers().is_empty() {
            return Ok(());
        }
        note.voice_gain
            .validate_source_length(BeatTime::from_raw(source_length))
            .map_err(|error| StorageError::Invalid(error.0))?;
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(note.voice_gain.layers().len())
            .map_err(|_| StorageError::Invalid("could not reserve midi voice gain layers"))?;
        for layer in note.voice_gain.layers() {
            total_points = total_points
                .checked_add(layer.volume().points().len())
                .filter(|count| *count <= MAX_MIDI_VOLUME_POINTS)
                .ok_or(StorageError::Invalid("midi voice gain point limit reached"))?;
            let mut points = Vec::new();
            points
                .try_reserve_exact(layer.volume().points().len())
                .map_err(|_| StorageError::Invalid("could not reserve midi voice gain points"))?;
            for p in layer.volume().points() {
                points.push(Point {
                    beat: p.beat.raw(),
                    incoming: p.incoming.linear(),
                    value: p.value.linear(),
                });
            }
            layers.push(Layer {
                start: layer.start().raw(),
                end: layer.end().raw(),
                millidb: layer.volume().clip_gain().millidb(),
                points,
            });
        }
        rows.try_reserve(1)
            .map_err(|_| StorageError::Invalid("could not reserve midi voice gains"))?;
        rows.push(Row {
            note: note.id,
            layers,
        });
        note.voice_gain = MidiVoiceGain::default();
        Ok(())
    })?;
    rows.sort_unstable_by_key(|row| row.note);
    let neutral =
        Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::midi_clip_gain::encode(&neutral)?;
    value["midi_voice_gains"] = serde_json::to_value(rows)?;
    value["version"] = 63.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    Project::from_snapshot(decode_snapshot(bytes)?).map_err(|e| StorageError::Invalid(e.0))
}

fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    if value.get("version").and_then(Value::as_u64) != Some(63) {
        return Err(StorageError::Invalid("invalid midi voice gain schema"));
    }
    let raw = value
        .as_object_mut()
        .and_then(|object| object.remove("midi_voice_gains"))
        .ok_or(StorageError::Invalid("midi voice gain inventory missing"))?;
    value["version"] = 62.into();
    let project = super::midi_clip_gain::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = project.snapshot();
    let mut note_count = 0usize;
    visit(&mut snapshot, |_, _| {
        note_count += 1;
        Ok(())
    })?;
    let entries = raw
        .as_array()
        .ok_or(StorageError::Invalid("invalid midi voice gain inventory"))?;
    if entries.len() > note_count {
        return Err(StorageError::Invalid(
            "midi voice gain inventory exceeds note count",
        ));
    }
    let mut point_count = 0usize;
    for entry in entries {
        let layers = entry
            .get("layers")
            .and_then(Value::as_array)
            .ok_or(StorageError::Invalid("invalid midi voice gain layers"))?;
        if layers.is_empty() || layers.len() > MAX_MIDI_VOICE_GAIN_LAYERS {
            return Err(StorageError::Invalid("invalid midi voice gain layer count"));
        }
        for layer in layers {
            let points = layer
                .get("points")
                .and_then(Value::as_array)
                .ok_or(StorageError::Invalid("invalid midi voice gain points"))?;
            if points.len() > MAX_MIDI_VOLUME_POINTS_PER_CLIP {
                return Err(StorageError::Invalid("midi voice gain point limit reached"));
            }
            point_count = point_count
                .checked_add(points.len())
                .filter(|n| *n <= MAX_MIDI_VOLUME_POINTS)
                .ok_or(StorageError::Invalid("midi voice gain point limit reached"))?;
        }
    }
    let rows: Vec<Row> = serde_json::from_value(raw)?;
    let mut remaining = BTreeMap::new();
    for row in rows {
        if row.note == 0 || remaining.insert(row.note, row.layers).is_some() {
            return Err(StorageError::Invalid(
                "invalid or duplicate midi voice gain owner",
            ));
        }
    }
    visit(&mut snapshot, |note, source_length| {
        let Some(rows) = remaining.remove(&note.id) else {
            return Ok(());
        };
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(rows.len())
            .map_err(|_| StorageError::Invalid("could not reserve midi voice gain layers"))?;
        for row in rows {
            let length = row
                .end
                .checked_sub(row.start)
                .filter(|n| *n > 0)
                .ok_or(StorageError::Invalid("invalid midi voice gain interval"))?;
            let mut points = Vec::new();
            points
                .try_reserve_exact(row.points.len())
                .map_err(|_| StorageError::Invalid("could not reserve midi voice gain points"))?;
            for p in row.points {
                points.push(MidiClipVolumePoint {
                    beat: BeatTime::from_raw(p.beat),
                    incoming: MidiClipGain::new(p.incoming)
                        .map_err(|e| StorageError::Invalid(e.0))?,
                    value: MidiClipGain::new(p.value).map_err(|e| StorageError::Invalid(e.0))?,
                });
            }
            let volume = MidiClipVolume::new(points, BeatTime::from_raw(length))
                .map_err(|e| StorageError::Invalid(e.0))?
                .with_clip_gain(
                    ClipGain::new(row.millidb).map_err(|e| StorageError::Invalid(e.0))?,
                );
            layers.push(
                MidiVoiceGainLayer::new(
                    BeatTime::from_raw(row.start),
                    BeatTime::from_raw(row.end),
                    BeatTime::from_raw(source_length),
                    volume,
                )
                .map_err(|e| StorageError::Invalid(e.0))?,
            );
        }
        note.voice_gain = MidiVoiceGain::new(layers).map_err(|e| StorageError::Invalid(e.0))?;
        Ok(())
    })?;
    if !remaining.is_empty() {
        return Err(StorageError::Invalid("midi voice gain owner missing"));
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentic_schema62_migrates_at_unity_and_round_trips_current() {
        let original = include_bytes!("../../tests/fixtures/midi-voice-gain/schema62.json");
        let project = crate::decode(original).unwrap();
        let token =
            crate::FileToken::from_validated_bytes(std::sync::Arc::from(original.as_slice()));
        assert!(token.requires_format_upgrade());
        let clip = &project.tracks()[0].midi_clips()[0];
        assert_eq!(clip.volume().clip_gain().millidb(), -6_000);
        assert_eq!(clip.volume().points()[0].value.linear(), 0.5);
        assert_eq!(clip.notes()[0].velocity(), 90);
        assert!(clip.notes()[0].voice_gain().layers().is_empty());
        let bytes = crate::encode(&project).unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["version"], 63);
        assert_eq!(json["midi_voice_gains"], serde_json::json!([]));
        assert_eq!(
            crate::decode(&bytes).unwrap().snapshot(),
            project.snapshot()
        );
    }
    #[test]
    fn malformed_inventory_and_unknown_or_duplicate_owners_refuse() {
        let project = crate::decode(include_bytes!(
            "../../tests/fixtures/midi-voice-gain/schema62.json"
        ))
        .unwrap();
        let original: Value = serde_json::from_slice(&crate::encode(&project).unwrap()).unwrap();
        let decode = |value: &Value| crate::decode(&serde_json::to_vec(value).unwrap());
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove("midi_voice_gains");
        assert!(decode(&missing).is_err());
        let row = serde_json::json!({"note":u64::MAX,"layers":[{"start":0,"end":1,"millidb":0,"points":[]}]});
        let mut unknown = original.clone();
        unknown["midi_voice_gains"] = serde_json::json!([row]);
        assert!(
            decode(&unknown)
                .unwrap_err()
                .to_string()
                .contains("owner missing")
        );
        let mut duplicate = original.clone();
        let id = project.tracks()[0].midi_clips()[0].notes()[0].id().value();
        let row =
            serde_json::json!({"note":id,"layers":[{"start":0,"end":1,"millidb":0,"points":[]}]});
        duplicate["midi_voice_gains"] = serde_json::json!([row.clone(), row]);
        assert!(
            decode(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        let mut empty = original.clone();
        empty["midi_voice_gains"] = serde_json::json!([{"note":id,"layers":[]}]);
        assert!(decode(&empty).is_err());
        let mut invalid = original.clone();
        invalid["midi_voice_gains"] = serde_json::json!([{"note":id,"layers":[{"start":0,"end":1,"millidb":0,"points":[{"beat":0,"incoming":0.0,"value":5.0}]}]}]);
        assert!(
            decode(&invalid)
                .unwrap_err()
                .to_string()
                .contains("clip volume")
        );
        let mut outside = original.clone();
        outside["midi_voice_gains"] = serde_json::json!([{"note":id,"layers":[{"start":0,"end":u64::MAX,"millidb":0,"points":[]}]}]);
        assert!(
            decode(&outside)
                .unwrap_err()
                .to_string()
                .contains("exceeds midi source")
        );
        let mut extra = original.clone();
        extra["midi_voice_gains"] = serde_json::json!([{"note":id,"layers":[{"start":0,"end":1,"millidb":0,"points":[],"unexpected":true}]}]);
        assert!(decode(&extra).is_err());
        let mut unsupported = original;
        unsupported["midi_voice_gains"] =
            serde_json::json!([{"note":id,"layers":[{"start":0,"end":1,"millidb":0,"points":[]}]}]);
        assert!(
            decode(&unsupported)
                .unwrap_err()
                .to_string()
                .contains("not yet supported")
        );
    }
    #[test]
    fn nonempty_inventory_decodes_to_exact_arrangement_and_session_note_owners() {
        let project = crate::decode(include_bytes!(
            "../../tests/fixtures/midi-voice-gain/schema62.json"
        ))
        .unwrap();
        let mut json: Value = serde_json::from_slice(&crate::encode(&project).unwrap()).unwrap();
        let arrangement = project.tracks()[0].midi_clips()[0].notes()[0].id().value();
        let original_snapshot = project.snapshot();
        let SessionCellContentSnapshot::Midi(session) = &original_snapshot.session.cells[0].content
        else {
            panic!("session")
        };
        let session_id = session.source.notes[0].id;
        let end = BeatTime::from_quarters(4).unwrap().raw();
        json["midi_voice_gains"] = serde_json::json!([
            {"note":arrangement,"layers":[{"start":0,"end":end,"millidb":-6000,"points":[
                {"beat":0,"incoming":0.25,"value":0.25},
                {"beat":end,"incoming":0.5,"value":1.0}
            ]}]},
            {"note":session_id,"layers":[{"start":0,"end":end,"millidb":6000,"points":[]}]}
        ]);
        let bytes = serde_json::to_vec(&json).unwrap();
        let snapshot = decode_snapshot(&bytes).unwrap();
        let clips = match &snapshot.tracks[0].content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            _ => panic!("midi track"),
        };
        let layer = &clips[0].notes[0].voice_gain.layers()[0];
        assert_eq!(layer.volume().clip_gain().millidb(), -6000);
        assert_eq!(layer.volume().points()[1].incoming.linear(), 0.5);
        assert_eq!(layer.volume().points()[1].value.linear(), 1.0);
        assert_eq!(layer.curve_gain_at(BeatTime::from_raw(end)).linear(), 0.5);
        let SessionCellContentSnapshot::Midi(session) = &snapshot.session.cells[0].content else {
            panic!("session")
        };
        assert_eq!(
            session.source.notes[0].voice_gain.layers()[0]
                .volume()
                .clip_gain()
                .millidb(),
            6000
        );
        // Full admission stays guarded until edit and render consumers are ready.
        assert!(
            crate::decode(&bytes)
                .unwrap_err()
                .to_string()
                .contains("not yet supported")
        );
    }
}
