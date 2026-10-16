//! Arbitrary byte evidence, not generated Ableton-format or musical fixtures.
use hex_project::{Command, Created, Editor, ImportProvenance, MAX_IMPORT_DOCUMENT_BYTES, Project};
use hex_storage::{MAX_DOCUMENT_BYTES, decode, encode, load, save_new};
use serde_json::{Value, json};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn evidence(bytes: Vec<u8>) -> ImportProvenance {
    ImportProvenance::new("ableton-live-set", 1, 1, bytes).unwrap()
}
fn project(bytes: Vec<u8>) -> Project {
    let mut snapshot = Project::new(48_000).unwrap().snapshot();
    snapshot.import_provenance = Some(evidence(bytes));
    Project::from_snapshot(snapshot).unwrap()
}

#[test]
fn exact_original_bytes_survive_edits_history_and_save_reopen() {
    // Includes non-UTF8 data and unknown metadata; no XML mapping is claimed.
    let raw = b"unknown Private/Path\0\xff\r\n".to_vec();
    let original = project(raw.clone());
    let mut editor = Editor::new(original.clone(), 4).unwrap();
    editor.execute(Command::MasterGain { gain: 0.25 }).unwrap();
    let edited = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project(), &original);
    assert!(editor.redo());
    assert_eq!(editor.project(), &edited);
    assert_eq!(edited.import_provenance().unwrap().original_bytes(), raw);
    assert!(!format!("{edited:?}").contains("Private/Path"));
    let snapshot = edited.snapshot();
    assert_eq!(Project::from_snapshot(snapshot).unwrap(), edited);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hex-provenance-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("saved.hex");
    save_new(&path, &edited).unwrap();
    let saved = fs::read(&path).unwrap();
    let reopened = load(&path).unwrap();
    assert_eq!(reopened, edited);
    assert_eq!(reopened.import_provenance().unwrap().original_bytes(), raw);
    assert!(save_new(&path, &original).is_err());
    assert_eq!(fs::read(&path).unwrap(), saved);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bounded_envelopes_and_versions_are_strict() {
    assert!(ImportProvenance::new("ableton-live-set", 1, 1, vec![]).is_err());
    for (kind, version, adapter) in [
        ("other", 1, 1),
        ("ableton-live-set", 2, 1),
        ("ableton-live-set", 1, 2),
    ] {
        assert!(ImportProvenance::new(kind, version, adapter, vec![0]).is_err());
    }
    assert!(
        ImportProvenance::new(
            "ableton-live-set",
            1,
            1,
            vec![0; MAX_IMPORT_DOCUMENT_BYTES + 1]
        )
        .is_err()
    );
    let maximum = project(vec![255; MAX_IMPORT_DOCUMENT_BYTES]);
    let bytes = encode(&maximum).unwrap();
    assert!(bytes.len() < MAX_DOCUMENT_BYTES);
    assert_eq!(decode(&bytes).unwrap(), maximum);
    let base: Value = serde_json::from_slice(&encode(&project(vec![1])).unwrap()).unwrap();
    for (key, value) in [
        ("kind", json!("other")),
        ("version", json!(2)),
        ("adapter_version", json!(0)),
        ("original_bytes", json!([])),
        ("original_bytes", json!([256])),
        ("extra", json!(true)),
    ] {
        let mut invalid = base.clone();
        invalid["import_provenance"][key] = value;
        assert!(
            decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{key}"
        );
    }
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("import_provenance");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut oversized = base.clone();
    oversized["import_provenance"]["original_bytes"] =
        json!(vec![0; MAX_IMPORT_DOCUMENT_BYTES + 1]);
    let oversized = serde_json::to_vec(&oversized).unwrap();
    assert!(oversized.len() < MAX_DOCUMENT_BYTES);
    assert!(decode(&oversized).is_err());
}

#[test]
fn schema_seventeen_migrates_absent_and_rejects_provenance_backport() {
    let empty = Project::new(48_000).unwrap();
    assert!(empty.import_provenance().is_none());
    let mut legacy: Value = serde_json::from_slice(&encode(&empty).unwrap()).unwrap();
    assert_eq!(legacy["version"], 24);
    assert!(legacy["import_provenance"].is_null());
    legacy["version"] = json!(17);
    legacy.as_object_mut().unwrap().remove("audio_feeds");
    legacy.as_object_mut().unwrap().remove("midi_routes");
    legacy.as_object_mut().unwrap().remove("routing");
    // A strict older reader refuses the new field even if null.
    assert!(decode(&serde_json::to_vec(&legacy).unwrap()).is_err());
    legacy.as_object_mut().unwrap().remove("import_provenance");
    let migrated = decode(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(migrated, empty);
    assert!(migrated.import_provenance().is_none());
    legacy["import_provenance"] =
        json!({"kind":"ableton-live-set","version":1,"adapter_version":1,"original_bytes":[42]});
    assert!(decode(&serde_json::to_vec(&legacy).unwrap()).is_err());
}

#[test]
fn combined_document_limit_refuses_publication_without_dropping_provenance() {
    let mut editor = Editor::new(project(vec![255; MAX_IMPORT_DOCUMENT_BYTES]), 1).unwrap();
    assert!(matches!(
        editor
            .execute(Command::ImportAudioTrack {
                name: "original".into(),
                path: "x".repeat(MAX_DOCUMENT_BYTES).into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap(),
        Created::Imported { .. }
    ));
    let candidate = editor.project();
    assert!(encode(candidate).is_err());
    assert_eq!(
        candidate
            .import_provenance()
            .unwrap()
            .original_bytes()
            .len(),
        MAX_IMPORT_DOCUMENT_BYTES
    );
}
