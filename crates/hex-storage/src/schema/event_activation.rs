//! Schema57 records inactive audio events, scoped to their owning clip and
//! retained root. Older event schemas have no activation field.
use crate::{Project, StorageError};
use hex_project::{AudioClipEdit, AudioEvent, Snapshot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    clip: u64,
    /// Zero identifies the visible sequence; nonzero identifies a retained root.
    root: u64,
    event: u64,
}

fn visit_edit(
    edit: &mut AudioClipEdit,
    clip: u64,
    root: u64,
    visit: &mut impl FnMut(Key, &mut AudioEvent),
) {
    if let Some(events) = Arc::make_mut(&mut edit.domain).events.as_mut() {
        for event in &mut Arc::make_mut(events).events {
            visit(
                Key {
                    clip,
                    root,
                    event: event.id,
                },
                event,
            );
        }
    }
}

fn visit_project(
    project: &Project,
    snapshot: &mut Snapshot,
    mut visit: impl FnMut(Key, &mut AudioEvent),
) -> Result<(), StorageError> {
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            let Some(edit) = clip.edit.as_mut() else {
                continue;
            };
            visit_edit(edit, clip.id, 0, &mut visit);
            if let Some(events) = Arc::make_mut(&mut edit.domain).events.as_mut() {
                for root in &mut Arc::make_mut(events).sources {
                    let mut retained = root.clip.retained_snapshot();
                    if let Some(edit) = retained.edit.as_mut() {
                        visit_edit(edit, clip.id, root.id, &mut visit);
                    }
                    *root = project
                        .restore_retained_audio_root(root.id, retained, Arc::clone(&root.tempo))
                        .map_err(|error| StorageError::Invalid(error.0))?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn require_all_active(project: &Project) -> Result<(), StorageError> {
    let mut snapshot = project.snapshot();
    let mut inactive = false;
    visit_project(project, &mut snapshot, |_, event| inactive |= !event.active)?;
    if inactive {
        Err(StorageError::Invalid(
            "older schema cannot save inactive audio events",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    if project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| {
            clip.retained_audio_roots()
                .any(|root| root.clip.has_retained_audio_roots())
        })
    {
        return Err(StorageError::Invalid(
            "older schema cannot save nested retained audio",
        ));
    }
    let mut snapshot = project.snapshot();
    let mut inactive = Vec::new();
    visit_project(project, &mut snapshot, |key, event| {
        if !event.active {
            inactive.push(key);
            event.active = true;
        }
    })?;
    super::audio_alias_projection::synchronize(&mut snapshot)?;
    let neutral =
        Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::note_activation::encode(&neutral)?;
    inactive.sort_unstable();
    value["inactive_audio_events"] = serde_json::to_value(inactive)?;
    value["version"] = 57.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: Value) -> Result<Project, StorageError> {
    if value.get("version").and_then(Value::as_u64) != Some(57) {
        return Err(StorageError::Invalid(
            "invalid audio event activation schema",
        ));
    }
    let keys: Vec<Key> = serde_json::from_value(
        value
            .as_object_mut()
            .and_then(|value| value.remove("inactive_audio_events"))
            .ok_or(StorageError::Invalid(
                "inactive audio event inventory missing",
            ))?,
    )?;
    if keys.len()
        > hex_project::MAX_CLIPS
            .saturating_mul(hex_project::MAX_AUDIO_EVENTS)
            .saturating_mul(2)
        || keys.iter().any(|key| key.clip == 0 || key.event == 0)
    {
        return Err(StorageError::Invalid(
            "invalid inactive audio event inventory",
        ));
    }
    let mut inactive: BTreeSet<_> = keys.iter().copied().collect();
    if inactive.len() != keys.len() {
        return Err(StorageError::Invalid("duplicate inactive audio event"));
    }
    value["version"] = 56.into();
    let project = super::note_activation::decode_value(value)?;
    let mut snapshot = project.snapshot();
    visit_project(&project, &mut snapshot, |key, event| {
        if inactive.remove(&key) {
            event.active = false;
        }
    })?;
    if !inactive.is_empty() {
        return Err(StorageError::Invalid("inactive audio event owner missing"));
    }
    super::audio_alias_projection::synchronize(&mut snapshot)?;
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{AudioEventEdit, Command, Created, Editor};

    #[test]
    fn inactive_audio_aliases_round_trip_without_losing_shared_content() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("import");
        };
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetActive {
                    events: vec![1],
                    active: false,
                },
            })
            .unwrap();
        editor
            .execute(Command::DuplicateAliasClips(vec![clip]))
            .unwrap();
        let expected = editor.project().snapshot();
        let historical = encode(editor.project()).unwrap();
        assert_eq!(
            decode(&serde_json::to_vec(&historical).unwrap())
                .unwrap()
                .snapshot(),
            expected
        );
        let current = crate::encode(editor.project()).unwrap();
        assert_eq!(crate::decode(&current).unwrap().snapshot(), expected);
        let mut inconsistent = historical;
        let inactive = inconsistent["inactive_audio_events"]
            .as_array_mut()
            .unwrap();
        assert_eq!(inactive.len(), 2);
        inactive.pop();
        assert!(decode(&serde_json::to_vec(&inconsistent).unwrap()).is_err());
    }

    #[test]
    fn previous_schema_migrates_audio_events_active() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 50 },
            })
            .unwrap();
        let old = super::super::note_activation::encode(editor.project()).unwrap();
        assert_eq!(old["version"], 56);
        let migrated = crate::decode(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(migrated, *editor.project());
        assert!(
            migrated
                .clip(clip)
                .unwrap()
                .audio_events()
                .events
                .iter()
                .all(|event| event.active)
        );
    }

    #[test]
    fn retains_scoped_inactive_events_and_refuses_damaged_inventories() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { track, asset, clip } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetActive {
                    events: vec![1],
                    active: false,
                },
            })
            .unwrap();
        let Created::Clip(second) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 150,
                duration: 100,
                source_offset: 0,
                source_length: 100,
                looping: false,
            })
            .unwrap()
        else {
            panic!("placement")
        };
        editor
            .execute(Command::SetClipActive {
                clip: second,
                active: false,
            })
            .unwrap();
        let Created::Clip(group) = editor
            .execute(Command::GroupClips(vec![clip, second]))
            .unwrap()
        else {
            panic!("group")
        };
        let project = editor.project();
        let value = encode(project).unwrap();
        assert_eq!(value["inactive_audio_events"].as_array().unwrap().len(), 2);
        assert_eq!(
            decode(&serde_json::to_vec(&value).unwrap()).unwrap(),
            *project
        );
        assert!(super::super::note_activation::encode(project).is_err());
        assert!(crate::encode_schema20(project).is_err());
        for case in 0..3 {
            let mut bad = value.clone();
            match case {
                0 => {
                    bad.as_object_mut().unwrap().remove("inactive_audio_events");
                }
                1 => {
                    let key = bad["inactive_audio_events"][0].clone();
                    bad["inactive_audio_events"]
                        .as_array_mut()
                        .unwrap()
                        .push(key);
                }
                _ => bad["inactive_audio_events"][0]["clip"] = (group.value() + 1000).into(),
            }
            assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
    }
}
