//! Control-worker consent preserves the older file until an explicit save.
use super::*;
use std::{fs, sync::atomic::AtomicU64};

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
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
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
        "audio_event_gains",
        "midi_clip_gains",
        "automatic_track_names",
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
    // The consent flow, not the byte count, is this boundary's contract: the
    // service's older-reader ceiling sits just above the original document, so
    // duplicating its clips crosses it. hex-storage's provenance tests own the
    // real 8 MiB LEGACY_READER_DOCUMENT_BYTES threshold.
    fn large_request(service: &Service, request: Request) -> Event {
        service.submit(request).unwrap();
        next(service)
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
    *clips = (0..64)
        .map(|index| {
            let mut clip = seed.clone();
            clip.id = index + 10;
            clip.name = "n".repeat(hex_project::MAX_NAME_BYTES);
            clip.start_raw = index * BeatTime::ONE.raw();
            clip
        })
        .collect();
    snapshot.next_id = 74;
    let small = Project::from_snapshot(snapshot).unwrap();
    hex_storage::save_new(&path, &small).unwrap();
    let original = fs::read(&path).unwrap();
    let limit = original.len() + 1024;
    assert!(limit < hex_storage::LEGACY_READER_DOCUMENT_BYTES);
    let service = Service::new(48_000, 4, 8_000_000).unwrap();
    service.diagnostics.set_legacy_reader_limit(limit as u64);
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
    assert!(hex_storage::encode(&grown.state.project).unwrap().len() > limit);
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

#[test]
fn schema60_and_schema61_save_require_consent_or_save_as_for_current_schema() {
    for version in [60, 61] {
        let files = Files::new();
        let path = files.0.join(format!("schema{version}.hex"));
        let service = Service::new(48_000, 16, 8_000_000).unwrap();
        let initial = next(&service);
        let mut prior: serde_json::Value =
            serde_json::from_slice(&hex_storage::encode(&initial.state.project).unwrap()).unwrap();
        // Empty prior formats omit fields introduced by later schemas.
        assert_eq!(
            prior.as_object_mut().unwrap().remove("midi_clip_gains"),
            Some(serde_json::json!([]))
        );
        assert_eq!(
            prior
                .as_object_mut()
                .unwrap()
                .remove("automatic_track_names"),
            Some(serde_json::json!([]))
        );
        if version == 60 {
            assert_eq!(
                prior.as_object_mut().unwrap().remove("audio_event_gains"),
                Some(serde_json::json!([]))
            );
        }
        prior["version"] = version.into();
        let original = serde_json::to_vec(&prior).unwrap();
        fs::write(&path, &original).unwrap();
        let opened = request(&service, Request::Open(path.clone()));
        assert!(opened.error.is_none(), "{:?}", opened.error);
        assert!(opened.document_format_upgrade_required);
        assert_eq!(fs::read(&path).unwrap(), original);
        let refused = request(&service, Request::SaveCurrent);
        assert!(refused.save_confirmation_required && refused.error.is_some());
        assert_eq!(fs::read(&path).unwrap(), original);
        let copy = files.0.join("upgraded.hex");
        let saved = request(&service, Request::SaveNew(copy.clone()));
        assert!(saved.error.is_none(), "{:?}", saved.error);
        assert!(!saved.document_format_upgrade_required);
        assert_eq!(fs::read(&path).unwrap(), original);
        let upgraded: serde_json::Value = serde_json::from_slice(&fs::read(copy).unwrap()).unwrap();
        assert_eq!(upgraded["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    }
}

/// A 32 kHz mono ramp, so sinc v1 and v2 produce different project PCM.
fn ramp_wav(path: &Path, frames: u32) {
    let rate = 32_000_u32;
    let mut bytes = Vec::from(*b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 2).to_le_bytes());
    for index in 0..frames {
        bytes.extend_from_slice(&((index % 64) as i16 * 400 - 12_800).to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

/// A schema64 project whose only asset carries sinc v2 conversion metadata.
fn sinc_v2_project(files: &Files) -> PathBuf {
    let media = files.0.join("ramp.wav");
    ramp_wav(&media, 3_200);
    let decoded =
        hex_media::decode_wav_file(&media, hex_media::Limits::new(1 << 20, 8_000)).unwrap();
    let prepared = crate::PreparedImport::for_conversion(
        decoded,
        48_000,
        ResampleLimits {
            max_source_frames: 8_000,
            max_output_frames: 8_000,
        },
        hex_project::SINC_CONVERSION_V2,
    )
    .unwrap();
    let mut session = crate::Session::new(48_000, 16, 100_000).unwrap();
    session.import_prepared_revision(prepared, None).unwrap();
    let path = files.0.join("v2.hex");
    let bytes = hex_storage::encode(session.project()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], hex_storage::SINC_V2_SCHEMA_VERSION);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn sinc_v2_project_opens_resident_and_streamed_and_saves_without_consent() {
    let files = Files::new();
    let path = sinc_v2_project(&files);
    let original = fs::read(&path).unwrap();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next(&service);
    // Reconstruction compares the derived conversion version with the saved
    // asset, so a successful open proves the media was converted with v2.
    let opened = request(&service, Request::Open(path.clone()));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(!opened.document_format_upgrade_required);
    let asset = opened.state.project.assets().next().unwrap();
    assert_eq!(asset.conversion_version(), hex_project::SINC_CONVERSION_V2);
    let saved = request(&service, Request::SaveCurrent);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert!(!saved.save_confirmation_required);
    assert_eq!(fs::read(&path).unwrap(), original);

    let streamed = request(
        &service,
        Request::OpenWithAudioMode {
            source: ProjectSource::Saved(path.clone()),
            relinks: BTreeMap::new(),
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(SourcePlanReservation {
                reader: hex_media::VerifiedWavReaderLimits {
                    reader: hex_media::WavReaderLimits {
                        max_file_bytes: 64 * 1024 * 1024,
                        max_source_frames: 8_000_000,
                        max_header_count: 32,
                        max_window_frames: 4096,
                    },
                    max_manifest_bytes: 1024 * 1024,
                },
                max_retained_sources: 4,
                max_path_bytes: 1024 * 1024,
                max_manifest_bytes: 4 * 1024 * 1024,
            }),
        },
    );
    // Verified pages select the kernel by the saved version, so v2 streams.
    assert!(streamed.error.is_none(), "{:?}", streamed.error);
    let crate::AudioSourceBinding::VerifiedFile(source) = streamed
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .sources()
        .values()
        .next()
        .unwrap()
    else {
        panic!("verified binding expected")
    };
    assert_eq!(source.conversion_version(), hex_project::SINC_CONVERSION_V2);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn importing_v2_media_into_a_schema63_file_requires_overwrite_consent() {
    let files = Files::new();
    let path = files.0.join("old.hex");
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let initial = next(&service);
    let original = hex_storage::encode(&initial.state.project).unwrap();
    let header: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(header["version"], 63);
    fs::write(&path, &original).unwrap();
    let opened = request(&service, Request::Open(path.clone()));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(!opened.document_format_upgrade_required);

    let media = files.0.join("ramp.wav");
    ramp_wav(&media, 3_200);
    let imported = request(&service, Request::Import(media));
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let asset = imported.state.project.assets().next().unwrap();
    assert_eq!(asset.conversion_version(), hex_project::SINC_CONVERSION_V2);

    // Overwriting the schema63 file would lock out older readers.
    let refused = request(&service, Request::SaveCurrent);
    assert!(refused.save_complete && refused.error.is_some());
    assert!(refused.save_confirmation_required);
    assert_eq!(fs::read(&path).unwrap(), original);

    let saved = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 1,
            path: path.clone(),
        },
    );
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["version"], hex_storage::SINC_V2_SCHEMA_VERSION);
    assert_eq!(hex_storage::load(&path).unwrap(), *saved.state.project);
    let again = request(&service, Request::SaveCurrent);
    assert!(again.error.is_none() && !again.save_confirmation_required);
}

#[test]
fn pitching_a_clip_in_a_schema64_file_requires_overwrite_consent() {
    let files = Files::new();
    let path = sinc_v2_project(&files);
    let original = fs::read(&path).unwrap();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next(&service);
    let opened = request(&service, Request::Open(path.clone()));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    let clip = opened.state.project.tracks()[0].clips()[0].id();
    let pitched = request(
        &service,
        Request::Edit(Command::SetAudioClipPitch { clip, cents: 700 }),
    );
    assert!(pitched.error.is_none(), "{:?}", pitched.error);
    assert_eq!(
        hex_storage::schema_version_for(&pitched.state.project),
        hex_storage::WARP_RESAMPLER_SCHEMA_VERSION
    );

    // New pitch renders with the v2 warp resampler, which needs schema65.
    let refused = request(&service, Request::SaveCurrent);
    assert!(refused.save_complete && refused.error.is_some());
    assert!(refused.save_confirmation_required);
    assert_eq!(fs::read(&path).unwrap(), original);

    let saved = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 1,
            path: path.clone(),
        },
    );
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        written["version"],
        hex_storage::WARP_RESAMPLER_SCHEMA_VERSION
    );
    let loaded = hex_storage::load(&path).unwrap();
    assert_eq!(loaded, *saved.state.project);
    let edit = loaded.clip(clip).unwrap().audio_edit().unwrap();
    assert_eq!(edit.domain.warp_resampler, hex_project::WarpResampler::V2);
    let again = request(&service, Request::SaveCurrent);
    assert!(again.error.is_none() && !again.save_confirmation_required);
}

#[test]
fn reversing_a_musical_clip_in_a_schema64_file_requires_overwrite_consent() {
    let files = Files::new();
    let path = sinc_v2_project(&files);
    let original = fs::read(&path).unwrap();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next(&service);
    let opened = request(&service, Request::Open(path.clone()));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    let clip = opened.state.project.tracks()[0].clips()[0].id();
    let musical = request(
        &service,
        Request::Edit(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        }),
    );
    assert!(musical.error.is_none(), "{:?}", musical.error);
    let reversed = request(&service, Request::Edit(Command::ReverseClips(vec![clip])));
    assert!(reversed.error.is_none(), "{:?}", reversed.error);
    assert_eq!(
        hex_storage::schema_version_for(&reversed.state.project),
        hex_storage::CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION
    );

    // A new musical directed view reads with v2, which needs schema66.
    let refused = request(&service, Request::SaveCurrent);
    assert!(refused.save_complete && refused.error.is_some());
    assert!(refused.save_confirmation_required);
    assert_eq!(fs::read(&path).unwrap(), original);

    let saved = request(
        &service,
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 1,
            path: path.clone(),
        },
    );
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        written["version"],
        hex_storage::CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION
    );
    let loaded = hex_storage::load(&path).unwrap();
    assert_eq!(loaded, *saved.state.project);
    let view = loaded
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .content_view
        .unwrap();
    assert_eq!(view.read_resampler, hex_project::WarpResampler::V2);
    let again = request(&service, Request::SaveCurrent);
    assert!(again.error.is_none() && !again.save_confirmation_required);
}
