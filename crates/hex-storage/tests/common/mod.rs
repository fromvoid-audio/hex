use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Restore the frozen inline-instrument representation when comparing a current
/// writer with an evidenced pre-binding document. Later cycle/automation layers
/// must be empty; device IDs, states, references and placements are reconstructed
/// from the canonical source inventory.
pub fn restore_pre_binding_instruments(value: &mut Value) -> usize {
    remove_audio_cycles(value);
    let sources = value["native_sources"].as_array().unwrap().clone();
    let bindings = value["track_source_bindings"].as_array().unwrap().clone();
    let mut tracks = std::mem::take(value["tracks"].as_array_mut().unwrap());
    assert_eq!(tracks.len(), bindings.len());
    let mut promoted = 0;
    let mut assets = BTreeMap::new();
    for row in std::mem::take(value["device_assets"].as_array_mut().unwrap()) {
        assert!(
            assets
                .insert(row["device"].as_u64().unwrap(), row)
                .is_none()
        );
    }
    for (track, binding) in tracks.iter_mut().zip(bindings) {
        assert_eq!(track["id"], binding["track"]);
        match (
            binding["source"].as_u64(),
            binding["instrument_position"].as_u64(),
        ) {
            (None, None) => {
                assert!(track["instrument"].is_null());
                assert!(track["instrument_position"].is_null());
            }
            (Some(source_id), Some(position)) => {
                let source = sources
                    .iter()
                    .find(|source| source["id"].as_u64() == Some(source_id))
                    .unwrap();
                let index = source["instrument_index"].as_u64().unwrap() as usize;
                let device = &source["devices"][index];
                track["instrument"] = device["device"].clone();
                track["instrument_position"] = json!(position);
                let id = device["device"]["id"].as_u64().unwrap();
                assert!(
                    assets
                        .insert(
                            id,
                            json!({
                                "device": id,
                                "references": device["references"],
                                "comparison": device["comparison"],
                            }),
                        )
                        .is_none()
                );
                promoted += 1;
            }
            _ => panic!("source and instrument position must both be null or present"),
        }
    }
    let mut ordered_assets = Vec::new();
    for track in &tracks {
        for device in track["devices"].as_array().unwrap() {
            ordered_assets.push(assets.remove(&device["id"].as_u64().unwrap()).unwrap());
        }
        if !track["instrument"].is_null() {
            ordered_assets.push(
                assets
                    .remove(&track["instrument"]["id"].as_u64().unwrap())
                    .unwrap(),
            );
        }
    }
    for track in value["return_tracks"].as_array().unwrap() {
        for device in track["devices"].as_array().unwrap() {
            ordered_assets.push(assets.remove(&device["id"].as_u64().unwrap()).unwrap());
        }
    }
    for device in value["main_devices"].as_array().unwrap() {
        ordered_assets.push(assets.remove(&device["id"].as_u64().unwrap()).unwrap());
    }
    assert!(
        assets.is_empty(),
        "current device manifest has an unknown owner"
    );
    value["device_assets"] = Value::Array(ordered_assets);
    value["tracks"] = Value::Array(std::mem::take(&mut tracks));
    assert_eq!(
        sources.len(),
        promoted,
        "historical comparison cannot discard independent retained sources"
    );
    value.as_object_mut().unwrap().remove("native_sources");
    value
        .as_object_mut()
        .unwrap()
        .remove("track_source_bindings");
    let current_next_id = value["next_id"].as_u64().unwrap();
    value["next_id"] = json!(current_next_id.checked_sub(promoted as u64).unwrap());
    promoted
}

/// Historical comparisons may omit only an actually empty alias inventory.
pub fn remove_empty_aliases(value: &mut Value) {
    for field in ["alias_sources", "alias_members"] {
        if let Some(rows) = value.as_object_mut().unwrap().remove(field) {
            assert_eq!(
                rows,
                json!([]),
                "historical comparison cannot discard clip aliases"
            );
        }
    }
}

/// Historical comparisons may omit only an actually empty schema53 host inventory.
pub fn remove_empty_host_automation(value: &mut Value) {
    if let Some(rows) = value
        .as_object_mut()
        .unwrap()
        .remove("automatic_track_names")
    {
        assert_eq!(
            rows,
            json!([]),
            "historical comparison cannot discard automatic track names"
        );
    }
    if let Some(rows) = value.as_object_mut().unwrap().remove("midi_clip_gains") {
        assert_eq!(
            rows,
            json!([]),
            "historical comparison cannot discard midi clip gain"
        );
    }
    if let Some(rows) = value.as_object_mut().unwrap().remove("audio_event_gains") {
        assert_eq!(
            rows,
            json!([]),
            "historical comparison cannot discard event gain"
        );
    }
    if let Some(rows) = value.as_object_mut().unwrap().remove("nested_audio") {
        assert!(
            rows.as_array()
                .unwrap()
                .iter()
                .all(|row| row["state"].is_null()),
            "historical comparison cannot discard nested retained sources"
        );
    }
    for field in ["inactive_midi_notes", "inactive_audio_events"] {
        if let Some(rows) = value.as_object_mut().unwrap().remove(field) {
            assert_eq!(
                rows,
                json!([]),
                "historical comparison cannot discard activation"
            );
        }
    }
    // A historical projection may erase only the actually equivalent authored
    // selection. Base would change playback and must never be discarded.
    for inventory in ["device_automation", "host_automation"] {
        // Some historical comparisons have already asserted and removed the
        // empty provider inventory before removing the newer host layer.
        let Some(rows) = value.get_mut(inventory) else {
            continue;
        };
        for row in rows.as_array_mut().unwrap() {
            assert_eq!(
                row.as_object_mut().unwrap().remove("read_selection"),
                Some(json!("authored")),
                "historical comparison cannot discard base read selection"
            );
        }
    }

    assert_eq!(
        value.get("host_automation"),
        Some(&json!([])),
        "historical comparison cannot discard host automation"
    );
    value.as_object_mut().unwrap().remove("host_automation");
}

/// Remove the schema48 cycle layer and any newer empty automation inventory
/// when constructing a frozen pre-cycle envelope. Never discard authored data.
pub fn remove_audio_cycles(value: &mut Value) {
    remove_empty_host_automation(value);
    remove_empty_aliases(value);
    if let Some(automation) = value.as_object_mut().unwrap().remove("device_automation") {
        assert_eq!(
            automation,
            json!([]),
            "historical comparison cannot discard device automation"
        );
    }
    let cycles = value
        .as_object_mut()
        .unwrap()
        .remove("session_audio_cycles")
        .unwrap();
    assert!(
        cycles
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["cycle"].is_null()),
        "historical comparison cannot discard audio cycles"
    );
}
