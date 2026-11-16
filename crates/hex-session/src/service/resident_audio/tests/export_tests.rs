//! Real ordinary export ownership outlives the canonical document and UI completion polling.
use super::*;
use std::time::Instant;

fn receive(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
}
fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = receive(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn load(service: &Service, path: &std::path::Path, request_id: u64, generation: u64) -> Event {
    let added = run(
        service,
        Request::Edit(Command::AddMidiTrack {
            name: "source".into(),
        }),
    );
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    run(
        service,
        Request::LoadFirstSampler {
            request_id,
            project_generation: generation,
            revision: added.state.revision,
            track,
            slot: SamplerLoadSlot::New(hex_project::InstrumentLoadPlacement::First),
            path: path.to_owned(),
            cancel: Arc::new(AtomicBool::new(false)),
        },
    )
}
fn service() -> Service {
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
        .checked_mul(2)
        .unwrap();
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8,
        None,
        Box::new(OfflineSink {
            observed: Arc::new(Observed::default()),
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    .checked_add(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
                    .unwrap(),
            },
            max_scan_frames: 8192,
        },
    )
    .unwrap();
    assert!(receive(&service).error.unwrap().contains("fixture offline"));
    service
}

#[test]
fn ordinary_export_source_remains_charged_through_new_cancel_and_physical_completion() {
    // Four original frames per bank. A+B fits8; A+B+C cannot. New actually retires
    // Session/history/current/pending A, leaving only the accepted export's real owners.
    // Cancellation and public result polling are deliberately not retirement evidence.
    for cancel in [false, true] {
        let a = sampler_export::tests::Files::new(false);
        let b = sampler_export::tests::Files::new(false);
        let c = sampler_export::tests::Files::new(false);
        let mut other_pcm = std::fs::read(b.source()).unwrap();
        for sample in other_pcm[44..].chunks_exact_mut(2) {
            sample.copy_from_slice(&(-8192_i16).to_le_bytes());
        }
        std::fs::write(b.source(), other_pcm).unwrap();
        let service = service();
        let loaded = load(&service, a.source(), 1, 0);
        let track = loaded.state.project.tracks()[0].id();
        let placed = run(
            &service,
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::ONE,
            }),
        );
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("clip")
        };
        run(
            &service,
            Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            }),
        );
        let (started, entered) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        *service.export_test_hook.lock().unwrap() = Some(Box::new(move || {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
        }));
        let control = hex_export::ExportControl::default();
        run(
            &service,
            Request::ExportRange {
                path: a.output().to_owned(),
                start: 0,
                frames: Some(12),
                signal: hex_export::ExportSignal::Main,
                control: control.clone(),
                format: hex_export::ExportFormat::Pcm16,
                overwrite: false,
            },
        );
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let fresh = run(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::Resident,
                reservation: None,
            },
        );
        assert!(fresh.project_replaced);
        assert!(fresh.state.project.tracks().is_empty());
        assert!(!fresh.state.can_undo);
        let current = load(&service, b.source(), 2, 1);
        if cancel {
            control.cancel();
        }
        let import = || Request::ImportDeviceAsset {
            path: c.source().to_owned(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        service.submit(import()).unwrap();
        let refused = receive(&service);
        assert!(refused.error.is_some(), "A is still owned by paused export");
        assert_eq!(refused.state.project, current.state.project);
        assert_eq!(refused.state.revision, current.state.revision);
        assert_eq!(refused.state.can_undo, current.state.can_undo);
        assert!(matches!(
            service.export_events.try_recv(),
            Err(TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        // Only the real worker's post-drop terminal (consumed by canonical control)
        // can release A. Do not poll the public ExportEvent before admission succeeds.
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        loop {
            service.submit(import()).unwrap();
            let imported = receive(&service);
            if imported.error.is_none() {
                assert!(matches!(imported.created, Some(Created::Asset(_))));
                break;
            }
            assert_eq!(imported.state.project, current.state.project);
            assert!(Instant::now() < deadline, "{:?}", imported.error);
            thread::sleep(Duration::from_millis(1));
        }
        let exported = service
            .export_events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert_eq!(exported.result.is_err(), cancel);
        if cancel {
            assert!(!a.output().exists());
        } else {
            let bytes = std::fs::read(a.output()).unwrap();
            let left = bytes[44..]
                .chunks_exact(4)
                .map(|frame| i16::from_le_bytes([frame[0], frame[1]]))
                .collect::<Vec<_>>();
            let expected = [
                4096, 6144, 8192, 10240, 12288, 14336, 16384, 12970, 9557, 6144, 2731, -683,
            ];
            // PCM16 export applies ±1 LSB TPDF dither.
            assert_eq!(left.len(), expected.len());
            for (actual, expected) in left.iter().zip(expected) {
                assert!((i32::from(*actual) - expected).abs() <= 1, "{left:?}");
            }
        }
    }
}

#[test]
fn ordinary_export_scope_budget_and_failed_dispatch_refuse_without_publishing() {
    let files = sampler_export::tests::Files::new(false);
    let mut session = interactive_sampler_session(&files);
    let mut sink = OfflineSink {
        observed: Arc::new(Observed::default()),
        pending: None,
    };
    let snapshot = session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap()
        .prepare(
            session.project(),
            &mut sink,
            &session.media_bank(),
            session.sine_table(),
            &session.audio_transforms,
            &session.audio_clip_waveforms,
            hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 0,
            },
        )
        .unwrap();
    assert!(snapshot.prepared.requires_device_resources());
    let source = sampler_export::ServiceExportSnapshot {
        prepared: Arc::clone(&snapshot.prepared),
        sources: None,
    };
    let (work, receiver) = mpsc::sync_channel(1);
    drop(receiver);
    let (_finished, completion) = mpsc::sync_channel(1);
    let (stop, wait) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || wait.recv().unwrap());
    let mut lane = ExportLane::new(work, completion, worker);
    struct StopWorker(mpsc::SyncSender<()>);
    impl Drop for StopWorker {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    // Release the worker before lane's joining Drop, including assertion unwinds.
    let _stop_worker = StopWorker(stop);
    let before = session.project().clone();
    let refs = Arc::strong_count(&snapshot.prepared);
    let mut missing = sampler_export::tests::job(&source, files.output().to_owned());
    assert!(
        lane.prepare_job(&mut missing, &session, None, None, None, None)
            .unwrap_err()
            .contains("matching resident resource snapshot")
    );
    drop(missing);
    // Even an equivalent separately compiled graph is not the captured compiler owner.
    let foreign = session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap()
        .prepare(
            session.project(),
            &mut sink,
            &session.media_bank(),
            session.sine_table(),
            &session.audio_transforms,
            &session.audio_clip_waveforms,
            hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 0,
            },
        )
        .unwrap();
    let mut wrong = sampler_export::tests::job(&source, files.output().to_owned());
    assert!(
        lane.prepare_job(
            &mut wrong,
            &session,
            None,
            Some((&foreign, &sink)),
            None,
            None
        )
        .unwrap_err()
        .contains("source scope differs")
    );
    assert!(wrong.resources.is_none());
    drop(wrong);
    let limits = session.native_reservation.unwrap();
    session
        .native_reservation
        .as_mut()
        .unwrap()
        .storage
        .max_retained_bytes = 1;
    let mut refused = sampler_export::tests::job(&source, files.output().to_owned());
    assert!(
        lane.prepare_job(
            &mut refused,
            &session,
            None,
            Some((&snapshot, &sink)),
            None,
            None
        )
        .is_err()
    );
    assert!(refused.resources.is_none());
    assert!(lane.active.is_none());
    drop(refused);
    session.native_reservation = Some(limits);
    let mut accepted = sampler_export::tests::job(&source, files.output().to_owned());
    lane.prepare_job(
        &mut accepted,
        &session,
        None,
        Some((&snapshot, &sink)),
        None,
        None,
    )
    .unwrap();
    assert!(accepted.resources.is_some());
    assert!(lane.active.is_some());
    // A disconnected queue returns/drops the exact never-transferred job before its
    // anchor is cleared. No authored/history/output mutation accompanies this refusal.
    assert!(lane.submit(accepted).is_err());
    assert!(lane.active.is_none());
    assert_eq!(Arc::strong_count(&snapshot.prepared), refs);
    assert_eq!(session.project(), &before);
    assert!(!files.output().exists());
}
