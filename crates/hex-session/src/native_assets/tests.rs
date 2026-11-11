use super::*;
use std::{fs, path::Path, sync::atomic::AtomicU64};

pub(crate) fn reservation() -> ResidentNativeReservation {
    ResidentNativeReservation {
        storage: AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 16,
            root_rows: 8,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        },
        max_scan_frames: 1024,
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-native-asset-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Original 8 kHz mono PCM16: explicit decoder normalization to stereo.
        let samples = [4096_i16, -8192, 16384, 0];
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&44_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&8000_u32.to_le_bytes());
        bytes.extend_from_slice(&16000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&8_u32.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
    fn decoded(&self) -> DecodedAudio {
        hex_media::decode_wav_file(&self.0, hex_media::Limits::new(1024, 1024)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn asset(created: Created) -> AssetId {
    let Created::Asset(asset) = created else {
        panic!("expected actual asset id")
    };
    asset
}
fn session() -> Session {
    Session::new(16000, 8, 1024)
        .unwrap()
        .with_native_reservation(reservation())
        .unwrap()
}
fn assert_refused_unchanged(s: &mut Session, fixture: &Fixture, cancel: &AtomicBool) {
    let before = s.project().clone();
    let undo = s.can_undo();
    let redo = s.can_redo();
    let count = s.native_assets.ids().count();
    let overviews = s.source_overviews();
    let peak_usage = s.source_overview_usage();
    assert!(
        s.import_device_asset(fixture.decoded(), cancel, None)
            .is_err()
    );
    assert_eq!(s.project(), &before);
    assert_eq!(s.can_undo(), undo);
    assert_eq!(s.can_redo(), redo);
    assert_eq!(s.native_assets.ids().count(), count);
    assert!(s.source_overviews().shares_storage_with(&overviews));
    assert_eq!(s.source_overview_usage(), peak_usage);
}
#[test]
fn native_asset_import_keeps_original_pcm_capacity_and_rejects_clip_reuse() {
    let fixture = Fixture::new();
    let decoded = fixture.decoded();
    let original = decoded.audio().frames().as_ptr();
    let capacity = decoded.audio().pcm_capacity_frames();
    let mut s = session();
    let (created, prepared) = s
        .import_device_asset(decoded, &AtomicBool::new(false), None)
        .unwrap();
    let id = asset(created);
    let metadata = s.project().asset(id).unwrap();
    assert_eq!(metadata.path(), Path::new(&fixture.0));
    assert_eq!(
        (
            metadata.source_sample_rate(),
            metadata.source_frames(),
            metadata.playback_frames(),
            metadata.conversion_version()
        ),
        (8000, 4, 8, hex_project::SINC_CONVERSION_V2)
    );
    assert!(s.audio(id).is_none());
    assert!(s.media.is_empty());
    let source = s.native_source(id).unwrap();
    assert_eq!(source.sample_rate(), 8000);
    assert_eq!(source.resident_frames().unwrap().as_ptr(), original);
    assert_eq!(
        source.resident_frames().unwrap(),
        &[[0.125, 0.125], [-0.25, -0.25], [0.5, 0.5], [0.0, 0.0]]
    );
    assert_eq!(s.native_assets.capacity_frames().unwrap(), capacity as u64);
    assert!(s.can_undo());
    let Created::Track(track) = s
        .command(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let before = s.project().clone();
    assert!(
        matches!(s.command(Command::PlaceClip { track, asset: id, start: 0, duration: 8, source_offset: 0, source_length: 8, looping: false }), Err(SessionError::DeviceAssetClipUnsupported(found)) if found == id)
    );
    assert_eq!(s.project(), &before);
    // An unassigned source does not change ordinary compiled playback.
    assert_eq!(prepared.arrangement().sample_rate(), 16000);
}
#[test]
fn native_asset_import_exact_budget_one_under_and_staging_overlap() {
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut measured = session();
    measured
        .import_device_asset(fixture.decoded(), &cancel, None)
        .unwrap();
    let retained = measured.native_storage_on_control().unwrap().retained_bytes;
    // First construction has one extra Vec header; current store is empty.
    let peak = retained + mem::size_of::<Vec<(AssetId, Box<()>)>>();
    for (retained_limit, peak_limit, succeeds) in [
        (retained, peak, true),
        (retained - 1, peak, false),
        (retained, peak - 1, false),
    ] {
        let mut r = reservation();
        r.storage.max_retained_bytes = retained_limit;
        r.storage.max_construction_peak_bytes = peak_limit;
        let mut s = Session::new(16000, 8, 1024)
            .unwrap()
            .with_native_reservation(r)
            .unwrap();
        if succeeds {
            s.import_device_asset(fixture.decoded(), &cancel, None)
                .unwrap();
        } else {
            assert_refused_unchanged(&mut s, &fixture, &cancel);
        }
    }
    // Equal content decoded independently owns a separate buffer and is charged.
    let first = measured.native_assets.ids().next().unwrap();
    let old_pointer = measured
        .native_source(first)
        .unwrap()
        .resident_frames()
        .unwrap()
        .as_ptr();
    let before = measured.native_storage_on_control().unwrap();
    measured
        .import_device_asset(fixture.decoded(), &cancel, None)
        .unwrap();
    let second = measured.native_assets.ids().last().unwrap();
    assert_ne!(
        measured
            .native_source(second)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr(),
        old_pointer
    );
    assert!(measured.native_storage_on_control().unwrap().retained_bytes > before.retained_bytes);
    assert_eq!(
        measured
            .native_source(first)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr(),
        old_pointer
    );
}
#[test]
fn native_asset_import_cancel_work_rows_frames_and_model_failure_are_atomic() {
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut unconfigured = Session::new(16000, 8, 1024).unwrap();
    assert_refused_unchanged(&mut unconfigured, &fixture, &cancel);
    let mut s = session();
    s.import_device_asset(fixture.decoded(), &cancel, None)
        .unwrap();
    s.undo_prepared_revision(None).unwrap();
    assert!(s.can_redo());
    assert_refused_unchanged(&mut s, &fixture, &AtomicBool::new(true));
    for kind in 0..4 {
        let mut r = reservation();
        match kind {
            0 => r.max_scan_frames = 3,
            1 => r.storage.root_rows = 1,
            2 => r.storage.data_rows = 4,
            _ => r.storage.handle_rows = 0,
        }
        let mut s = Session::new(16000, 8, 1024)
            .unwrap()
            .with_native_reservation(r)
            .unwrap();
        assert_refused_unchanged(&mut s, &fixture, &cancel);
    }
    let mut s = Session::new(16000, 8, 3)
        .unwrap()
        .with_native_reservation(reservation())
        .unwrap();
    assert_refused_unchanged(&mut s, &fixture, &cancel);
}

#[test]
fn native_asset_import_model_id_exhaustion_preserves_custody_and_history() {
    let fixture = Fixture::new();
    let mut s = session();
    let mut snapshot = s.project().snapshot();
    snapshot.next_id = u64::MAX;
    s.editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    assert_refused_unchanged(&mut s, &fixture, &AtomicBool::new(false));
}

#[test]
fn native_asset_import_service_returns_actual_id_and_canceled_request_preserves_revision() {
    use crate::service::{Event, Request, Service};
    fn event(service: &Service) -> Event {
        let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
        loop {
            match service.try_event() {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty)
                    if std::time::Instant::now() < deadline =>
                {
                    crate::test_wait::pause()
                }
                Err(reason) => panic!("worker event: {reason}"),
            }
        }
    }
    let fixture = Fixture::new();
    let service = Service::with_native_reservation(16000, 8, 1024, reservation()).unwrap();
    event(&service);
    service
        .submit(Request::ImportDeviceAsset {
            path: fixture.0.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let imported = event(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let id = asset(imported.created.unwrap());
    let metadata = imported.state.project.asset(id).unwrap();
    assert_eq!(
        (
            metadata.source_sample_rate(),
            metadata.source_frames(),
            metadata.playback_frames()
        ),
        (8000, 4, 8)
    );
    assert_eq!(metadata.path(), fixture.0.as_path());
    assert!(imported.state.project.tracks().is_empty());
    assert!(imported.playback.is_some());
    // Real stereo WAV through the same new worker request, with unequal channels.
    let stereo = Fixture::new();
    let mut bytes = fs::read(&stereo.0).unwrap();
    bytes[22..24].copy_from_slice(&2_u16.to_le_bytes());
    bytes[28..32].copy_from_slice(&32000_u32.to_le_bytes());
    bytes[32..34].copy_from_slice(&4_u16.to_le_bytes());
    fs::write(&stereo.0, bytes).unwrap();
    let stereo_service = Service::with_native_reservation(16000, 8, 1024, reservation()).unwrap();
    event(&stereo_service);
    stereo_service
        .submit(Request::ImportDeviceAsset {
            path: stereo.0.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let stereo_event = event(&stereo_service);
    assert!(stereo_event.error.is_none(), "{:?}", stereo_event.error);
    let stereo_id = asset(stereo_event.created.unwrap());
    let metadata = stereo_event.state.project.asset(stereo_id).unwrap();
    assert_eq!(
        (
            metadata.source_sample_rate(),
            metadata.source_frames(),
            metadata.playback_frames()
        ),
        (8000, 2, 4)
    );
    let mut direct = session();
    let decoded = stereo.decoded();
    let pointer = decoded.audio().frames().as_ptr();
    let id = asset(
        direct
            .import_device_asset(decoded, &AtomicBool::new(false), None)
            .unwrap()
            .0,
    );
    let pcm = direct.native_source(id).unwrap().resident_frames().unwrap();
    assert_eq!(pcm, &[[0.125, -0.25], [0.5, 0.0]]);
    assert_eq!(pcm.as_ptr(), pointer);
    service
        .submit(Request::ImportDeviceAsset {
            path: fixture.0.clone(),
            cancel: Arc::new(AtomicBool::new(true)),
        })
        .unwrap();
    let refused = event(&service);
    assert!(refused.error.is_some());
    assert!(refused.created.is_none());
    assert_eq!(refused.state.revision, imported.state.revision);
    assert_eq!(refused.state.project, imported.state.project);
    // The legacy constructor stays opt-out, with a visible refusal.
    let legacy = Service::new(16000, 8, 1024).unwrap();
    event(&legacy);
    legacy
        .submit(Request::ImportDeviceAsset {
            path: fixture.0.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let refused = event(&legacy);
    assert!(
        refused
            .error
            .unwrap()
            .contains("explicit resident reservation")
    );
    assert!(refused.state.project.assets().next().is_none());
}

#[test]
fn native_asset_import_and_ordinary_import_share_frame_and_byte_limits() {
    let fixture = Fixture::new();
    let mut s = session();
    s.import_device_asset(fixture.decoded(), &AtomicBool::new(false), None)
        .unwrap();
    let current = s.native_storage_on_control().unwrap().retained_bytes;
    let ordinary = PreparedImport::new(fixture.decoded()).unwrap();
    let additional = ordinary.retained_pcm_frames().unwrap();
    let mut r = reservation();
    r.storage.max_retained_bytes = current + pcm_bytes(additional).unwrap() - 1;
    s = s.with_native_reservation(r).unwrap();
    let before = s.project().clone();
    let count = s.native_assets.ids().count();
    assert!(s.import(ordinary).is_err());
    assert_eq!(s.project(), &before);
    assert!(s.media.is_empty());
    assert_eq!(s.native_assets.ids().count(), count);
    // Existing AddAsset rate refusal is still strict despite the new command.
    let mut model = Editor::new(Project::new(16000).unwrap(), 8).unwrap();
    assert!(
        model
            .execute(Command::AddAsset {
                path: fixture.0.clone(),
                sample_rate: 8000,
                frames: 4
            })
            .is_err()
    );
    let mut limited = Session::new(8000, 8, 7)
        .unwrap()
        .with_native_reservation(reservation())
        .unwrap();
    limited
        .import_device_asset(fixture.decoded(), &AtomicBool::new(false), None)
        .unwrap();
    let before = limited.project().clone();
    assert!(
        limited
            .import(PreparedImport::new(fixture.decoded()).unwrap())
            .is_err()
    );
    assert_eq!(limited.project(), &before);
}

#[test]
fn native_asset_import_cancellation_at_final_admission_preserves_real_editor_and_media() {
    let fixture = Fixture::new();
    let mut s = session();
    let cancel = AtomicBool::new(false);
    let first = asset(
        s.import_device_asset(fixture.decoded(), &cancel, None)
            .unwrap()
            .0,
    );
    let pointer = s
        .native_source(first)
        .unwrap()
        .resident_frames()
        .unwrap()
        .as_ptr();
    let before = s.project().clone();
    let storage = s.native_storage_on_control().unwrap();
    let history = (s.can_undo(), s.can_redo());
    let result = s.import_device_asset_with_admission(fixture.decoded(), &cancel, None, || {
        cancel.store(true, Ordering::Relaxed);
        Ok(())
    });
    assert!(matches!(
        result,
        Err(SessionError::NativeResource(
            ResourcePreparationError::Cancelled
        ))
    ));
    assert_eq!(s.project(), &before);
    assert_eq!((s.can_undo(), s.can_redo()), history);
    assert_eq!(s.native_storage_on_control().unwrap(), storage);
    assert_eq!(
        s.native_source(first)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr(),
        pointer
    );
}

#[test]
fn native_asset_import_preserves_positive_ordinary_track_and_clip_imports() {
    let fixture = Fixture::new();
    let mut s = session();
    let native = asset(
        s.import_device_asset(fixture.decoded(), &AtomicBool::new(false), None)
            .unwrap()
            .0,
    );
    let pointer = s
        .native_source(native)
        .unwrap()
        .resident_frames()
        .unwrap()
        .as_ptr();
    let prepare = || {
        PreparedImport::at_rate(
            fixture.decoded(),
            16000,
            ResampleLimits {
                max_source_frames: 1024,
                max_output_frames: 1024,
            },
        )
        .unwrap()
    };
    let Created::Imported { track, .. } = s.import(prepare()).unwrap() else {
        panic!("new ordinary audio track")
    };
    assert_eq!(s.project().tracks()[0].clips().len(), 1);
    let first = s.project().tracks()[0].clips()[0].asset();
    assert_ne!(first, native);
    assert_eq!(s.audio(first).unwrap().frames().len(), 8);
    s.import_prepared_audio_clip_revision(prepare(), track, 16, None)
        .unwrap();
    assert_eq!(s.project().tracks()[0].clips().len(), 2);
    // The unchanged file re-imports onto its ordinary resident asset, never
    // onto the device asset decoded from the same file.
    assert_eq!(s.project().tracks()[0].clips()[1].asset(), first);
    assert_eq!(s.project().assets().count(), 2);
    assert!(s.audio(native).is_none());
    assert_eq!(
        s.native_source(native)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr(),
        pointer
    );
}

#[test]
fn first_sampler_staged_prepare_failure_or_late_cancel_keeps_source_model_and_history() {
    for late_cancel in [false, true] {
        let fixture = Fixture::new();
        let mut s = session();
        let cancel = AtomicBool::new(false);
        let first = asset(
            s.import_device_asset(fixture.decoded(), &cancel, None)
                .unwrap()
                .0,
        );
        let Created::Track(track) = s
            .editor
            .execute(Command::AddMidiTrack {
                name: "target".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let pointer = s
            .native_source(first)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr();
        let before = s.project().clone();
        let storage = s.native_storage_on_control().unwrap();
        let history = (s.can_undo(), s.can_redo());
        let (state, source_slot) =
            hex_engine::native_device_resources::sampler_import_template_on_control(4).unwrap();
        let finalized = std::cell::Cell::new(false);
        let refused = s.import_device_asset_prepared(
            fixture.decoded(),
            &cancel,
            NativeAssetDestination::NewInstrument {
                track,
                replacing: None,
                state,
                source_slot,
            },
            |candidate, _, _, _, _, _, _| {
                assert_eq!(candidate.assets().count(), before.assets().count() + 1);
                assert!(candidate.track(track).unwrap().instrument().is_some());
                if late_cancel {
                    cancel.store(true, Ordering::Release);
                    Ok(())
                } else {
                    Err(SessionError::Invalid("injected staged preparation refusal"))
                }
            },
            |_, _, _| {
                finalized.set(true);
                Ok(())
            },
        );
        assert!(refused.is_err());
        assert!(
            !finalized.get(),
            "refusal/cancellation precedes final publication admission"
        );
        assert_eq!(s.project(), &before);
        assert_eq!(s.native_storage_on_control().unwrap(), storage);
        assert_eq!((s.can_undo(), s.can_redo()), history);
        assert_eq!(
            s.native_source(first)
                .unwrap()
                .resident_frames()
                .unwrap()
                .as_ptr(),
            pointer
        );
    }
}

#[test]
fn accepted_native_import_finalizer_has_no_later_cancellation_rollback() {
    let fixture = Fixture::new();
    let mut s = session();
    let cancel = AtomicBool::new(false);
    let (created, ()) = s
        .import_device_asset_prepared(
            fixture.decoded(),
            &cancel,
            NativeAssetDestination::Unassigned,
            |_, _, _, _, _, _, _| Ok(()),
            |_, _, _stage| {
                // Model cancellation racing after successful queue acceptance. Once this final
                // operation accepts, the prepared source/history transaction must commit.
                cancel.store(true, Ordering::Release);
                Ok(())
            },
        )
        .unwrap();
    let created = asset(created);
    assert!(s.project().asset(created).is_some());
    assert!(s.native_source(created).is_some());
    assert!(s.can_undo());
}

#[test]
fn peak_budget_and_late_precommit_cancellation_preserve_source_editor_history_and_cache() {
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let standard = crate::source_overview::OverviewBudget::standard()
        .unwrap()
        .usage();
    let mut s = session();
    // This fixture has four original frames: four signed stereo peak leaves
    // occupy 64 bytes. Short sources no longer reserve a full overview array.
    s.overview_budget =
        crate::source_overview::OverviewBudget::with_limits(63, standard.max_metadata_bytes)
            .unwrap();
    assert_refused_unchanged(&mut s, &fixture, &cancel);
    s.overview_budget =
        crate::source_overview::OverviewBudget::with_limits(128, standard.max_metadata_bytes)
            .unwrap();
    s.import_device_asset(fixture.decoded(), &cancel, None)
        .unwrap();
    let before = s.project().snapshot();
    let history = (s.can_undo(), s.can_redo());
    let cache = s.source_overviews();
    let usage = s.source_overview_usage();
    assert_eq!(usage.bin_capacity_bytes, 64);
    assert!(
        s.import_device_asset_with_admission(fixture.decoded(), &cancel, None, || {
            cancel.store(true, Ordering::Release);
            Ok(())
        })
        .is_err()
    );
    assert_eq!(s.project().snapshot(), before);
    assert_eq!((s.can_undo(), s.can_redo()), history);
    assert!(s.source_overviews().shares_storage_with(&cache));
    assert_eq!(s.source_overview_usage(), usage);
    assert_eq!(s.native_assets.ids().count(), 1);
}
