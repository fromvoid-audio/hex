mod first_load;
mod history;
mod overview;
mod replacement;
mod save_open;
use super::*;
use std::{
    fs,
    sync::atomic::AtomicU64,
    time::{Duration, Instant},
};
pub(crate) struct Files {
    source: PathBuf,
    output: PathBuf,
}
impl Files {
    pub(crate) fn output(&self) -> &Path {
        &self.output
    }
    pub(crate) fn source(&self) -> &Path {
        &self.source
    }
    pub(crate) fn new(stereo: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hex-sampler-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let source = dir.join("original.wav");
        let output = dir.join("out.wav");
        let samples = if stereo {
            vec![4096_i16, -8192, 16384, 0, -4096, 8192, 0, 0]
        } else {
            vec![4096_i16, 16384, -4096, 0]
        };
        let channels = if stereo { 2_u16 } else { 1 };
        let data = (samples.len() * 2) as u32;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&8000_u32.to_le_bytes());
        bytes.extend_from_slice(&(8000_u32 * u32::from(channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        fs::write(&source, bytes).unwrap();
        Self { source, output }
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.source.parent().unwrap());
    }
}
fn reservation() -> crate::ResidentNativeReservation {
    crate::ResidentNativeReservation {
        storage: hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 64,
            root_rows: 64,
            max_retained_bytes: 20_000_000,
            max_construction_peak_bytes: 30_000_000,
        },
        max_scan_frames: 1024,
    }
}
fn event(s: &Service) -> Event {
    // Blocks on the event channel; bounded only against hangs.
    match s.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("event: worker event absent: {error}"),
    }
}
fn export_event(s: &Service) -> ExportEvent {
    let end = Instant::now() + crate::test_wait::HANG_BOUND;
    loop {
        match s.try_export_event() {
            Ok(e) => return e,
            Err(TryRecvError::Empty) if Instant::now() < end => {
                thread::sleep(Duration::from_millis(1))
            }
            Err(e) => panic!("export event {e}"),
        }
    }
}
fn edit(s: &Service, c: Command) -> Event {
    s.submit(Request::Edit(c)).unwrap();
    let e = event(s);
    assert!(e.error.is_none(), "{:?}", e.error);
    e
}
fn assign(s: &Service, path: PathBuf) -> Event {
    s.submit(Request::ImportDeviceAsset {
        path,
        cancel: Arc::new(AtomicBool::new(false)),
    })
    .unwrap();
    let imported = event(s);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let Some(Created::Asset(asset)) = imported.created else {
        panic!("asset")
    };
    let Some(Created::Track(track)) = edit(
        s,
        Command::AddMidiTrack {
            name: "sampler".into(),
        },
    )
    .created
    else {
        panic!("track")
    };
    let Some(Created::Clip(clip)) = edit(
        s,
        Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        },
    )
    .created
    else {
        panic!("clip")
    };
    edit(
        s,
        Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        },
    );
    s.submit(Request::AssignSampler { track, asset }).unwrap();
    let e = event(s);
    assert!(e.error.is_none(), "{:?}", e.error);
    e
}
#[test]
fn sampler_service_real_mono_stereo_assign_and_controlled_file_pcm() {
    for stereo in [false, true] {
        let files = Files::new(stereo);
        let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
        assert!(event(&service).error.is_none());
        let assigned = assign(&service, files.source.clone());
        assert!(assigned.playback.is_none());
        assert!(
            assigned
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .sources()
                .is_empty(),
            "public context must not own original sampler PCM"
        );
        assert!(assigned.device_preview.is_none());
        assert!(
            assigned
                .status
                .unwrap()
                .contains("native playback unavailable")
        );
        let control = hex_export::ExportControl::default();
        service
            .submit(Request::ExportRange {
                path: files.output.clone(),
                start: 0,
                frames: Some(12),
                signal: hex_export::ExportSignal::Main,
                control,
                format: hex_export::ExportFormat::Pcm16,
                overwrite: false,
            })
            .unwrap();
        let queued = event(&service);
        assert!(queued.error.is_none(), "{:?}", queued.error);
        assert!(queued.playback.is_none());
        let complete = export_event(&service);
        assert!(complete.result.is_ok(), "{:?}", complete.result.err());
        let bytes = fs::read(&files.output).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16000);
        let pcm = bytes[44..]
            .chunks_exact(4)
            .map(|f| {
                [
                    i16::from_le_bytes([f[0], f[1]]),
                    i16::from_le_bytes([f[2], f[3]]),
                ]
            })
            .collect::<Vec<_>>();
        let left = [4096, 10240, 16384, 6144, -4096, -2048, 0, 0, 0, 0, 0, 0];
        let right = if stereo {
            [-8192, -4096, 0, 4096, 8192, 4096, 0, 0, 0, 0, 0, 0]
        } else {
            left
        };
        assert_pcm_near(
            &pcm,
            &left
                .into_iter()
                .zip(right)
                .map(|(l, r)| [l, r])
                .collect::<Vec<_>>(),
            "controlled file pcm",
        );
        service
            .submit(Request::RestorePlayback { generation: 7 })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert!(refused.playback.is_none());
    }
}

fn block_export(service: &Service) -> (mpsc::Receiver<()>, mpsc::SyncSender<()>) {
    let (started, entered) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    *service.export_test_hook.lock().unwrap() = Some(Box::new(move || {
        started.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(10)).unwrap();
    }));
    (entered, release)
}

fn queue_export(service: &Service, files: &Files, control: hex_export::ExportControl) {
    service
        .submit(Request::ExportRange {
            path: files.output.clone(),
            start: 0,
            frames: Some(12),
            signal: hex_export::ExportSignal::Main,
            control,
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        })
        .unwrap();
    assert!(event(service).error.is_none());
}

#[test]
fn captured_sampler_survives_new_and_ordinary_open_and_capability_persists() {
    let files = Files::new(true);
    let second = Files::new(false);
    let saved = files.source.parent().unwrap().join("ordinary.hex");
    let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
    event(&service);
    // Save an actual ordinary document; opening it must preserve the explicit
    // service capability and must not replace the independently captured export.
    edit(
        &service,
        Command::AddTrack {
            name: "ordinary".into(),
        },
    );
    service
        .submit(Request::Import(files.source.clone()))
        .unwrap();
    assert!(event(&service).error.is_none());
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    assert!(event(&service).error.is_none());
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    assert!(event(&service).error.is_none());
    assign(&service, files.source.clone());
    let (entered, release) = block_export(&service);
    queue_export(&service, &files, hex_export::ExportControl::default());
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    let fresh = event(&service);
    assert!(fresh.error.is_none(), "{:?}", fresh.error);
    assert!(fresh.project_replaced);
    assert!(fresh.state.project.tracks().is_empty());
    assign(&service, second.source.clone());
    service.submit(Request::Open(saved)).unwrap();
    let opened = event(&service);
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(opened.project_replaced);
    assert_eq!(opened.state.project.assets().count(), 1);
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    assert!(event(&service).error.is_none());
    assign(&service, second.source.clone());
    release.send(()).unwrap();
    assert!(export_event(&service).result.is_ok());
    let bytes = fs::read(&files.output).unwrap();
    assert_pcm_bytes_near(&bytes[44..52], &[0, 16, 0, 224, 0, 40, 0, 240]);
    // The next real export uses the new document's mono source, after the old
    // terminal is drained by control rather than by resource-bearing UI events.
    queue_export(&service, &second, hex_export::ExportControl::default());
    assert!(export_event(&service).result.is_ok());
    let bytes = fs::read(&second.output).unwrap();
    assert_pcm_bytes_near(&bytes[44..52], &[0, 16, 0, 16, 0, 40, 0, 40]);
}

#[test]
fn sampler_export_cancel_and_file_failure_release_lane_for_next_job() {
    let files = Files::new(false);
    let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
    event(&service);
    assign(&service, files.source.clone());
    let control = hex_export::ExportControl::default();
    let (entered, release) = block_export(&service);
    queue_export(&service, &files, control.clone());
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    control.cancel();
    release.send(()).unwrap();
    assert!(export_event(&service).result.is_err());
    assert!(!files.output.exists());
    fs::write(&files.output, b"preserve").unwrap();
    queue_export(&service, &files, hex_export::ExportControl::default());
    assert!(export_event(&service).result.is_err());
    assert_eq!(fs::read(&files.output).unwrap(), b"preserve");
    fs::remove_file(&files.output).unwrap();
    queue_export(&service, &files, hex_export::ExportControl::default());
    assert!(export_event(&service).result.is_ok());
}

#[test]
fn terminal_reclaims_on_idle_control_without_ui_export_polling() {
    let files = Files::new(false);
    let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
    event(&service);
    let worker_id = service.worker.as_ref().unwrap().thread().id();
    let (_observer, dropped) =
        hex_engine::native_device_resources::control_drop_probe::observe(worker_id);
    assign(&service, files.source.clone());
    let (entered, release) = block_export(&service);
    queue_export(&service, &files, hex_export::ExportControl::default());
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    assert!(event(&service).error.is_none());
    assert!(matches!(dropped.try_recv(), Err(TryRecvError::Empty)));
    release.send(()).unwrap();
    assert_eq!(
        dropped.recv_timeout(Duration::from_secs(5)).unwrap(),
        worker_id
    );
    assert!(matches!(dropped.try_recv(), Err(TryRecvError::Empty)));
    assert!(export_event(&service).result.is_ok());
}

#[test]
fn dropping_service_is_prompt_and_joined_control_reclaims_blocked_export_source() {
    let files = Files::new(false);
    let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
    event(&service);
    let worker_id = service.worker.as_ref().unwrap().thread().id();
    let (_observer, dropped) =
        hex_engine::native_device_resources::control_drop_probe::observe(worker_id);
    assign(&service, files.source.clone());
    let (entered, release) = block_export(&service);
    queue_export(&service, &files, hex_export::ExportControl::default());
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    assert!(event(&service).error.is_none());
    let start = Instant::now();
    drop(service);
    assert!(start.elapsed() < Duration::from_millis(250));
    assert!(matches!(dropped.try_recv(), Err(TryRecvError::Empty)));
    release.send(()).unwrap();
    assert_eq!(
        dropped.recv_timeout(Duration::from_secs(5)).unwrap(),
        worker_id
    );
}

pub(crate) fn direct_sampler(files: &Files) -> (Session, ServiceExportSnapshot) {
    let mut session = Session::new(16000, 16, 1024)
        .unwrap()
        .with_native_reservation(reservation())
        .unwrap();
    let decoded = decode_wav_file(
        files.source.clone(),
        Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
    )
    .unwrap();
    let (Created::Asset(asset), snapshot) = session
        .import_service_snapshot(decoded, &AtomicBool::new(false), Admission::default())
        .unwrap()
    else {
        panic!("asset")
    };
    let (Created::Track(track), snapshot) = session
        .command_service_snapshot(
            Command::AddMidiTrack {
                name: "sampler".into(),
            },
            Admission {
                current: Some(snapshot.selection()),
                ..Admission::default()
            },
        )
        .unwrap()
    else {
        panic!("track")
    };
    let state = session
        .native_assets
        .sampler_state_on_control(asset)
        .unwrap();
    let (_, snapshot) = session
        .command_service_snapshot(
            Command::InsertInstrument { track, state },
            Admission {
                current: Some(snapshot.selection()),
                ..Admission::default()
            },
        )
        .unwrap();
    (session, snapshot)
}

pub(crate) fn job(snapshot: &ServiceExportSnapshot, path: PathBuf) -> ExportJob {
    ExportJob {
        job_id: 0,
        streamed: None,
        sources: None,
        resources: None,
        revision: 0,
        path,
        prepared: Arc::clone(&snapshot.prepared),
        frames: 12,
        controlled: None,
    }
}

#[test]
fn export_exact_and_one_under_union_limits_and_failed_queue_are_atomic() {
    let files = Files::new(false);
    let (mut session, snapshot) = direct_sampler(&files);
    let (work_tx, work_rx) = mpsc::sync_channel(1);
    // A live worker with a deliberately disconnected input exercises the real
    // returned-job path, without fabricating any source teardown notification.
    drop(work_rx);
    let (_finished_tx, finished_rx) = mpsc::sync_channel(1);
    let (stop, wait) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        wait.recv().unwrap();
    });
    let mut lane = ExportLane::new(work_tx, finished_rx, worker);
    // Drop before lane: even a failed admission assertion must release the
    // test worker before ExportLane joins it during unwinding.
    struct StopWorker(mpsc::SyncSender<()>);
    impl Drop for StopWorker {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let _stop_worker = StopWorker(stop);
    let mut first = job(&snapshot, files.output.clone());
    lane.prepare_job(
        &mut first,
        &session,
        snapshot.sources.as_ref(),
        None,
        None,
        None,
    )
    .unwrap();
    let active = lane.active.as_ref().unwrap();
    let selections = [
        snapshot.selection(),
        active.snapshot.selection(),
        ControlSamplerSelection {
            sources: first.sources.as_ref(),
            prepared: Some(&first.prepared),
        },
    ];
    let stores = [&session.native_assets];
    let phase = DeviceResourcePhase {
        registered: &[],
        stores: &stores,
        selections: &selections,
        outbound: &[],
        legacy: &[],
        checked_runtime_bytes: active.runtime_bytes,
        construction_bytes: 0,
    };
    let totals =
        resources::inspect_device_resource_union(&phase, &phase, reservation().storage).unwrap();
    drop(first);
    lane.active = None;
    let exact = hex_engine::AudioStorageLedgerReservation {
        max_retained_bytes: totals.retained.retained_bytes,
        max_construction_peak_bytes: totals.construction.construction_peak_bytes,
        ..reservation().storage
    };
    for limits in [
        hex_engine::AudioStorageLedgerReservation {
            max_retained_bytes: exact.max_retained_bytes - 1,
            ..exact
        },
        hex_engine::AudioStorageLedgerReservation {
            max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
            ..exact
        },
    ] {
        session.native_reservation.as_mut().unwrap().storage = limits;
        let mut refused = job(&snapshot, files.output.clone());
        assert!(
            lane.prepare_job(
                &mut refused,
                &session,
                snapshot.sources.as_ref(),
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(lane.active.is_none());
        assert!(refused.sources.is_none());
    }
    session.native_reservation.as_mut().unwrap().storage = exact;
    let mut accepted = job(&snapshot, files.output.clone());
    lane.prepare_job(
        &mut accepted,
        &session,
        snapshot.sources.as_ref(),
        None,
        None,
        None,
    )
    .unwrap();
    assert!(lane.submit(accepted).is_err());
    assert!(lane.active.is_none());
    assert!(!files.output.exists());
}

#[test]
fn sampler_unsupported_history_and_ranges_refuse_without_public_source_escape() {
    let files = Files::new(false);
    let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
    event(&service);
    let assigned = assign(&service, files.source.clone());
    let saved = files
        .source
        .parent()
        .unwrap()
        .join("unqualified-sampler.hex");
    for request in [Request::Undo, Request::Redo] {
        service.submit(request).unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.revision, assigned.state.revision);
        assert!(refused.playback.is_none());
        assert!(
            refused
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .sources()
                .is_empty()
        );
    }
    for (start, signal) in [
        (1, hex_export::ExportSignal::Main),
        (0, hex_export::ExportSignal::CapturedMix(vec![])),
    ] {
        service
            .submit(Request::ExportRange {
                path: files.output.clone(),
                start,
                frames: Some(12),
                signal,
                control: hex_export::ExportControl::default(),
                format: hex_export::ExportFormat::Pcm16,
                overwrite: false,
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.playback.is_none());
        assert!(export_event(&service).result.is_err());
        assert!(!files.output.exists());
    }
    // Construct a current-format metadata fixture through storage directly.
    // Missing content identity must refuse before attempting the missing media.
    hex_storage::save_new(&saved, &assigned.state.project).unwrap();
    fs::remove_file(&files.source).unwrap();
    service.submit(Request::Open(saved)).unwrap();
    let refused = event(&service);
    assert!(
        refused
            .error
            .unwrap()
            .contains("qualified original source reconstruction")
    );
    assert!(!refused.project_replaced);
    assert_eq!(refused.state.revision, assigned.state.revision);
    assert!(refused.playback.is_none());
}

#[test]
fn captured_export_new_and_media_open_check_complete_union_before_adoption() {
    let files = Files::new(true);
    let (session, snapshot) = direct_sampler(&files);
    let anchor = ServiceExportSnapshot {
        prepared: Arc::clone(&snapshot.prepared),
        sources: Some(
            snapshot
                .sources
                .as_ref()
                .unwrap()
                .capture_on_control()
                .unwrap(),
        ),
    };
    let source = anchor.sources.as_ref().unwrap();
    let (_dispatched_job_owner, obligation) = source.capture_for_dispatch_on_control(1).unwrap();
    let admission = Admission {
        registered: &[],
        seeks: None,
        preview: None,
        native: NativeSamplerReservationHold::default(),
        current: Some(snapshot.selection()),
        export: Some(anchor.selection()),
        export_resources: None,
        runtime_bytes: hex_engine::SessionPlayback::control_sampler_export_runtime_allowance(
            &snapshot.prepared,
            source,
        )
        .unwrap()
            + std::mem::size_of_val(&obligation),
        outbound: Some(resources::ControlSamplerOutbound {
            anchor: source,
            obligation: &obligation,
            job: 1,
        }),
    };
    let empty = Session::new(16000, 16, 1024)
        .unwrap()
        .with_native_reservation(reservation())
        .unwrap();
    let empty_prepared = compile(&empty, None).unwrap();
    let mut ordinary = Session::new(16000, 16, 1024).unwrap();
    import(
        &mut ordinary,
        files.source.clone(),
        None,
        Admission::default(),
        &mut Default::default(),
    )
    .unwrap();
    let saved = files.source.parent().unwrap().join("media.hex");
    hex_storage::save_new(&saved, ordinary.project()).unwrap();
    let (opened, _, _, _) = recovery::open_source_with_limits(
        &session,
        16,
        &ProjectSource::Saved(saved),
        &BTreeMap::new(),
        DocumentAudioMode::Resident,
        None,
        Some(reservation()),
        admission.replacement_frame_allowance(&session).unwrap(),
        admission,
    )
    .unwrap();
    let opened_prepared = compile(&opened, None).unwrap();
    for (mut candidate, prepared, expected_frames) in
        [(empty, empty_prepared, 4), (opened, opened_prepared, 16)]
    {
        let selection = ControlSamplerSelection {
            sources: None,
            prepared: Some(&prepared),
        };
        let totals = admission
            .check(&session, &candidate, selection, 0, None)
            .unwrap();
        assert_eq!(totals.retained_frames, expected_frames);
        let exact = hex_engine::AudioStorageLedgerReservation {
            max_retained_bytes: totals.retained.retained_bytes,
            max_construction_peak_bytes: totals.construction.construction_peak_bytes,
            ..reservation().storage
        };
        candidate.native_reservation.as_mut().unwrap().storage = exact;
        admission
            .check(&session, &candidate, selection, 0, None)
            .unwrap();
        for limits in [
            hex_engine::AudioStorageLedgerReservation {
                max_retained_bytes: exact.max_retained_bytes - 1,
                ..exact
            },
            hex_engine::AudioStorageLedgerReservation {
                max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
                ..exact
            },
        ] {
            candidate.native_reservation.as_mut().unwrap().storage = limits;
            assert!(
                admission
                    .check(&session, &candidate, selection, 0, None)
                    .is_err()
            );
            assert_eq!(
                session.project().tracks().len(),
                1,
                "refused replacement has not adopted a document"
            );
        }
    }
}

#[test]
fn ordinary_import_cannot_spend_frames_still_owned_by_captured_export() {
    for kind in 0..3 {
        let exact = if kind == 2 { 28 } else { 16 };
        for limit in [exact - 1, exact] {
            let files = Files::new(false);
            let service =
                Service::with_native_reservation(16000, 16, limit, reservation()).unwrap();
            event(&service);
            assign(&service, files.source.clone());
            let (entered, release) = block_export(&service);
            queue_export(&service, &files, hex_export::ExportControl::default());
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            service
                .submit(Request::NewWithAudioMode {
                    mode: DocumentAudioMode::Resident,
                    reservation: None,
                })
                .unwrap();
            assert!(event(&service).error.is_none());
            let fresh = edit(
                &service,
                Command::AddTrack {
                    name: "ordinary".into(),
                },
            );
            let Some(Created::Track(track)) = fresh.created else {
                panic!("track")
            };
            let request = match kind {
                0 => Request::Import(files.source.clone()),
                1 => Request::ImportToTrack {
                    path: files.source.clone(),
                    track,
                    start: 0,
                },
                _ => Request::ImportAudioBatch {
                    paths: vec![files.source.clone(), files.source.clone()],
                    destination: crate::import_batch::AudioBatchDestination::Track {
                        track,
                        start: 0,
                    },
                    context: AudioBatchContext {
                        project_generation: 1,
                        revision: fresh.state.revision,
                        active_gesture: false,
                    },
                    control: Arc::new(
                        crate::import_batch_decode::AudioBatchControl::new(2).unwrap(),
                    ),
                    options: Default::default(),
                },
            };
            service.submit(request).unwrap();
            let outcome = event(&service);
            release.send(()).unwrap();
            if limit < exact {
                assert!(
                    outcome.error.is_some(),
                    "captured + whole staged original/derivative union exceeds limit, kind {kind}"
                );
                assert_eq!(outcome.state.revision, fresh.state.revision);
                assert_eq!(outcome.state.project.assets().count(), 0);
                assert_eq!(outcome.state.can_undo, fresh.state.can_undo);
            } else {
                assert!(outcome.error.is_none(), "{:?}", outcome.error);
                assert_eq!(
                    outcome.state.project.assets().count(),
                    if kind == 2 { 2 } else { 1 }
                );
            }
            assert!(export_event(&service).result.is_ok());
            if limit < exact {
                // Original capability survives refusal; terminal frees its part.
                service
                    .submit(Request::Import(files.source.clone()))
                    .unwrap();
                assert!(event(&service).error.is_none());
            }
        }
    }
}

#[test]
fn legacy_staged_owner_union_has_independent_exact_byte_limits_before_commit() {
    let files = Files::new(false);
    let (session, snapshot) = direct_sampler(&files);
    let anchor = ServiceExportSnapshot {
        prepared: Arc::clone(&snapshot.prepared),
        sources: Some(
            snapshot
                .sources
                .as_ref()
                .unwrap()
                .capture_on_control()
                .unwrap(),
        ),
    };
    let source = anchor.sources.as_ref().unwrap();
    let (_job, obligation) = source.capture_for_dispatch_on_control(1).unwrap();
    drop(session);
    drop(snapshot);
    let mut fresh = Session::new(16000, 16, 1024)
        .unwrap()
        .with_native_reservation(reservation())
        .unwrap();
    let admission = Admission {
        registered: &[],
        seeks: None,
        preview: None,
        native: NativeSamplerReservationHold::default(),
        current: None,
        export: Some(anchor.selection()),
        export_resources: None,
        outbound: Some(resources::ControlSamplerOutbound {
            anchor: source,
            obligation: &obligation,
            job: 1,
        }),
        runtime_bytes: hex_engine::SessionPlayback::control_sampler_export_runtime_allowance(
            &anchor.prepared,
            source,
        )
        .unwrap()
            + std::mem::size_of_val(&obligation),
    };
    let staged = PreparedImport::at_rate(
        decode_wav_file(
            &files.source,
            Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
        )
        .unwrap(),
        16000,
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: 1024,
        },
    )
    .unwrap();
    let totals = admission
        .legacy_stage_inventory(&fresh, std::slice::from_ref(&staged))
        .unwrap()
        .unwrap();
    assert_eq!(totals.retained_frames, 16);
    drop(staged);
    let exact = hex_engine::AudioStorageLedgerReservation {
        max_retained_bytes: totals.retained.retained_bytes,
        max_construction_peak_bytes: totals.construction.construction_peak_bytes,
        ..reservation().storage
    };
    for limits in [
        hex_engine::AudioStorageLedgerReservation {
            max_retained_bytes: exact.max_retained_bytes - 1,
            ..exact
        },
        hex_engine::AudioStorageLedgerReservation {
            max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
            ..exact
        },
    ] {
        fresh.native_reservation.as_mut().unwrap().storage = limits;
        assert!(
            import(
                &mut fresh,
                files.source.clone(),
                None,
                admission,
                &mut Default::default()
            )
            .is_err()
        );
        assert_eq!(fresh.project().assets().count(), 0);
        assert!(!fresh.can_undo());
        assert_eq!(fresh.resident_frames(), 0);
    }
    fresh.native_reservation.as_mut().unwrap().storage = exact;
    assert!(
        import(
            &mut fresh,
            files.source.clone(),
            None,
            admission,
            &mut Default::default()
        )
        .is_ok()
    );
    assert_eq!(fresh.project().assets().count(), 1);
    assert!(fresh.can_undo());
}

#[test]
fn private_sampler_edit_refuses_unverified_media_metadata_before_history_commit() {
    let mut results = Vec::new();
    for native in [false, true] {
        let files = Files::new(false);
        let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
        event(&service);
        let before = assign(&service, files.source.clone());
        let path = files.source.parent().unwrap().join("never-decoded.wav");
        let command = if native {
            Command::ImportDeviceAsset {
                path,
                sample_rate: 16000,
                frames: 4,
            }
        } else {
            Command::AddAsset {
                path,
                sample_rate: 16000,
                frames: 4,
            }
        };
        service.submit(Request::Edit(command)).unwrap();
        let result = event(&service);
        results.push(result.error.is_some());
        if result.error.is_some() {
            assert_eq!(result.state.project, before.state.project);
            assert_eq!(result.state.revision, before.state.revision);
            assert_eq!(result.state.can_undo, before.state.can_undo);
            assert_eq!(result.state.can_redo, before.state.can_redo);
            queue_export(&service, &files, hex_export::ExportControl::default());
            assert!(export_event(&service).result.is_ok());
            assert_pcm_bytes_near(
                &fs::read(&files.output).unwrap()[44..52],
                &[0, 16, 0, 16, 0, 40, 0, 40],
            );
        }
    }
    assert_eq!(
        results,
        [true, true],
        "generic Edit must not invent decoded media custody"
    );
}

/// PCM16 export applies ±1 LSB TPDF dither: compare decoded stereo frames
/// within one LSB of the plain-rounding expectation.
pub(super) fn assert_pcm_near(actual: &[[i16; 2]], expected: &[[i16; 2]], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}");
    for (actual, expected) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert!(
            (i32::from(*actual) - i32::from(*expected)).abs() <= 1,
            "{context}: {actual:?} not within one lsb of {expected:?}"
        );
    }
}

/// Same comparison for raw little-endian PCM16 stereo bytes.
fn assert_pcm_bytes_near(actual: &[u8], expected: &[u8]) {
    let decode = |bytes: &[u8]| {
        bytes
            .chunks_exact(4)
            .map(|f| {
                [
                    i16::from_le_bytes([f[0], f[1]]),
                    i16::from_le_bytes([f[2], f[3]]),
                ]
            })
            .collect::<Vec<_>>()
    };
    assert_pcm_near(&decode(actual), &decode(expected), "pcm bytes");
}
