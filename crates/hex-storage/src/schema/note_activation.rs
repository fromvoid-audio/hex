//! Schema 56 stores inactive MIDI identities without changing the frozen 55
//! representation. Older projects contain only active notes.
use crate::{Project, StorageError};
use hex_project::{
    AliasContentSnapshot, MidiNoteSnapshot, SessionCellContentSnapshot, Snapshot,
    TrackContentSnapshot,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn visit_notes(snapshot: &mut Snapshot, mut visit: impl FnMut(&mut MidiNoteSnapshot)) {
    for source in &mut snapshot.aliases.sources {
        if let AliasContentSnapshot::Midi { notes, .. } = &mut source.content {
            for note in notes {
                visit(note);
            }
        }
    }
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(_) => continue,
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        };
        for clip in clips {
            for note in &mut clip.notes {
                visit(note);
            }
        }
    }
    for cell in &mut snapshot.session.cells {
        if let SessionCellContentSnapshot::Midi(clip) = &mut cell.content {
            for note in &mut clip.source.notes {
                visit(note);
            }
        }
    }
}

pub(crate) fn require_all_active(project: &Project) -> Result<(), StorageError> {
    let mut snapshot = project.snapshot();
    let mut inactive = false;
    visit_notes(&mut snapshot, |note| inactive |= !note.active);
    if inactive {
        Err(StorageError::Invalid(
            "older schema cannot save inactive midi notes",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    super::event_activation::require_all_active(project)?;
    let mut snapshot = project.snapshot();
    let mut inactive = Vec::new();
    visit_notes(&mut snapshot, |note| {
        if !note.active {
            inactive.push(note.id);
            note.active = true;
        }
    });
    let mut value = if inactive.is_empty() {
        super::automation_read::encode(project)?
    } else {
        let neutral = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
        super::automation_read::encode(&neutral)?
    };
    inactive.sort_unstable();
    value["inactive_midi_notes"] = serde_json::to_value(inactive)?;
    value["version"] = 56.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: Value) -> Result<Project, StorageError> {
    if value.get("version").and_then(Value::as_u64) != Some(56) {
        return Err(StorageError::Invalid("invalid midi note activation schema"));
    }
    let ids: Vec<u64> = serde_json::from_value(
        value
            .as_object_mut()
            .and_then(|v| v.remove("inactive_midi_notes"))
            .ok_or(StorageError::Invalid(
                "inactive midi note inventory missing",
            ))?,
    )?;
    if ids.len() > hex_project::MAX_NOTES || ids.contains(&0) {
        return Err(StorageError::Invalid(
            "invalid inactive midi note inventory",
        ));
    }
    let mut inactive: BTreeSet<_> = ids.iter().copied().collect();
    if inactive.len() != ids.len() {
        return Err(StorageError::Invalid("duplicate inactive midi note"));
    }
    value["version"] = 55.into();
    let project = super::automation_read::decode_value(value)?;
    if inactive.is_empty() {
        return Ok(project);
    }
    let mut snapshot = project.snapshot();
    visit_notes(&mut snapshot, |note| {
        if inactive.remove(&note.id) {
            note.active = false;
        }
    });
    if !inactive.is_empty() {
        return Err(StorageError::Invalid("inactive midi note owner missing"));
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, Editor};

    fn fixture() -> Project {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "notes".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        editor.project().clone()
    }

    #[test]
    fn current_round_trip_and_frozen_reader_preserve_activation() {
        let active = fixture();
        let legacy = super::super::automation_read::encode(&active).unwrap();
        let migrated = crate::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(migrated.snapshot(), active.snapshot());
        let mut snapshot = active.snapshot();
        visit_notes(&mut snapshot, |note| note.active = false);
        let inactive = Project::from_snapshot(snapshot).unwrap();
        let encoded = crate::encode(&inactive).unwrap();
        assert_eq!(
            crate::decode(&encoded).unwrap().snapshot(),
            inactive.snapshot()
        );
        assert!(super::super::automation_read::encode(&inactive).is_err());
        assert!(crate::encode_schema22(&inactive).is_err());
        assert!(crate::encode_schema20(&inactive).is_err());
    }

    #[test]
    fn missing_duplicate_and_unknown_inventory_refuse() {
        let project = fixture();
        let value: Value = serde_json::from_slice(&crate::encode(&project).unwrap()).unwrap();
        let mut missing = value.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("inactive_midi_notes");
        assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
        let mut unknown = value.clone();
        unknown["inactive_midi_notes"] = serde_json::json!([u64::MAX]);
        assert!(crate::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
        let mut duplicate = value;
        duplicate["inactive_midi_notes"] = serde_json::json!([1, 1]);
        assert!(crate::decode(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    }

    #[test]
    fn aliases_and_session_notes_survive_inactive_round_trip() {
        let mut editor = Editor::new(fixture(), 16).unwrap();
        let track = editor.project().tracks()[0].id();
        let arrangement = editor.project().tracks()[0].midi_clips()[0].id();
        editor
            .execute(Command::DuplicateAliasClips(vec![arrangement]))
            .unwrap();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "session".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(session) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "cell".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip: session,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 64,
                channel: 1,
                velocity: 90,
            })
            .unwrap();
        let mut snapshot = editor.project().snapshot();
        let mut count = 0;
        visit_notes(&mut snapshot, |note| {
            note.active = false;
            count += 1;
        });
        assert_eq!(count, 2, "alias source stored once plus Session note");
        let project = Project::from_snapshot(snapshot).unwrap();
        let bytes = crate::encode(&project).unwrap();
        assert_eq!(
            crate::decode(&bytes).unwrap().snapshot(),
            project.snapshot()
        );
    }
}
