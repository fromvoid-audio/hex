//! Schema67 device-state gating: the historical schema66 fixture saves
//! unchanged, a crossfaded Sampler state (v3) needs schema67 and overwrite
//! consent, and older schemas refuse newer states.
use super::*;
use crate::{FileToken, decode, encode, schema_version_for};
use hex_project::{Command, Editor};

const SCHEMA66: &[u8] = include_bytes!("../../../tests/fixtures/sampler-crossfade/schema66.json");
const SAMPLER: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sampler");

fn json_objects_sort_keys() -> bool {
    let mut map = serde_json::Map::new();
    map.insert("b".into(), Value::Null);
    map.insert("a".into(), Value::Null);
    serde_json::to_string(&map).unwrap() == r#"{"a":null,"b":null}"#
}

/// (state version, payload length) of each track's sampler instrument.
fn samplers(project: &Project) -> Vec<(u32, usize)> {
    project
        .tracks()
        .iter()
        .filter_map(|track| track.instrument())
        .map(hex_project::Device::state)
        .filter(|state| state.type_id() == SAMPLER)
        .map(|state| (state.state_version(), state.payload().len()))
        .collect()
}

/// The fixture with its loaded sampler re-authored as v3: the v2 bytes plus
/// a trailing 12.5 ms crossfade (opaque here, as in the device codec).
fn with_crossfade() -> Project {
    let mut editor = Editor::new(decode(SCHEMA66).unwrap(), 64).unwrap();
    let device = editor
        .project()
        .tracks()
        .iter()
        .filter_map(|track| track.instrument())
        .find(|device| device.state().payload().len() == 96)
        .unwrap();
    let mut payload = device.state().payload().to_vec();
    payload.extend_from_slice(&12.5_f64.to_le_bytes());
    let state = NativeDeviceState::new(SAMPLER, 3, payload)
        .unwrap()
        .with_host_metadata_from(device.state())
        .unwrap();
    let device = device.id();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    editor.project().clone()
}

#[test]
fn historical_schema66_sampler_project_saves_unchanged_with_v2_states() {
    let project = decode(SCHEMA66).unwrap();
    assert_eq!(samplers(&project), [(2, 96), (2, 5)]);
    assert!(!contains_newer_states(&project));
    assert_eq!(schema_version_for(&project), 66);
    let token = FileToken::from_validated_bytes(SCHEMA66.into());
    assert!(!token.requires_save_confirmation(&project).unwrap());
    let saved = encode(&project).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&saved).unwrap(),
        serde_json::from_slice::<Value>(SCHEMA66).unwrap()
    );
    if json_objects_sort_keys() {
        assert_eq!(saved, SCHEMA66, "byte-identical to the historical file");
    }
}

#[test]
fn crossfaded_sampler_saves_as_schema67_round_trips_and_needs_consent() {
    let project = with_crossfade();
    assert_eq!(samplers(&project), [(3, 104), (2, 5)]);
    assert!(contains_newer_states(&project));
    assert_eq!(schema_version_for(&project), 67);
    let saved = encode(&project).unwrap();
    let value: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(value["version"], 67);
    let loaded = decode(&saved).unwrap();
    assert_eq!(loaded.snapshot(), project.snapshot());
    assert_eq!(encode(&loaded).unwrap(), saved);
    // Overwriting the schema66 file loses older-reader compatibility.
    let older = FileToken::from_validated_bytes(SCHEMA66.into());
    assert!(older.requires_save_confirmation(&project).unwrap());
    let current = FileToken::from_validated_bytes(saved.clone().into());
    assert!(!current.requires_save_confirmation(&project).unwrap());
    // Turning the crossfade off (a v2 state again) saves the older layout.
    let reverted = decode(SCHEMA66).unwrap();
    assert!(!current.requires_save_confirmation(&reverted).unwrap());
    assert_eq!(schema_version_for(&reverted), 66);
    for writer in [crate::encode_schema22, crate::encode_schema20] {
        assert!(writer(&project).is_err());
    }
}

#[test]
fn older_schemas_refuse_newer_device_states_and_67_reads_older_states() {
    let saved = encode(&with_crossfade()).unwrap();
    let relabel = |bytes: &[u8], version: u32| {
        let mut value: Value = serde_json::from_slice(bytes).unwrap();
        value["version"] = version.into();
        decode(&serde_json::to_vec(&value).unwrap())
    };
    assert!(relabel(&saved, 66).is_err());
    assert!(relabel(&saved, 68).is_err());
    assert!(decode(&saved).is_ok());
    // A schema67 document without a newer state still opens, and saves back
    // in its older layout.
    let older = relabel(SCHEMA66, 67).unwrap();
    assert_eq!(older.snapshot(), decode(SCHEMA66).unwrap().snapshot());
    assert_eq!(schema_version_for(&older), 66);
}
