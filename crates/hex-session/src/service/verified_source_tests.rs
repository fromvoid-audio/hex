//! Actual verified readers + canonical worker; no native ownership is inferred.
use super::*;
use crate::{AudioSourceBinding, DocumentAudioMode, SourcePlanReservation};
use std::{fs, sync::atomic::AtomicU64};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) struct Files(PathBuf);
impl Files {
    pub(super) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-verified-service-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub(super) fn wav(&self, name: &str, rate: u32, frames: u32, value: i16) -> PathBuf {
        let path = self.0.join(name);
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
        for _ in 0..frames {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub(super) fn reservation(count: usize) -> SourcePlanReservation {
    SourcePlanReservation {
        reader: hex_media::VerifiedWavReaderLimits {
            reader: hex_media::WavReaderLimits {
                max_file_bytes: 64 * 1024 * 1024,
                max_source_frames: 8_000_000,
                max_header_count: 32,
                max_window_frames: 4096,
            },
            max_manifest_bytes: 1024 * 1024,
        },
        max_retained_sources: count,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 4 * 1024 * 1024,
    }
}
pub(super) fn next(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
}
pub(super) fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn verified(snapshot: &CanonicalAudioSourceSnapshot) -> &crate::VerifiedAudioFile {
    let AudioSourceBinding::VerifiedFile(source) = snapshot.sources().values().next().unwrap()
    else {
        panic!("verified binding expected")
    };
    source
}
#[test]
fn ordinary_batch_exceeds_working_pcm_without_partial_commit_and_keeps_one_undo() {
    let files = Files::new();
    let a = files.wav("ordinary-a.wav", 24000, 3, 8192);
    let b = files.wav("ordinary-b.wav", 48000, 7, -8192);
    let bad = files.0.join("invalid.wav");
    fs::write(&bad, b"invalid wave").unwrap();
    let mut session = Session::new_with_audio_mode(
        48000,
        16,
        4,
        DocumentAudioMode::Resident,
        Some(reservation(4)),
    )
    .unwrap();
    let before = session.project().clone();
    let context = AudioBatchContext {
        project_generation: 0,
        revision: 0,
        active_gesture: false,
    };
    let destination = AudioBatchDestination::NewTracks {
        before_track: None,
        start: 8,
    };
    let rejected =
        AudioBatchImport::capture(vec![a.clone(), bad], destination, &session, context).unwrap();
    assert!(rejected.run(&mut session, None, || context).is_err());
    assert_eq!(session.project(), &before);
    assert!(session.verified_sources.is_empty());
    assert!(session.waveforms().is_empty());
    assert_eq!(session.resident_frames(), 0);
    assert!(!session.editor.can_undo());

    let batch = AudioBatchImport::capture(vec![a, b], destination, &session, context).unwrap();
    let control = batch.control();
    let (_, prepared) = batch.run(&mut session, None, || context).unwrap();
    assert_eq!(
        control.progress().phase,
        crate::import_batch_decode::AudioBatchPhase::Completed
    );
    assert_eq!(session.audio_mode(), DocumentAudioMode::Resident);
    assert!(prepared.arrangement().contains_streamed_sources());
    assert_eq!(session.resident_frames(), 0);
    assert!(session.media.is_empty());
    assert_eq!(session.waveforms().len(), 2);
    assert_eq!(
        session
            .project()
            .assets()
            .map(|asset| asset.playback_frames())
            .collect::<Vec<_>>(),
        vec![6, 7]
    );
    let committed = session.project().clone();
    let undone = session
        .undo_prepared_revision(Some(&prepared))
        .unwrap()
        .unwrap();
    assert!(session.project().tracks().is_empty());
    assert_eq!(session.project().assets().count(), 0);
    assert_eq!(session.project().sample_rate(), before.sample_rate());
    assert!(!session.editor.can_undo());
    let redone = session
        .redo_prepared_revision(Some(&undone))
        .unwrap()
        .unwrap();
    assert_eq!(session.project(), &committed);
    assert!(redone.arrangement().contains_streamed_sources());
}

#[test]
fn verified_batch_exceeds_resident_budget_without_pcm_and_undo_retains_source_epoch_and_bounds() {
    let files = Files::new();
    let a = files.wav("a.wav", 24000, 131072, 1000);
    let b = files.wav("b.wav", 48000, 131072, 2000);
    let mut session = Session::new_with_audio_mode(
        48000,
        16,
        0,
        DocumentAudioMode::VerifiedStreaming,
        Some(reservation(2)),
    )
    .unwrap();
    let initial_epoch = session.semantic_source_epoch;
    let context = AudioBatchContext {
        project_generation: 0,
        revision: 0,
        active_gesture: false,
    };
    let batch = AudioBatchImport::capture(
        vec![a, b],
        AudioBatchDestination::NewTracks {
            before_track: None,
            start: 0,
        },
        &session,
        context,
    )
    .unwrap();
    let (_, prepared) = batch.run(&mut session, None, || context).unwrap();
    assert!(prepared.arrangement().contains_streamed_sources());
    assert!(
        session
            .project()
            .tracks()
            .iter()
            .all(|track| track.instrument().is_none())
    );
    let track_ids: Vec<_> = session
        .project()
        .tracks()
        .iter()
        .map(|track| track.id())
        .collect();
    assert_eq!(session.resident_frames(), 0);
    assert!(session.media.is_empty());
    assert!(session.waveforms().is_empty());
    assert_eq!(
        session
            .project()
            .assets()
            .map(|a| a.playback_frames())
            .collect::<Vec<_>>(),
        vec![262144, 131072]
    );
    let captured =
        CanonicalAudioSourceSnapshot::capture(&Arc::new(session.project().clone()), 0, 1, &session);
    assert_eq!(captured.required_sources().count(), 2);
    assert_ne!(session.semantic_source_epoch, initial_epoch);
    let epoch = session.semantic_source_epoch;
    let undone = session
        .undo_prepared_revision(Some(&prepared))
        .unwrap()
        .unwrap();
    assert!(session.project().assets().next().is_none());
    assert_eq!(session.verified_sources.len(), 2);
    let undone_capture =
        CanonicalAudioSourceSnapshot::capture(&Arc::new(session.project().clone()), 0, 2, &session);
    assert_eq!(undone_capture.sources().len(), 0); // history facts are not the current Project
    assert_eq!(undone_capture.required_sources().count(), 0);
    assert_eq!(captured.required_sources().count(), 2); // frozen old projection
    assert_eq!(session.semantic_source_epoch, epoch);
    let redone = session
        .redo_prepared_revision(Some(&undone))
        .unwrap()
        .unwrap();
    assert!(redone.arrangement().contains_streamed_sources());
    assert_eq!(
        session
            .project()
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>(),
        track_ids
    );
    assert!(
        session
            .project()
            .tracks()
            .iter()
            .all(|track| track.instrument().is_none())
    );
    assert_eq!(session.semantic_source_epoch, epoch);
    assert!(session.compile().is_err());
    let clips: Vec<_> = session
        .project()
        .tracks()
        .iter()
        .flat_map(|track| track.clips().iter().map(|clip| clip.id()))
        .collect();
    let mut previous = redone;
    for clip in clips {
        previous = session
            .command_prepared_revision(Command::RemoveClip(clip), Some(&previous))
            .unwrap()
            .1;
    }
    assert_eq!(session.project().assets().count(), 2);
    let source_free =
        CanonicalAudioSourceSnapshot::capture(&Arc::new(session.project().clone()), 0, 3, &session);
    assert_eq!(source_free.sources().len(), 2); // actual unused current asset records
    assert_eq!(source_free.required_sources().count(), 0);
    assert!(
        session
            .verify_import(files.wav("third.wav", 48000, 16, 1))
            .is_err()
    );
    let fresh = Session::new_with_audio_mode(
        48000,
        16,
        0,
        DocumentAudioMode::VerifiedStreaming,
        Some(reservation(2)),
    )
    .unwrap();
    assert_ne!(fresh.semantic_source_epoch, epoch);
}
#[test]
fn ordinary_document_keeps_placing_complete_samples_past_its_resident_working_set() {
    let files = Files::new();
    let a = files.wav("resident-first.wav", 48000, 4, 1000);
    let b = files.wav("next-complete-source.wav", 48000, 4, 2000);
    let service = Service::new(48000, 16, 4).unwrap();
    let _ = next(&service);
    let new = run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(4)),
        },
    );
    assert!(new.error.is_none(), "{:?}", new.error);
    let first = run(&service, Request::Import(a));
    assert!(first.error.is_none(), "{:?}", first.error);
    let track = first.state.project.tracks()[0].id();
    let original = first.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::ImportToTrack {
            path: b,
            track,
            start: 4,
        })
        .unwrap();
    let added = next(&service);
    assert!(
        added.error.is_none(),
        "the ordinary sample drop must use bounded file sources: {:?}",
        added.error
    );
    let clips = added.state.project.track(track).unwrap().clips();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].id(), original);
    assert_eq!((clips[0].start(), clips[0].duration()), (0, 4));
    assert_eq!((clips[1].start(), clips[1].duration()), (4, 4));
    assert_ne!(clips[0].asset(), clips[1].asset());
    let new_clip = clips[1].id();
    let snapshot = added.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(snapshot.audio_mode(), DocumentAudioMode::Resident);
    assert_eq!(snapshot.required_sources().count(), 2);
    assert!(
        snapshot
            .sources()
            .values()
            .any(|source| matches!(source, AudioSourceBinding::VerifiedFile(_))),
        "crossing the PCM working set must not just increase its frame ceiling"
    );
    let expected = added.state.project.snapshot();
    let undone = run(&service, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project.track(track).unwrap().clips().len(), 1);
    let redone = run(&service, Request::Redo);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project.clip(new_clip).unwrap().duration(), 4);
    let saved = files.0.join("growing-project.hex");
    let save = run(&service, Request::SaveNew(saved.clone()));
    assert!(save.error.is_none(), "{:?}", save.error);
    let reset = run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(4)),
        },
    );
    assert!(reset.error.is_none(), "{:?}", reset.error);
    assert!(reset.state.project.tracks().is_empty());
    let reopened = run(&service, Request::Open(saved));
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(reopened.state.project.snapshot(), expected);
    assert_eq!(
        reopened.state.project.track(track).unwrap().clips().len(),
        2
    );
    assert_eq!(reopened.state.project.clip(new_clip).unwrap().duration(), 4);
}

#[test]
fn canonical_verified_bindings_survive_save_as_and_changed_bytes_require_explicit_reopen() {
    let files = Files::new();
    let path = files.wav("source.wav", 24000, 1024, 1000);
    let service = Service::new(48000, 16, 0).unwrap();
    assert!(next(&service).playback.is_some());
    let empty = run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(4)),
        },
    );
    assert!(empty.project_replaced && empty.playback.is_none());
    let imported = run(&service, Request::Import(path.clone()));
    assert!(imported.playback.is_none());
    assert!(imported.state.project.tracks()[0].instrument().is_none());
    let resident = Service::new(48000, 16, 4096).unwrap();
    let _ = next(&resident);
    let resident_import = run(&resident, Request::Import(path.clone()));
    assert!(
        resident_import.state.project.tracks()[0]
            .instrument()
            .is_none()
    );
    for track in [
        &imported.state.project.tracks()[0],
        &resident_import.state.project.tracks()[0],
    ] {
        assert!(track.devices().is_empty());
        assert_eq!(track.ordered_device_chain().count(), 0);
    }
    let snapshot = imported.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(snapshot.audio_mode(), DocumentAudioMode::VerifiedStreaming);
    assert_eq!(snapshot.context().editor_revision, imported.state.revision);
    let fingerprint = *verified(snapshot).fingerprint();
    let saved = files.0.join("saved.hex");
    let event = run(&service, Request::SaveNew(saved.clone()));
    assert_eq!(
        verified(event.state.canonical_audio_source.as_ref().unwrap()).path(),
        path
    );
    assert!(std::ptr::eq(
        verified(snapshot).path(),
        verified(event.state.canonical_audio_source.as_ref().unwrap()).path()
    ));
    files.wav("source.wav", 24000, 1024, 3000);
    let event = run(&service, Request::Edit(Command::MasterGain { gain: 0.5 }));
    assert_eq!(
        *verified(event.state.canonical_audio_source.as_ref().unwrap()).fingerprint(),
        fingerprint
    );
    assert_eq!(*verified(snapshot).fingerprint(), fingerprint);
    let changed_source_path = files.wav("wrong-format.wav", 48000, 512, 1000);
    let asset = imported.state.project.assets().next().unwrap().id();
    service
        .submit(Request::OpenWithAudioMode {
            source: ProjectSource::Saved(saved.clone()),
            relinks: BTreeMap::from([(asset, changed_source_path)]),
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(4)),
        })
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.is_some());
    assert!(!refused.project_replaced);
    assert_eq!(
        *verified(refused.state.canonical_audio_source.as_ref().unwrap()).fingerprint(),
        fingerprint
    );

    let reopened = run(
        &service,
        Request::OpenWithAudioMode {
            source: ProjectSource::Saved(saved),
            relinks: BTreeMap::new(),
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(4)),
        },
    );
    assert!(reopened.playback.is_none());
    assert_ne!(
        *verified(reopened.state.canonical_audio_source.as_ref().unwrap()).fingerprint(),
        fingerprint
    );
    assert!(
        reopened
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation
            > snapshot.context().project_generation
    );
    service
        .submit(Request::Export(files.0.join("must-not-render.wav")))
        .unwrap();
    assert!(next(&service).playback.is_none());
    let completion = service
        .export_events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(
        completion
            .result
            .unwrap_err()
            .contains("unavailable for verified streaming")
    );
    assert!(!files.0.join("must-not-render.wav").exists());
}

#[test]
fn project_sample_request_refuses_verified_streaming_without_history_change() {
    let files = Files::new();
    let path = files.wav("verified-project-sample.wav", 48_000, 64, 1000);
    let service = Service::new(48_000, 16, 0).unwrap();
    let _ = next(&service);
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(2)),
        },
    );
    let imported = run(&service, Request::Import(path));
    let snapshot = imported
        .state
        .canonical_audio_source
        .as_ref()
        .expect("verified source snapshot");
    assert_eq!(snapshot.audio_mode(), DocumentAudioMode::VerifiedStreaming);
    assert!(imported.playback.is_none());
    let track = imported.state.project.tracks()[0].id();
    let asset = imported.state.project.assets().next().unwrap().id();
    assert!(matches!(
        snapshot.sources().get(&asset),
        Some(AudioSourceBinding::VerifiedFile(_))
    ));
    let request = Request::InsertProjectSample {
        project_generation: snapshot.context().project_generation,
        revision: imported.state.revision,
        asset,
        destination: crate::import_batch::AudioBatchDestination::Track { track, start: 64 },
        new_track_gain: None,
        warp: hex_project::AudioImportWarp::Off,
        audition_timing: None,
    };
    service.submit(request).unwrap();
    let refused = next(&service);
    assert_eq!(
        refused.error.as_deref(),
        Some("project sample placement requires resident audio")
    );
    assert_eq!(refused.state.revision, imported.state.revision);
    assert_eq!(refused.state.project, imported.state.project);

    // The refused request must not add an undo entry ahead of the import.
    let undone = run(&service, Request::Undo);
    assert!(undone.state.project.tracks().is_empty());
    assert!(undone.state.project.assets().next().is_none());
    let redone = run(&service, Request::Redo);
    assert_eq!(redone.state.project, imported.state.project);
}

#[test]
fn invalid_verified_batch_is_atomic_and_metadata_reservation_precedes_canonical_adoption() {
    let files = Files::new();
    let good = files.wav("good.wav", 48000, 4096, 10);
    let bad = files.0.join("bad.wav");
    fs::write(&bad, b"bad").unwrap();
    let mut session = Session::new_with_audio_mode(
        48000,
        16,
        0,
        DocumentAudioMode::VerifiedStreaming,
        Some(reservation(2)),
    )
    .unwrap();
    let before = session.project().clone();
    let epoch = session.semantic_source_epoch;
    let context = AudioBatchContext {
        project_generation: 1,
        revision: 2,
        active_gesture: false,
    };
    let batch = AudioBatchImport::capture(
        vec![good.clone(), bad],
        AudioBatchDestination::NewTracks {
            before_track: None,
            start: 0,
        },
        &session,
        context,
    )
    .unwrap();
    assert!(batch.run(&mut session, None, || context).is_err());
    assert_eq!(session.project(), &before);
    assert!(session.verified_sources.is_empty());
    assert_eq!(session.semantic_source_epoch, epoch);
    let mut limits = reservation(2);
    limits.max_path_bytes = 1;
    let refused = Session::new_with_audio_mode(
        48000,
        16,
        0,
        DocumentAudioMode::VerifiedStreaming,
        Some(limits),
    )
    .unwrap();
    assert!(refused.verify_import(good).is_err());
    assert_eq!(refused.project().assets().count(), 0);
    limits.reader.reader.max_source_frames = crate::MAX_STREAMED_SOURCE_FRAMES;
    assert!(limits.validate().is_ok());
    limits.reader.reader.max_source_frames = crate::MAX_STREAMED_SOURCE_FRAMES + 1;
    assert!(limits.validate().is_err());
    limits.reader.reader.max_source_frames = 8_000_000;
    limits.reader.reader.max_file_bytes = crate::MAX_STREAMED_FILE_BYTES + 1;
    assert!(limits.validate().is_err());
}

#[test]
fn verified_import_into_existing_track_preserves_its_instrument_and_native_refusal() {
    let files = Files::new();
    let path = files.wav("existing.wav", 48000, 16, 1000);
    let service = Service::new(48000, 16, 0).unwrap();
    next(&service);
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(2)),
        },
    );
    let added = run(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "existing".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }),
    );
    let track = &added.state.project.tracks()[0];
    let instrument = track.instrument().unwrap().id();
    let imported = run(
        &service,
        Request::ImportToTrack {
            path,
            track: track.id(),
            start: 0,
        },
    );
    assert_eq!(
        imported.state.project.tracks()[0]
            .instrument()
            .unwrap()
            .id(),
        instrument
    );
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .candidate_reservation_quote()
            .is_err()
    );
}

#[test]
fn verified_bindings_prepare_through_the_ordinary_resource_scope_without_pcm() {
    let files = Files::new();
    let source = files.wav("ordinary-scope.wav", 48_000, 8, 1000);
    let mut session = Session::new_with_audio_mode(
        48_000,
        16,
        0,
        DocumentAudioMode::VerifiedStreaming,
        Some(reservation(4)),
    )
    .unwrap();
    let verified = session.verify_import(source).unwrap();
    session
        .import_verified_revision(
            verified,
            VerifiedImportDestination::NewTrack {
                before: None,
                start: 0,
            },
            None,
        )
        .unwrap();
    let cancel = AtomicBool::new(false);
    let originals = hex_engine::native_device_resources::ResidentDeviceAssets::default();
    let resources =
        hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
            &originals,
            session.project(),
            1_000_000,
            &cancel,
        )
        .unwrap();
    let mut bank = session.media_bank();
    let prepared = Arc::new(
        bank.prepare_with_device_resources(
            session.project(),
            session.sine_table(),
            None,
            &session.audio_transforms,
            &resources,
            0,
            &cancel,
        )
        .unwrap(),
    );
    assert!(prepared.contains_streamed_sources());
    assert!(prepared.requires_device_resources());
    assert!(hex_engine::SessionPlayback::new(Arc::clone(&prepared)).is_err());
    assert!(hex_engine::SessionPlayback::new_with_device_resources(prepared, &resources).is_ok());
    assert_eq!(session.resident_frames(), 0);
    assert!(bank.resident.is_empty());
    assert_eq!(bank.verified.len(), 1);
    assert_eq!(session.project().tracks()[0].clips()[0].duration(), 8);
    bank.epoch = 0;
    assert!(
        bank.prepare_with_device_resources(
            session.project(),
            session.sine_table(),
            None,
            &session.audio_transforms,
            &resources,
            0,
            &cancel
        )
        .is_err()
    );
}

#[test]
fn ordinary_reserved_sources_verify_full_files_without_decoding_or_changing_mode() {
    let files = Files::new();
    let path = files.wav("metadata-only.wav", 48_000, 32, 1000);
    let mut session = Session::new_with_audio_mode(
        48_000,
        16,
        0,
        DocumentAudioMode::Resident,
        Some(reservation(1)),
    )
    .unwrap();
    let before = session.project().snapshot();
    let epoch = session.semantic_source_epoch;
    let source = session.verify_import(path.clone()).unwrap();
    assert_eq!(source.playback_frames(), 32);
    assert_eq!(session.audio_mode(), DocumentAudioMode::Resident);
    assert_ne!(epoch, 0);
    assert_eq!(session.resident_frames(), 0);
    assert!(session.media.is_empty());
    assert!(
        session.verified_sources.is_empty(),
        "verification is not commit authority"
    );
    assert_eq!(session.project().snapshot(), before);
    assert!(
        session
            .verify_import_with_pending(path.clone(), &[source], || false)
            .is_err(),
        "metadata capacity remains bounded"
    );
    assert!(
        session
            .verify_import_with_pending(path, &[], || true)
            .is_err(),
        "cancellation refuses before publication"
    );
    let other = Session::new_with_audio_mode(
        48_000,
        16,
        0,
        DocumentAudioMode::Resident,
        Some(reservation(1)),
    )
    .unwrap();
    assert_ne!(other.semantic_source_epoch, epoch);
    session
        .source_reservation
        .as_mut()
        .unwrap()
        .max_retained_sources = 0;
    assert!(
        Session::new_with_audio_mode(
            48_000,
            16,
            0,
            DocumentAudioMode::Resident,
            session.source_reservation
        )
        .is_err(),
        "ordinary plans receive the same validation"
    );
}

#[test]
fn ordinary_partial_working_set_places_full_sources_and_bad_files_preserve_history() {
    let files = Files::new();
    let a = files.wav("first.wav", 48_000, 4, 1000);
    let b = files.wav("second.wav", 48_000, 4, 2000);
    let service = Service::new(48_000, 16, 6).unwrap();
    let _ = next(&service);
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(4)),
        },
    );
    let first = run(&service, Request::Import(a));
    let track = first.state.project.tracks()[0].id();
    let added = run(
        &service,
        Request::ImportToTrack {
            path: b,
            track,
            start: 4,
        },
    );
    assert_eq!(added.state.project.track(track).unwrap().clips().len(), 2);
    let snapshot = added.state.canonical_audio_source.as_ref().unwrap();
    let pcm: usize = snapshot
        .sources()
        .values()
        .map(|source| match source {
            AudioSourceBinding::Resident(audio) => audio.frames().len(),
            AudioSourceBinding::VerifiedFile(source) => {
                assert_eq!(source.playback_frames(), 4);
                0
            }
        })
        .sum();
    assert_eq!(
        pcm, 4,
        "the second complete source does not consume the partial PCM allowance"
    );
    let before = added.state.project.snapshot();
    let malformed = files.0.join("malformed.wav");
    fs::write(&malformed, b"not a wave file").unwrap();
    service
        .submit(Request::ImportToTrack {
            path: malformed,
            track,
            start: 8,
        })
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.revision, added.state.revision);
    assert_eq!(refused.state.project.snapshot(), before);
    assert_eq!(
        (refused.state.can_undo, refused.state.can_redo),
        (added.state.can_undo, added.state.can_redo)
    );
}

#[test]
fn ordinary_small_reopen_retains_source_planning_for_the_next_complete_drop() {
    let files = Files::new();
    let a = files.wav("reopen-first.wav", 48_000, 4, 1000);
    let b = files.wav("reopen-next.wav", 48_000, 4, 2000);
    let service = Service::new(48_000, 16, 4).unwrap();
    let _ = next(&service);
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(4)),
        },
    );
    let first = run(&service, Request::Import(a));
    let track = first.state.project.tracks()[0].id();
    let saved = files.0.join("small.hex");
    run(&service, Request::SaveNew(saved.clone()));
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(4)),
        },
    );
    let opened = run(&service, Request::Open(saved));
    assert_eq!(opened.state.project.track(track).unwrap().clips().len(), 1);
    let added = run(
        &service,
        Request::ImportToTrack {
            path: b,
            track,
            start: 4,
        },
    );
    assert_eq!(added.state.project.track(track).unwrap().clips().len(), 2);
    assert_eq!(
        added
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .audio_mode(),
        DocumentAudioMode::Resident
    );
}

#[test]
fn file_batch_storage_and_preparation_refusals_preserve_project_sources_and_waveforms() {
    let files = Files::new();
    let a = files.wav("accepted.wav", 48_000, 4, 1000);
    let b = files.wav("replacement.wav", 48_000, 8, 2000);
    let mut session = Session::new_with_audio_mode(
        48_000,
        16,
        0,
        DocumentAudioMode::VerifiedStreaming,
        Some(reservation(4)),
    )
    .unwrap();
    let source = session.verify_import(a).unwrap();
    let (created, _) = session
        .import_verified_revision(
            source,
            VerifiedImportDestination::NewTrack {
                before: None,
                start: 0,
            },
            None,
        )
        .unwrap();
    let hex_project::Created::Imported { track, clip, .. } = created else {
        panic!("seed import")
    };
    // Truthful cached peaks for the existing constant file; this display owner must
    // survive a replacement whose private prepared waveform map drops that clip.
    let value = 1000.0 / 32768.0;
    let waveform = Arc::new(
        hex_media::waveform::Waveform::from_peaks(
            4,
            vec![
                [hex_media::waveform::Peak {
                    min: value,
                    max: value,
                }; 2],
            ],
        )
        .unwrap(),
    );
    session.audio_clip_waveforms.get_mut().insert(
        clip,
        AudioClipWaveform {
            waveform: Arc::clone(&waveform),
            offset: 0,
            length: 4,
            preview_root: None,
        },
    );
    let before = session.project().snapshot();
    let epoch = session.semantic_source_epoch;
    let history = (session.can_undo(), session.can_redo());
    let context = AudioBatchContext {
        project_generation: 1,
        revision: 1,
        active_gesture: false,
    };
    let batch = AudioBatchImport::capture(
        vec![b.clone()],
        AudioBatchDestination::Track { track, start: 0 },
        &session,
        context,
    )
    .unwrap();
    let result = batch.run_with_storage_admission_using(
        &mut session,
        None,
        || context,
        |_, _| Err(SessionError::Invalid("injected storage refusal")),
        |_, _| panic!("storage refusal precedes preparation"),
    );
    assert!(result.is_err());
    assert_eq!(session.project().snapshot(), before);
    let batch = AudioBatchImport::capture(
        vec![b],
        AudioBatchDestination::Track { track, start: 0 },
        &session,
        context,
    )
    .unwrap();
    let called = std::cell::Cell::new(false);
    let result = batch.run_with_storage_admission_using(
        &mut session,
        None,
        || context,
        |_, members| {
            assert!(members.is_empty());
            Ok(())
        },
        |input, admit| {
            called.set(true);
            assert_eq!(input.media.verified.len(), 2);
            let views = input.waveforms;
            let prepared = crate::import_batch::prepare_asset_free(input, admit)?;
            assert!(prepared.contains_streamed_sources());
            assert!(
                views.borrow().is_empty(),
                "candidate replacement has no resident waveform root"
            );
            Err(SessionError::Invalid("injected publication refusal"))
        },
    );
    assert!(result.is_err());
    assert!(
        called.get(),
        "file-backed batches must honor the caller's preparation hook"
    );
    assert_eq!(session.project().snapshot(), before);
    assert_eq!(session.semantic_source_epoch, epoch);
    assert_eq!(session.verified_sources.len(), 1);
    assert_eq!(session.resident_frames(), 0);
    assert_eq!((session.can_undo(), session.can_redo()), history);
    let retained = session.audio_clip_waveforms().unwrap();
    assert_eq!(retained.len(), 1);
    assert!(Arc::ptr_eq(&retained[&clip].waveform, &waveform));
}

#[test]
fn identical_file_extrema_do_not_hide_independent_channels() {
    let files = Files::new();
    let path = files.0.join("independent.wav");
    let mut index = 0;
    hex_export::write_float_wav_new(&path, 48_000, 4096, &AtomicBool::new(false), |output| {
        for frame in output {
            *frame = if index % 2 == 0 {
                [0.8, -0.8]
            } else {
                [-0.8, 0.8]
            };
            index += 1;
        }
        Ok(())
    })
    .unwrap();
    let mut source =
        crate::VerifiedAudioFile::inspect(path, 48_000, reservation(1).reader, || false).unwrap();
    source.prepare_waveform_on_control(|| false).unwrap();
    let waveform = source.cached_waveform().unwrap();
    assert!(waveform.bins().iter().all(|pair| pair[0] == pair[1]));
    assert!(!waveform.has_identical_channels());
}

#[test]
fn ordinary_file_peaks_match_complete_resident_conversion_without_retaining_pcm() {
    let files = Files::new();
    for (source_rate, project_rate) in [(48_000, 48_000), (44_100, 48_000), (96_000, 44_100)] {
        let path = files.wav("peaks.wav", source_rate, 5003, 0);
        let mut bytes = fs::read(&path).unwrap();
        for (index, sample) in bytes[44..].chunks_exact_mut(2).enumerate() {
            let value = if index == 5002 {
                -32_000
            } else {
                ((index % 17) as i16 - 8) * 2000
            };
            sample.copy_from_slice(&value.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        let mut source = crate::VerifiedAudioFile::inspect(
            path.clone(),
            project_rate,
            reservation(1).reader,
            || false,
        )
        .unwrap();
        let facts = source.clone();
        if source_rate == project_rate {
            let mut small = reservation(1).reader;
            small.reader.max_window_frames = 2;
            let mut bounded =
                crate::VerifiedAudioFile::inspect(path.clone(), project_rate, small, || false)
                    .unwrap();
            bounded.prepare_waveform_on_control(|| false).unwrap();
            assert_eq!(bounded.cached_waveform().unwrap().source_frames(), 5003);
        }
        assert!(source.cached_waveform().is_none());
        let decoded = hex_media::decode_wav_file(
            path.clone(),
            hex_media::Limits::new(64 * 1024 * 1024, 8192),
        )
        .unwrap();
        let resident = crate::PreparedImport::at_rate(
            decoded,
            project_rate,
            hex_media::ResampleLimits {
                max_source_frames: 8192,
                max_output_frames: 16_384,
            },
        )
        .unwrap();
        let expected = hex_media::waveform::summarize_peaks(&resident.audio, 1024).unwrap();
        source.prepare_waveform_on_control(|| false).unwrap();
        let actual = source.cached_waveform().unwrap();
        assert!(actual.has_identical_channels());
        assert_eq!(actual.source_frames(), expected.source_frames());
        assert_eq!(actual.bins(), expected.bins());
        assert_eq!(actual.cached_bin_count(), expected.cached_bin_count());
        assert!(actual.cached_bin_count() <= 2047);
        assert!(!actual.shares_audio_with(&resident.audio));
        assert_eq!(
            source, facts,
            "derived display data must not refresh file facts"
        );
        // Keep the same cancellation point on a source with enough work to
        // reach it even when contiguous scanning finishes the tiny oracle in
        // fewer polls. Assert the actual refusal reason, not just any failure.
        let cancel_path = files.wav("cancel-waveform.wav", source_rate, 65_537, 1000);
        let mut cancelled = crate::VerifiedAudioFile::inspect(
            cancel_path,
            project_rate,
            reservation(1).reader,
            || false,
        )
        .unwrap();
        let mut polls = 0;
        let refusal = cancelled
            .prepare_waveform_on_control(|| {
                polls += 1;
                polls > 10
            })
            .unwrap_err();
        assert!(polls > 10);
        assert!(refusal.to_string().contains("cancel"), "{refusal}");
        assert!(cancelled.cached_waveform().is_none());
        files.wav("peaks.wav", source_rate, 5003, 1234);
        let mut changed = facts;
        assert!(changed.prepare_waveform_on_control(|| false).is_err());
        assert!(changed.cached_waveform().is_none());
    }
}

#[test]
fn ordinary_reopen_keeps_processed_resident_sources_and_plain_files_with_atomic_refusal() {
    let files = Files::new();
    let processed_path = files.wav("processed.wav", 48_000, 8, 8192);
    let plain_path = files.wav("plain-file.wav", 48_000, 32, 4096);
    let service = Service::new(48_000, 16, 16).unwrap();
    next(&service);
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(4)),
        },
    );
    let first = run(&service, Request::Import(processed_path));
    let processed = first.state.project.tracks()[0].clips()[0].id();
    run(
        &service,
        Request::Edit(Command::SetAudioClipPitch {
            clip: processed,
            cents: 137,
        }),
    );
    run(
        &service,
        Request::Edit(Command::SetAudioClipMusical {
            clip: processed,
            enabled: true,
        }),
    );
    let imported = run(
        &service,
        Request::ImportToNewTrack {
            path: plain_path,
            before_track: None,
            start: 0,
        },
    );
    let plain = imported
        .state
        .project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .find(|clip| clip.id() != processed)
        .unwrap()
        .id();
    run(
        &service,
        Request::Edit(Command::SetClipFades {
            clip: plain,
            fade_in: 2,
            fade_out: 2,
        }),
    );
    let path = files.0.join("processed-and-file.hex");
    run(&service, Request::SaveNew(path.clone()));
    let expected = hex_storage::load(&path).unwrap();
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(4)),
        },
    );
    let opened = run(&service, Request::Open(path));
    assert!(opened.open_complete);
    assert_eq!(opened.state.project.as_ref(), &expected);
    let source = opened.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(source.audio_mode(), DocumentAudioMode::Resident);
    assert_eq!(
        source
            .sources()
            .values()
            .filter(|source| matches!(source, AudioSourceBinding::VerifiedFile(_)))
            .count(),
        1
    );
    assert_eq!(opened.state.waveforms.len(), 2);
    service
        .submit(Request::Edit(Command::SetAudioClipPitch {
            clip: plain,
            cents: 137,
        }))
        .unwrap();
    let refused = next(&service);
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("require resident audio")
    );
    assert_eq!(refused.state.project, opened.state.project);
    assert_eq!(refused.state.revision, opened.state.revision);
    assert_eq!(refused.state.can_undo, opened.state.can_undo);
    let after = refused.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(after.context(), source.context());
    let asset = opened.state.project.clip(plain).unwrap().asset();
    let (AudioSourceBinding::VerifiedFile(before), Some(AudioSourceBinding::VerifiedFile(after))) =
        (&source.sources()[&asset], after.sources().get(&asset))
    else {
        panic!("verified facts")
    };
    assert_eq!(before, after);
}

#[test]
fn plain_reverse_file_backed_clip_commits_and_reopens_without_resident_pcm() {
    let files = Files::new();
    let media = files.wav("reverse.wav", 48_000, 8_000, 4096);
    let media_bytes = fs::read(&media).unwrap();
    let service = Service::new(48_000, 16, 16).unwrap();
    next(&service);
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(2)),
        },
    );
    let imported = run(&service, Request::Import(media.clone()));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    run(
        &service,
        Request::Edit(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        }),
    );
    let reversed = run(&service, Request::Edit(Command::ReverseClips(vec![clip])));
    let expected = reversed.state.project.snapshot();
    let asset = reversed.state.project.clip(clip).unwrap().asset();
    assert!(matches!(
        reversed
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .get(&asset),
        Some(AudioSourceBinding::VerifiedFile(_))
    ));
    assert!(
        reversed
            .state
            .project
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .content_view
            .unwrap()
            .window
            .reversed
    );
    let path = files.0.join("reverse.hex");
    run(&service, Request::SaveNew(path.clone()));
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(reservation(2)),
        },
    );
    let reopened = run(&service, Request::Open(path));
    assert_eq!(reopened.state.project.snapshot(), expected);
    assert!(matches!(
        reopened
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .get(&asset),
        Some(AudioSourceBinding::VerifiedFile(_))
    ));
    assert_eq!(fs::read(media).unwrap(), media_bytes);
}
