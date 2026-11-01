//! Arbitrary byte evidence, not generated Ableton-format or musical fixtures.
use hex_project::{Command, Created, Editor, ImportProvenance, MAX_IMPORT_DOCUMENT_BYTES, Project};
use hex_storage::{LEGACY_READER_DOCUMENT_BYTES, decode, encode, load, save_new};
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
    assert!(bytes.len() < LEGACY_READER_DOCUMENT_BYTES);
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
    assert!(oversized.len() < LEGACY_READER_DOCUMENT_BYTES);
    assert!(decode(&oversized).is_err());
}

#[test]
fn current_provenance_is_required_even_when_empty() {
    let empty = Project::new(48_000).unwrap();
    let bytes = encode(&empty).unwrap();
    assert_eq!(decode(&bytes).unwrap(), empty);
    let mut missing: Value = serde_json::from_slice(&bytes).unwrap();
    missing.as_object_mut().unwrap().remove("import_provenance");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
}

/// Maximal import evidence survives beside a source. Documents above the
/// former 8 MiB reader limit are covered by the save/replace/recover test below.
#[test]
fn maximal_provenance_preserves_original_bytes_and_source_identity() {
    let mut editor = Editor::new(project(vec![255; MAX_IMPORT_DOCUMENT_BYTES]), 1).unwrap();
    assert!(matches!(
        editor
            .execute(Command::ImportAudioTrack {
                name: "original".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap(),
        Created::Imported { .. }
    ));
    let candidate = editor.project();
    let bytes = encode(candidate).expect("maximal provenance encodes");
    assert_eq!(decode(&bytes).unwrap(), *candidate);
    assert_eq!(
        candidate
            .import_provenance()
            .unwrap()
            .original_bytes()
            .len(),
        MAX_IMPORT_DOCUMENT_BYTES
    );
}

#[test]
fn large_documents_save_replace_recover_and_checkpoint_without_truncation() {
    use hex_storage::{
        Journal, JournalRestartToken, load_journal, load_recovery, load_with_token,
        replace_existing, restart_journal,
    };
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    editor
        .execute(Command::ImportAudioTrack {
            name: "original".into(),
            path: "x".repeat(LEGACY_READER_DOCUMENT_BYTES + 1).into(),
            sample_rate: 48_000,
            frames: 100,
        })
        .unwrap();
    let large = editor.project();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hex-large-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("large.hex");
    let small = Project::new(48_000).unwrap();
    save_new(&path, &small).unwrap();
    let (_, small_token) = load_with_token(&path).unwrap();
    assert!(!small_token.requires_save_confirmation(&small).unwrap());
    assert!(small_token.requires_save_confirmation(large).unwrap());
    // An explicit ceiling uses the same rule: only crossing it from at/below asks.
    let small_len = fs::read(&path).unwrap().len();
    assert!(
        !small_token
            .requires_save_confirmation_within(&small, small_len)
            .unwrap()
    );
    let mut grown = Editor::new(small.clone(), 1).unwrap();
    grown
        .execute(Command::AddTrack { name: "t".into() })
        .unwrap();
    assert!(
        small_token
            .requires_save_confirmation_within(grown.project(), small_len)
            .unwrap()
    );
    // Already above the ceiling: no new compatibility loss to confirm.
    assert!(
        !small_token
            .requires_save_confirmation_within(large, small_len - 1)
            .unwrap()
    );
    replace_existing(&path, &small_token, large).unwrap();
    let (reopened, token) = load_with_token(&path).unwrap();
    assert_eq!(reopened, *large);
    token.check_unchanged(&path).unwrap();
    assert!(!token.requires_save_confirmation(large).unwrap());
    assert_eq!(load_recovery(&path).unwrap().project, small);
    replace_existing(&path, &token, large).unwrap();
    assert_eq!(load_recovery(&path).unwrap().project, *large);
    assert!(replace_existing(&path, &small_token, &small).is_err());
    assert_eq!(load(&path).unwrap(), *large);
    let copy = dir.join("copy.hex");
    save_new(&copy, large).unwrap();
    assert_eq!(load(&copy).unwrap(), *large);
    let slot = dir.join("autosave");
    let mut journal = Journal::open(slot.clone()).unwrap();
    journal.checkpoint(large).unwrap();
    assert_eq!(load_journal(&slot).unwrap(), *large);
    drop(journal);
    Journal::open(slot.clone()).unwrap();
    // A recognized future envelope is preserved verbatim before a fresh large
    // checkpoint. Restart uses the same framing and exact-byte slot ownership.
    let mut unavailable = fs::read(&slot).unwrap();
    unavailable[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&slot, &unavailable).unwrap();
    let restart = JournalRestartToken::capture(slot.clone()).unwrap();
    let outcome = restart_journal(&restart, large);
    outcome.result.unwrap();
    assert_eq!(
        fs::read(outcome.preserved_path.unwrap()).unwrap(),
        unavailable
    );
    assert_eq!(load_journal(&slot).unwrap(), *large);
    fs::remove_dir_all(dir).unwrap();
}
