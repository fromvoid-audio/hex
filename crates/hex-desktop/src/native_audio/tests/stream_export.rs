//! Actual Service/native capability and verified sources; no physical output.
use super::*;
use hex_stream::StreamConfig;

#[test]
fn verified_export_reopens_exact_sources_offline_without_resident_pcm_or_project_edits() {
    output_unavailable_export(false);
}
#[test]
fn grouped_verified_export_works_without_native_output() {
    output_unavailable_export(true);
}
fn output_unavailable_export(grouped: bool) {
    let files = Files::new();
    let original = files.wav(24_000, 8192);
    let original_bytes = fs::read(&original).unwrap();
    let (events, audio_events) = mpsc::sync_channel(8);
    let mut sink = NativeAudioSink::new(events);
    sink.output_factory = Some(Box::new(|_| Err("fixture output unavailable".into())));
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        0,
        None,
        Box::new(sink),
        AudioPolicy::new().unwrap().resident_native,
    )
    .unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "canonical export fixture stalled"
            );
            std::thread::yield_now();
        }
    };
    assert!(next().error.is_none());
    assert!(
        matches!(
            audio_events.recv_timeout(Duration::from_secs(1)).unwrap(),
            EditorAudioEvent::Unavailable { error, .. }
                if error.to_string().contains("fixture output unavailable")
        ),
        "injected output failure stays visible asynchronously"
    );
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        })
        .unwrap();
    assert!(next().error.is_none());
    service.submit(Request::Import(original.clone())).unwrap();
    let mut imported = next();
    assert!(imported.error.is_none(), "{:?}", imported.error);
    if grouped {
        let track = imported.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(hex_project::Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            }))
            .unwrap();
        imported = next();
        assert!(imported.error.is_none(), "{:?}", imported.error);
    }
    assert!(imported.playback.is_none(), "no resident fallback");
    let snapshot = imported.state.canonical_audio_source.as_ref().unwrap();
    assert!(
        snapshot
            .required_sources()
            .all(|(_, binding)| matches!(binding, AudioSourceBinding::VerifiedFile(_)))
    );
    let project = imported.state.project.clone();
    let revision = imported.state.revision;
    let path = files.0.join("verified-export.wav");
    service.submit(Request::Export(path.clone())).unwrap();
    let queued = next();
    assert!(queued.error.is_none(), "{:?}", queued.error);
    assert_eq!(queued.state.revision, revision);
    assert_eq!(queued.state.project, project);
    assert_eq!(queued.state.unsaved, imported.state.unsaved);
    let deadline = Instant::now() + Duration::from_secs(10);
    let completion = loop {
        match service.try_export_event() {
            Ok(event) => break event,
            Err(mpsc::TryRecvError::Empty) => {
                assert!(
                    Instant::now() < deadline,
                    "verified export completion stalled"
                );
                std::thread::yield_now();
            }
            Err(error) => panic!("export channel: {error}"),
        }
    };
    let report = completion
        .result
        .expect("verified export must prepare actual pages");
    assert_eq!(completion.revision, revision);
    assert_eq!(report.frames, 16_384);
    let rendered = fs::read(path).unwrap();
    assert_eq!(rendered.len(), 44 + 16_384 * 4);
    assert!(
        rendered[44..]
            .chunks_exact(2)
            .any(|sample| sample != [0, 0])
    );
    assert_eq!(fs::read(original).unwrap(), original_bytes);
}

// Rate-only host fixture: the actual canonical Service and production offline
// page worker run; there is no device/output/native publication substitute.
struct OfflineSink {
    rate: u32,
    render: hex_session::service::StreamedExporter,
}
impl WorkerAudioSink for OfflineSink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(self.rate)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn poll(&mut self) {}
    fn close(&mut self) {}
    fn streamed_exporter(&self) -> Option<hex_session::service::StreamedExporter> {
        Some(self.render)
    }
}
fn offline(rate: u32, render: hex_session::service::StreamedExporter) -> Service {
    let service =
        Service::with_audio_sink(16, 0, None, Box::new(OfflineSink { rate, render })).unwrap();
    assert!(event(&service).error.is_none());
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        },
    );
    service
}
fn event(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(Instant::now() < deadline, "source worker stalled");
        std::thread::yield_now();
    }
}
fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn exported(service: &Service) -> hex_session::service::ExportEvent {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match service.try_export_event() {
            Ok(event) => return event,
            Err(mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < deadline, "offline export stalled");
                std::thread::yield_now();
            }
            Err(error) => panic!("export channel: {error}"),
        }
    }
}
fn varied(files: &Files, name: &str, rate: u32, frames: u32) -> PathBuf {
    let base = files.wav(rate, frames);
    let mut bytes = fs::read(base).unwrap();
    for (i, frame) in bytes[44..].chunks_exact_mut(2).enumerate() {
        let value = ((i as i32 * 73) % 40_000 - 20_000) as i16;
        frame.copy_from_slice(&value.to_le_bytes());
    }
    let path = files.0.join(name);
    fs::write(&path, bytes).unwrap();
    path
}
fn tiny(
    request: &hex_session::service::StreamedExportRequest<'_>,
) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
    crate::streamed_export::render_with_config(request, StreamConfig::with_pool_pages(3))
        .map_err(Into::into)
}

#[test]
fn verified_export_turns_over_a_two_page_cache_and_refuses_overlapping_demand_without_output() {
    let files = Files::new();
    let first = varied(&files, "first.wav", 48000, 12288);
    let second = varied(&files, "second.wav", 48000, 12288);
    let streamed = offline(48000, tiny);
    run(&streamed, Request::Import(first.clone()));
    let output = files.0.join("turnover.wav");
    run(&streamed, Request::Export(output.clone()));
    let completed = exported(&streamed);
    assert!(completed.result.is_ok(), "{:?}", completed.result);
    let resident = Service::new(48000, 16, 100000).unwrap();
    event(&resident);
    run(&resident, Request::Import(first));
    let oracle = files.0.join("resident.wav");
    run(&resident, Request::Export(oracle.clone()));
    assert!(exported(&resident).result.is_ok());
    assert_eq!(
        fs::read(output).unwrap(),
        fs::read(oracle).unwrap(),
        "actual worker cache turnover keeps every sample"
    );
    run(&streamed, Request::Import(second));
    let third = varied(&files, "third.wav", 48000, 12288);
    let imported = run(&streamed, Request::Import(third));
    let refused = files.0.join("over-capacity.wav");
    run(&streamed, Request::Export(refused.clone()));
    let error = exported(&streamed).result.unwrap_err();
    assert!(
        error.message().contains("simultaneous cache capacity"),
        "{error}"
    );
    assert!(!refused.exists());
    let after = run(
        &streamed,
        Request::Edit(hex_project::Command::MasterGain { gain: 1.0 }),
    );
    assert_eq!(after.state.project, imported.state.project);
}

#[test]
fn verified_export_refuses_missing_changed_and_short_sources_preserving_existing_output_and_retry()
{
    use hex_session::service::{ExportControl, ExportSignal};
    for (failure, grouped) in [
        (0, false),
        (1, false),
        (2, false),
        (0, true),
        (1, true),
        (2, true),
    ] {
        let files = Files::new();
        let source = varied(&files, "source.wav", 48000, 12288);
        let original = fs::read(&source).unwrap();
        let service = offline(48000, crate::streamed_export::render);
        let mut imported = run(&service, Request::Import(source.clone()));
        if grouped {
            let track = imported.state.project.tracks()[0].id();
            imported = run(
                &service,
                Request::Edit(hex_project::Command::GroupTracks {
                    tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                    name: "group".into(),
                }),
            );
        }
        match failure {
            0 => fs::remove_file(&source).unwrap(),
            1 => {
                let mut changed = original.clone();
                changed[48] ^= 1;
                fs::write(&source, changed).unwrap();
            }
            _ => fs::write(&source, &original[..50]).unwrap(),
        }
        let output = files.0.join("existing.wav");
        fs::write(&output, b"previous successful export").unwrap();
        let queued = run(
            &service,
            Request::ExportRange {
                path: output.clone(),
                start: 0,
                frames: None,
                signal: ExportSignal::Main,
                control: ExportControl::default(),
                format: hex_export::ExportFormat::Pcm16,
                overwrite: true,
            },
        );
        assert_eq!(queued.state.project, imported.state.project);
        let completed = exported(&service);
        assert!(
            completed.result.is_err(),
            "failure {failure} must refuse publication"
        );
        assert_eq!(fs::read(&output).unwrap(), b"previous successful export");
        fs::write(&source, &original).unwrap();
        let retry = files.0.join("retry.wav");
        run(&service, Request::Export(retry.clone()));
        assert!(
            exported(&service).result.is_ok(),
            "failed source job releases export lane"
        );
        assert!(retry.exists());
        assert_eq!(fs::read(&source).unwrap(), original);
    }
}

#[test]
fn verified_export_cancellation_preserves_destination_and_allows_retry() {
    use hex_session::service::{ExportControl, ExportSignal};
    let files = Files::new();
    let source = varied(&files, "source.wav", 48000, 12288);
    let original = fs::read(&source).unwrap();
    let service = offline(48000, crate::streamed_export::render);
    run(&service, Request::Import(source.clone()));
    let output = files.0.join("existing.wav");
    fs::write(&output, b"previous export").unwrap();
    let control = ExportControl::default();
    assert!(control.cancel());
    run(
        &service,
        Request::ExportRange {
            path: output.clone(),
            start: 0,
            frames: None,
            signal: ExportSignal::Main,
            control,
            format: hex_export::ExportFormat::Pcm16,
            overwrite: true,
        },
    );
    assert!(exported(&service).result.is_err());
    assert_eq!(fs::read(output).unwrap(), b"previous export");
    let retry = files.0.join("retry.wav");
    run(&service, Request::Export(retry));
    assert!(exported(&service).result.is_ok());
    assert_eq!(fs::read(source).unwrap(), original);
}

#[test]
fn verified_export_never_overwrites_its_original_source_even_with_export_overwrite_consent() {
    use hex_session::service::{ExportControl, ExportSignal};
    let files = Files::new();
    let source = varied(&files, "source.wav", 48000, 12288);
    let original = fs::read(&source).unwrap();
    let service = offline(48000, crate::streamed_export::render);
    run(&service, Request::Import(source.clone()));
    let alias = files.0.join("alias.wav");
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    for destination in [source.clone(), alias] {
        run(
            &service,
            Request::ExportRange {
                path: destination,
                start: 0,
                frames: None,
                signal: ExportSignal::Main,
                control: ExportControl::default(),
                format: hex_export::ExportFormat::Pcm16,
                overwrite: true,
            },
        );
        let error = exported(&service).result.unwrap_err();
        assert!(
            error.message().contains("original media")
                || error.message().contains("existing regular file"),
            "{error}"
        );
        assert_eq!(fs::read(&source).unwrap(), original);
    }
}

fn replace_at_finish(
    request: &hex_session::service::StreamedExportRequest<'_>,
) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
    crate::streamed_export::test_support::finish_action(request, || {
        let AudioSourceBinding::VerifiedFile(file) =
            request.source().required_sources().next().unwrap().1
        else {
            panic!("verified file");
        };
        let mut changed = fs::read(file.path()).unwrap();
        changed[48] ^= 1;
        let replacement = file.path().with_extension("replacement");
        fs::write(&replacement, changed).unwrap();
        fs::rename(replacement, file.path()).unwrap();
    })
}
fn cancel_at_finish(
    request: &hex_session::service::StreamedExportRequest<'_>,
) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
    crate::streamed_export::test_support::finish_action(request, || {
        assert!(
            request.control().cancel(),
            "publication has not been sealed"
        );
    })
}

#[test]
fn verified_export_rechecks_cached_sources_and_cancellation_before_publication() {
    use hex_session::service::{ExportControl, ExportSignal};
    for render in [
        replace_at_finish as hex_session::service::StreamedExporter,
        cancel_at_finish,
    ] {
        let files = Files::new();
        let source = varied(&files, "source.wav", 48000, 12288);
        let service = offline(48000, render);
        run(&service, Request::Import(source));
        let output = files.0.join("existing.wav");
        fs::write(&output, b"previous export").unwrap();
        let control = ExportControl::default();
        run(
            &service,
            Request::ExportRange {
                path: output.clone(),
                start: 0,
                frames: None,
                signal: ExportSignal::Main,
                control: control.clone(),
                format: hex_export::ExportFormat::Pcm16,
                overwrite: true,
            },
        );
        let error = exported(&service).result.unwrap_err();
        assert!(
            error.message().contains("changed") || error.message().contains("cancel"),
            "{error}"
        );
        assert_eq!(
            control.progress().processed_frames,
            control.progress().total_frames,
            "injection occurs after every audio frame was rendered"
        );
        assert_eq!(fs::read(output).unwrap(), b"previous export");
    }
}

type ExportGate = (SyncSender<u64>, mpsc::Receiver<()>);
static EXPORT_GATE: Mutex<Option<ExportGate>> = Mutex::new(None);
fn gated_export(
    request: &hex_session::service::StreamedExportRequest<'_>,
) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
    let (entered, release) = EXPORT_GATE.lock().unwrap().take().unwrap();
    entered
        .send(request.source().context().editor_revision)
        .unwrap();
    release.recv_timeout(Duration::from_secs(5)).unwrap();
    crate::streamed_export::render(request)
}

#[test]
fn verified_export_keeps_captured_revision_through_later_edit_and_new_document() {
    let files = Files::new();
    let source = varied(&files, "source.wav", 48000, 12288);
    let service = offline(48000, gated_export);
    let imported = run(&service, Request::Import(source.clone()));
    let (entered, entered_rx) = mpsc::sync_channel(1);
    let (release, release_rx) = mpsc::sync_channel(1);
    *EXPORT_GATE.lock().unwrap() = Some((entered, release_rx));
    let output = files.0.join("captured.wav");
    run(&service, Request::Export(output.clone()));
    assert_eq!(
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        imported.state.revision
    );
    run(
        &service,
        Request::Edit(hex_project::Command::MasterGain { gain: 0.5 }),
    );
    assert!(
        matches!(
            service.submit(Request::Export(files.0.join("busy.wav"))),
            Err(hex_session::service::SubmitError::Busy(
                Request::Export(_),
                hex_session::service::BusyReason::ExportPending
            ))
        ),
        "a blocked offline job still owns the single export lane"
    );
    assert!(!files.0.join("busy.wav").exists());
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        },
    );
    release.send(()).unwrap();
    let result = exported(&service);
    assert_eq!(result.revision, imported.state.revision);
    assert!(result.result.is_ok(), "{:?}", result.result);
    let resident = Service::new(48000, 16, 100000).unwrap();
    event(&resident);
    run(&resident, Request::Import(source));
    let oracle = files.0.join("original.wav");
    run(&resident, Request::Export(oracle.clone()));
    assert!(exported(&resident).result.is_ok());
    assert_eq!(fs::read(output).unwrap(), fs::read(oracle).unwrap());
}

fn cancel_after_first_chunk(
    request: &hex_session::service::StreamedExportRequest<'_>,
) -> Result<hex_export::ExportReport, hex_session::service::ExportFailure> {
    crate::streamed_export::test_support::chunk_action(request, || {
        assert!(request.control().cancel());
    })
}

#[test]
fn verified_export_cancellation_during_page_backed_render_discards_staging() {
    use hex_session::service::{ExportControl, ExportSignal};
    let files = Files::new();
    let source = varied(&files, "source.wav", 48000, 12288);
    let service = offline(48000, cancel_after_first_chunk);
    run(&service, Request::Import(source));
    let output = files.0.join("cancelled.wav");
    let control = ExportControl::default();
    run(
        &service,
        Request::ExportRange {
            path: output.clone(),
            start: 0,
            frames: None,
            signal: ExportSignal::Main,
            control: control.clone(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        },
    );
    assert!(
        exported(&service)
            .result
            .unwrap_err()
            .message()
            .contains("cancel")
    );
    let progress = control.progress();
    assert!(progress.processed_frames > 0);
    assert!(progress.processed_frames < progress.total_frames);
    assert!(!output.exists());
}

#[test]
fn verified_export_unexpected_render_page_miss_cannot_publish_engine_fallback_silence() {
    let files = Files::new();
    let source = varied(&files, "source.wav", 48000, 12288);
    let service = offline(48000, crate::streamed_export::test_support::missing_page);
    run(&service, Request::Import(source));
    let output = files.0.join("must-not-be-silence.wav");
    run(&service, Request::Export(output.clone()));
    let error = exported(&service).result.unwrap_err();
    assert!(
        error.message().contains("unexpected media page miss"),
        "{error}"
    );
    assert!(!output.exists());
}

#[test]
fn verified_export_writes_the_requested_format_through_streamed_pages() {
    use hex_session::service::{ExportControl, ExportFormat, ExportSignal};
    let files = Files::new();
    let source = varied(&files, "source.wav", 48000, 4096);
    let service = offline(48000, crate::streamed_export::render);
    run(&service, Request::Import(source));
    let export = |name: &str, format: ExportFormat| {
        let path = files.0.join(name);
        run(
            &service,
            Request::ExportRange {
                path: path.clone(),
                start: 0,
                frames: Some(2048),
                signal: ExportSignal::Main,
                control: ExportControl::default(),
                format,
                overwrite: false,
            },
        );
        let report = exported(&service).result.unwrap();
        assert_eq!(report.data_bytes, 2048 * format.frame_bytes());
        fs::read(path).unwrap()
    };
    let float = export("float.wav", ExportFormat::Float32);
    let pcm16 = export("pcm16.wav", ExportFormat::Pcm16);
    assert_eq!(u16::from_le_bytes([float[20], float[21]]), 3);
    assert_eq!(float.len(), 58 + 2048 * 8);
    for (float, pcm) in float[58..].chunks_exact(4).zip(pcm16[44..].chunks_exact(2)) {
        let float = f32::from_le_bytes([float[0], float[1], float[2], float[3]]);
        let pcm = i16::from_le_bytes([pcm[0], pcm[1]]);
        assert!(
            (f64::from(float) * 32_767.0 - f64::from(pcm)).abs() <= 1.5,
            "{float} vs {pcm}"
        );
    }
}
