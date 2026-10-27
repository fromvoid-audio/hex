//! Control-worker consent preserves the older file until an explicit save.
use super::*;
use std::{fs, sync::atomic::AtomicU64, time::Duration};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hex-format-upgrade-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn next(service: &Service) -> Event {
    service.events.recv_timeout(Duration::from_secs(5)).unwrap()
}
fn request(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    next(service)
}

#[test]
fn format_upgrade_requires_document_bound_consent_and_preserves_original_on_save_as_failure() {
    let files = Files::new();
    let path = files.0.join("old.hex");
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let initial = next(&service);
    let mut old: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(&initial.state.project).unwrap()).unwrap();
    // This empty schema36 fixture predates clip activation, sections, one-shot
    // boundaries and horizontal fade coordinates. Strip later-schema inventories
    // explicitly; historical readers must keep rejecting unknown fields.
    assert_eq!(
        old.as_object_mut().unwrap().remove("clip_activation"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut()
            .unwrap()
            .remove("midi_one_shot_boundaries"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("fade_curve_positions"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("content_views"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("grouped_audio"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("audio_event_fades"),
        Some(serde_json::json!([]))
    );
    // Schema44 adds required hierarchy rows; this fixture has no tracks.
    assert_eq!(
        old.as_object_mut().unwrap().remove("track_hierarchy"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("native_sources"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("track_source_bindings"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("session_audio_cycles"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("device_automation"),
        Some(serde_json::json!([]))
    );
    for key in [
        "alias_sources",
        "alias_members",
        "host_automation",
        "inactive_midi_notes",
        "inactive_audio_events",
        "nested_audio",
    ] {
        assert_eq!(
            old.as_object_mut().unwrap().remove(key),
            Some(serde_json::json!([]))
        );
    }
    old["version"] = 36.into();
    old.as_object_mut().unwrap().remove("arrangement_sections");
    let original = serde_json::to_vec(&old).unwrap();
    fs::write(&path, &original).unwrap();

    let opened = request(&service, Request::Open(path.clone()));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(opened.document_format_upgrade_required);
    assert_eq!(fs::read(&path).unwrap(), original);
    let refused = request(&service, Request::SaveCurrent);
    assert!(refused.save_complete && refused.error.is_some());
    assert!(refused.document_format_upgrade_required);
    assert!(refused.save_confirmation_required);
    assert_eq!(fs::read(&path).unwrap(), original);

    let wrong_document = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 1,
            path: files.0.join("different.hex"),
        },
    );
    assert!(wrong_document.error.is_some());
    let stale = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 0,
            path: path.clone(),
        },
    );
    assert!(stale.error.is_some());
    let failed_save_as = request(&service, Request::SaveNew(path.clone()));
    assert!(failed_save_as.error.is_some());
    assert!(failed_save_as.document_format_upgrade_required);
    assert_eq!(fs::read(&path).unwrap(), original);

    let saved = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 1,
            path: path.clone(),
        },
    );
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert!(!saved.document_format_upgrade_required);
    assert_eq!(hex_storage::load(&path).unwrap(), *opened.state.project);
    assert_eq!(
        hex_storage::load_recovery(&path).unwrap().project,
        *opened.state.project
    );
    let current = request(&service, Request::SaveCurrent);
    assert!(current.error.is_none());
    assert!(!current.document_format_upgrade_required);

    // Opening another copy does not reuse the preceding document's consent.
    let second = files.0.join("other-old.hex");
    fs::write(&second, &original).unwrap();
    assert!(
        request(&service, Request::Open(second.clone()))
            .error
            .is_none()
    );
    let copy = files.0.join("other-upgraded.hex");
    let save_as = request(&service, Request::SaveNew(copy.clone()));
    assert!(save_as.error.is_none(), "{:?}", save_as.error);
    assert!(!save_as.document_format_upgrade_required);
    assert_eq!(fs::read(second).unwrap(), original);
    assert_eq!(hex_storage::load(copy).unwrap(), *save_as.state.project);
}

#[test]
fn growing_current_document_requires_consent_only_when_crossing_older_reader_limit() {
    use hex_project::{BeatTime, TrackContentSnapshot};
    fn large_request(service: &Service, request: Request) -> Event {
        service.submit(request).unwrap();
        // Real 8k-clip validation/compilation runs in debug, outside a realtime
        // callback. Keep a finite deadline without using the tiny-fixture budget.
        service
            .events
            .recv_timeout(Duration::from_secs(60))
            .unwrap()
    }
    let files = Files::new();
    let path = files.0.join("growing.hex");
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    else {
        panic!("midi")
    };
    let seed = clips[0].clone();
    *clips = (0..4096)
        .map(|index| {
            let mut clip = seed.clone();
            clip.id = index + 10;
            clip.name = "n".repeat(hex_project::MAX_NAME_BYTES);
            clip.start_raw = index * BeatTime::ONE.raw();
            clip
        })
        .collect();
    snapshot.next_id = 4106;
    let small = Project::from_snapshot(snapshot).unwrap();
    hex_storage::save_new(&path, &small).unwrap();
    let original = fs::read(&path).unwrap();
    assert!(original.len() < hex_storage::LEGACY_READER_DOCUMENT_BYTES);
    let service = Service::new(48_000, 4, 8_000_000).unwrap();
    next(&service);
    let opened = large_request(&service, Request::Open(path.clone()));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(!opened.document_format_upgrade_required);
    let grown = large_request(
        &service,
        Request::DuplicateClips {
            project_generation: 1,
            revision: opened.state.revision,
            clips: opened.state.project.tracks()[0]
                .midi_clips()
                .iter()
                .map(|clip| clip.id())
                .collect(),
        },
    );
    assert!(grown.error.is_none(), "{:?}", grown.error);
    assert!(
        hex_storage::encode(&grown.state.project).unwrap().len()
            > hex_storage::LEGACY_READER_DOCUMENT_BYTES
    );
    let refused = large_request(&service, Request::SaveCurrent);
    assert!(refused.save_complete && refused.save_confirmation_required);
    assert!(refused.error.is_some());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!hex_storage::recovery_path(&path).unwrap().exists());
    let stale = large_request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 0,
            path: path.clone(),
        },
    );
    assert!(stale.error.is_some() && !stale.save_confirmation_required);
    assert_eq!(fs::read(&path).unwrap(), original);
    // Failed Save As leaves both the original and its outstanding compatibility
    // decision intact. Explicit consent still uses the original exact-byte token.
    let failed = large_request(&service, Request::SaveNew(path.clone()));
    assert!(failed.error.is_some() && !failed.save_confirmation_required);
    assert_eq!(fs::read(&path).unwrap(), original);
    let saved = large_request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 1,
            path: path.clone(),
        },
    );
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert!(!saved.save_confirmation_required);
    assert_eq!(hex_storage::load(&path).unwrap(), *grown.state.project);
    assert_eq!(hex_storage::load_recovery(&path).unwrap().project, small);
    let again = large_request(&service, Request::SaveCurrent);
    assert!(again.error.is_none() && !again.save_confirmation_required);
    let copy = files.0.join("large-copy.hex");
    let copied = large_request(&service, Request::SaveNew(copy.clone()));
    assert!(copied.error.is_none(), "{:?}", copied.error);
    assert_eq!(hex_storage::load(copy).unwrap(), *grown.state.project);
}
