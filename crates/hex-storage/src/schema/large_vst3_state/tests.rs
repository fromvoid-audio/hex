use super::*;

#[test]
fn compact_state_rejects_corruption_missing_slots_and_invalid_base_versions() {
    use hex_project::{Command, Created, Editor};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "historical vst3".into(),
        })
        .unwrap()
    else {
        panic!("expected track")
    };
    let state = NativeDeviceState::new(
        hex_plugin_state::TYPE_ID,
        9,
        vec![7; MAX_VST3_DEVICE_STATE_BYTES + 1],
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let bytes = crate::encode_stacked_legacy(editor.project()).unwrap();
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document["version"], LARGE_VST3_STATE_SCHEMA_VERSION);
    assert_eq!(crate::decode(&bytes).unwrap(), *editor.project());
    let refuses = |value: &serde_json::Value| {
        assert!(crate::decode(&serde_json::to_vec(value).unwrap()).is_err())
    };
    let rows = document["vst3_states"].take();
    document["vst3_states"] = serde_json::json!([]);
    refuses(&document);
    document["vst3_states"] = serde_json::json!([rows[0], rows[0]]);
    refuses(&document);
    document["vst3_states"] = rows;
    document["vst3_states"][0]["inactive"] = true.into();
    refuses(&document);
    document["vst3_states"][0]["inactive"] = false.into();
    document["base"]["version"] = crate::LARGE_VST3_STATE_SCHEMA_VERSION.into();
    refuses(&document);
    document = serde_json::from_slice(&bytes).unwrap();
    document["vst3_states"][0]["sha256"][0] =
        (document["vst3_states"][0]["sha256"][0].as_u64().unwrap() ^ 1).into();
    refuses(&document);
    document = serde_json::from_slice(&bytes).unwrap();
    document["vst3_states"][0]["payload"] = "!".into();
    refuses(&document);
    // One byte over the stored limit can have the same padded length as an
    // admissible payload. The decoded byte count still has to fit.
    let over = vec![0u8; hex_lattice_contract::MAX_STORED_VST3_DEVICE_STATE_BYTES + 1];
    let row = &mut document["vst3_states"][0];
    let device = row["device"].as_u64().unwrap();
    row["sha256"] = serde_json::json!(checksum(device, false, 9, &over));
    row["payload"] = STANDARD.encode(&over).into();
    let error = crate::decode(&serde_json::to_vec(&document).unwrap()).unwrap_err();
    assert!(matches!(
        error,
        crate::StorageError::Invalid("invalid vst3 project state payload")
    ));
}
