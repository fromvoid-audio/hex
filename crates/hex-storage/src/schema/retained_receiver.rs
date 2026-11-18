//! Schema68 adds retained receiver path ownership to the schema67 layout.
//! Older schemas carry no such state and migrate with an empty inventory.
//! The field is emitted only when needed; earlier readers refuse schema68.
use crate::{Project, StorageError};
use serde_json::Value;

pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .any(hex_project::Track::retained_source_path)
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let paths = std::mem::take(&mut snapshot.retained_source_paths);
    let neutral =
        Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::sampler_crossfade::encode(&neutral)?;
    value["retained_source_paths"] = serde_json::to_value(paths)?;
    value["version"] = crate::RETAINED_RECEIVER_SCHEMA_VERSION.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    if value.get("version").and_then(Value::as_u64)
        != Some(crate::RETAINED_RECEIVER_SCHEMA_VERSION.into())
    {
        return Err(StorageError::Invalid("invalid retained receiver schema"));
    }
    let paths = value
        .as_object_mut()
        .and_then(|object| object.remove("retained_source_paths"))
        .ok_or(StorageError::Invalid(
            "retained source path inventory missing",
        ))?;
    value["version"] = crate::DEVICE_STATE_SCHEMA_VERSION.into();
    let neutral = super::sampler_crossfade::decode(&serde_json::to_vec(&value)?)?;
    if paths
        .as_array()
        .is_none_or(|rows| rows.len() > neutral.tracks().len())
    {
        return Err(StorageError::Invalid(
            "invalid retained source path inventory",
        ));
    }
    let mut snapshot = neutral.snapshot();
    snapshot.retained_source_paths = serde_json::from_value(paths)?;
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    fn fixture() -> (Project, u64, u64) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        editor
            .execute(Command::InsertInstrument {
                track: receiver,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let instrument = editor
            .project()
            .track(receiver)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        for before in [Some(instrument), None] {
            editor
                .execute(Command::InsertDevice {
                    track: receiver,
                    before,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap();
        }
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(peer) = editor
            .execute(Command::AddMidiTrack {
                name: "peer".into(),
            })
            .unwrap()
        else {
            panic!("peer")
        };
        editor
            .execute(Command::BindTrackSource {
                track: peer,
                source: Some(source),
            })
            .unwrap();
        (editor.project().clone(), receiver.value(), peer.value())
    }

    #[test]
    fn retained_receiver_roundtrips_and_warns_on_older_overwrite() {
        let (original, receiver, _) = fixture();
        let old_bytes = crate::encode(&original).unwrap();
        let mut snapshot = original.snapshot();
        snapshot.retained_source_paths.push(receiver);
        let retained = Project::from_snapshot(snapshot).unwrap();
        let source = retained
            .track(retained.tracks()[0].id())
            .unwrap()
            .source_id()
            .unwrap();
        assert_eq!(retained.source_contributors(source).count(), 1);
        assert!(retained.tracks()[0].receives_source_audio());
        assert!(retained.tracks()[0].retained_source_path());
        assert_eq!(
            Editor::new(retained.clone(), 8).unwrap().project(),
            &retained
        );
        let bytes = crate::encode(&retained).unwrap();
        assert_eq!(crate::schema_version_for(&retained), 68);
        assert_eq!(
            crate::decode(&bytes).unwrap().snapshot(),
            retained.snapshot()
        );
        assert_eq!(
            crate::encode(&crate::decode(&bytes).unwrap()).unwrap(),
            bytes
        );
        let token = crate::FileToken::from_validated_bytes(old_bytes.into());
        assert!(token.requires_save_confirmation(&retained).unwrap());
        for writer in [crate::encode_schema22, crate::encode_schema20] {
            assert!(writer(&retained).is_err());
        }
    }

    #[test]
    fn historical_and_malformed_receiver_inventories_are_handled_strictly() {
        let (original, receiver, peer) = fixture();
        let old = crate::encode(&original).unwrap();
        assert!(
            crate::decode(&old)
                .unwrap()
                .snapshot()
                .retained_source_paths
                .is_empty()
        );
        let mut snapshot = original.snapshot();
        snapshot.retained_source_paths.push(receiver);
        let saved = crate::encode(&Project::from_snapshot(snapshot).unwrap()).unwrap();
        let value: Value = serde_json::from_slice(&saved).unwrap();
        for paths in [
            serde_json::json!([receiver, receiver]),
            serde_json::json!([peer]),
            serde_json::json!([u64::MAX]),
            serde_json::json!("invalid"),
        ] {
            let mut bad = value.clone();
            bad["retained_source_paths"] = paths;
            assert!(crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        let mut missing = value.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("retained_source_paths");
        assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
        let mut relabeled = value;
        relabeled["version"] = 67.into();
        assert!(crate::decode(&serde_json::to_vec(&relabeled).unwrap()).is_err());
    }

    #[test]
    fn retained_receiver_preserves_existing_newer_device_state_and_content_views() {
        let historical = include_bytes!("../../tests/fixtures/sampler-crossfade/schema66.json");
        let mut editor = Editor::new(crate::decode(historical).unwrap(), 16).unwrap();
        let (receiver, device, state) = {
            let track = editor
                .project()
                .tracks()
                .iter()
                .find(|track| {
                    track.receives_source_audio()
                        && track
                            .instrument()
                            .is_some_and(|d| d.state().payload().len() == 96)
                })
                .unwrap();
            let device = track.instrument().unwrap();
            let mut payload = device.state().payload().to_vec();
            payload.extend_from_slice(&12.5_f64.to_le_bytes());
            let state =
                hex_lattice_contract::NativeDeviceState::new(device.state().type_id(), 3, payload)
                    .unwrap()
                    .with_host_metadata_from(device.state())
                    .unwrap();
            (track.id(), device.id(), state)
        };
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        assert_eq!(crate::schema_version_for(editor.project()), 67);
        let mut snapshot = editor.project().snapshot();
        snapshot.retained_source_paths.push(receiver.value());
        let retained = Project::from_snapshot(snapshot).unwrap();
        let saved = crate::encode(&retained).unwrap();
        assert_eq!(crate::schema_version_for(&retained), 68);
        assert_eq!(
            crate::decode(&saved).unwrap().snapshot(),
            retained.snapshot()
        );
    }
}
