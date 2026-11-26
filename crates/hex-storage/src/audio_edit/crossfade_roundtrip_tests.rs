//! The new overlap semantics use a fresh format; pre51 remains strictly disjoint.
use hex_project::{Command, Created, Editor, Project, TimeRange};

#[allow(dead_code)]
#[path = "../../tests/common/mod.rs"]
mod common;

fn fixture() -> (Editor, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "metadata-only.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("fixture import")
    };
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase { clip, phase: 16 })
        .unwrap();
    editor
        .execute(Command::CopyClipTo {
            clip,
            track,
            start: 96_000,
        })
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("main fixture device")
    };
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(7),
            points: vec![hex_project::AutomationPoint {
                beat: hex_project::BeatTime::ZERO,
                value: hex_project::AutomationValue::new(0.25).unwrap(),
                outgoing: hex_project::AutomationInterpolation::Linear,
            }],
        })
        .unwrap();
    (editor, track)
}
#[test]
fn current_crossfade_roundtrip_and_historical_disjoint_migration_are_complete() {
    let (mut editor, track) = fixture();
    let before = editor.project().snapshot();
    let disjoint = crate::encode(editor.project()).unwrap();
    // Schema51 changes only overlap admission; the retained frozen50 writer
    // has the identical field layout. This disjoint document remains valid50.
    let mut old: serde_json::Value = serde_json::from_slice(&disjoint).unwrap();
    common::remove_empty_host_automation(&mut old);
    common::remove_empty_aliases(&mut old);
    old["version"] = 50.into();
    let bytes = serde_json::to_vec(&old).unwrap();
    let saved_bytes = bytes.clone();
    let migrated = crate::decode(&bytes).unwrap();
    assert_eq!(migrated.snapshot(), before);
    assert_eq!(bytes, saved_bytes);
    assert_eq!(
        crate::decode(&crate::encode(&migrated).unwrap())
            .unwrap()
            .snapshot(),
        before
    );
    editor
        .execute(Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 92_000, 100_000).unwrap(),
        })
        .unwrap();
    let after = editor.project().snapshot();
    let encoded = crate::encode(editor.project()).unwrap();
    let current: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        current["version"],
        crate::CAPABILITY_MANIFEST_SCHEMA_VERSION
    );
    assert!(
        current["capabilities"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("s_curve_fades"))
    );
    let historical = crate::encode_stacked_legacy(editor.project()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&historical).unwrap();
    assert_eq!(value["version"], crate::S_CURVE_FADE_SCHEMA_VERSION);
    assert_eq!(crate::decode(&historical).unwrap().snapshot(), after);
    assert_eq!(crate::decode(&encoded).unwrap().snapshot(), after);
    // The envelope's underlying overlap document cannot be relabeled as a
    // pre51 document to bypass the old disjointness rule.
    let mut value = value["base"].clone();
    assert_eq!(value["version"], crate::CURRENT_SCHEMA_VERSION);
    common::remove_empty_host_automation(&mut value);
    common::remove_empty_aliases(&mut value);
    value["version"] = 50.into();
    assert!(
        crate::decode(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .to_string()
            .contains("older schema")
    );
    assert!(crate::encode_schema22(editor.project()).is_err());
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}
