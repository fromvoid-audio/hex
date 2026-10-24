//! Actual verified readers + canonical worker; no native ownership is inferred.
use super::*;
use crate::{AudioSourceBinding, DocumentAudioMode, SourcePlanReservation};
use std::{fs, sync::atomic::AtomicU64, time::Duration};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-verified-service-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn wav(&self, name: &str, rate: u32, frames: u32, value: i16) -> PathBuf {
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
fn reservation(count: usize) -> SourcePlanReservation {
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
fn next(service: &Service) -> Event {
    service.events.recv_timeout(Duration::from_secs(5)).unwrap()
}
fn run(service: &Service, request: Request) -> Event {
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
        .recv_timeout(Duration::from_secs(5))
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
    limits.reader.reader.max_source_frames = 8_000_001;
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
