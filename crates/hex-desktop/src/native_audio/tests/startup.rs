//! Initial canonical editing does not depend on the native open result.
use super::*;

#[test]
fn delayed_initial_success_preserves_edits_and_project_rate() {
    delayed_completion(None, 48_000);
}

#[test]
fn delayed_initial_rate_mismatch_preserves_edits_and_reports_refusal() {
    delayed_completion(None, 44_100);
}

#[test]
fn delayed_initial_bridge_at_device_rate_authors_at_that_rate() {
    delayed_completion(Some(|| Some(44_100)), 44_100);
}

fn delayed_completion(bridge_probe: Option<fn() -> Option<u32>>, device_rate: u32) {
    let project_rate = bridge_probe.and_then(|probe| probe()).unwrap_or(48_000);
    let _native = native_fixture();
    let (events, audio_events) = mpsc::sync_channel(4);
    let mut sink = NativeAudioSink::new(events);
    sink.lifecycle_status = Some(|| None);
    sink.startup_bridge_rate_probe = bridge_probe;
    let release = Arc::new(AtomicBool::new(false));
    let gate = Arc::clone(&release);
    let backend = Arc::new(Mutex::new(None));
    let callback = Arc::clone(&backend);
    sink.output_factory = Some(Box::new(move |requested| {
        assert_eq!(requested, Some(project_rate));
        if !gate.load(Acquire) {
            return Ok(None);
        }
        let (output, renderer) =
            match hex_audio::worker_fixture::open_with_reliable_action_outcomes(device_rate) {
                Ok(pair) => pair,
                // The preceding fixture's asynchronous retirement still owns the
                // process-wide lease until actual teardown; use the real retry path.
                Err(error) if error == "audio output is open or still shutting down" => {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
        *callback.lock().unwrap() = Some(renderer);
        Ok(Some(output))
    }));
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        0,
        None,
        Box::new(sink),
        AudioPolicy::new().unwrap().resident_native,
    )
    .unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                assert_eq!(event.state.project.sample_rate(), project_rate);
                return event;
            }
            assert!(Instant::now() < deadline, "editing waited for audio open");
            std::thread::yield_now();
        }
    };
    let initial = next();
    service
        .submit(Request::Edit(hex_project::Command::AddTrack {
            name: "while opening".into(),
        }))
        .unwrap();
    let edited = next();
    assert_eq!(edited.state.revision, initial.state.revision + 1);
    assert_eq!(edited.state.project.tracks().len(), 1);
    assert!(audio_events.try_recv().is_err());
    // Both state and a completed canonical edit precede the native result.
    release.store(true, std::sync::atomic::Ordering::Release);
    let result = audio_events.recv_timeout(Duration::from_secs(1)).unwrap();
    let audio = match result {
        EditorAudioEvent::Opened { audio, .. } if device_rate == project_rate => {
            assert_eq!(audio.sample_rate, project_rate);
            Some(audio)
        }
        EditorAudioEvent::Unavailable { error, .. } if device_rate != project_rate => {
            let mismatch = error.rate_mismatch().unwrap();
            assert_eq!(mismatch.project_hz, project_rate);
            assert_eq!(mismatch.device_hz, device_rate);
            None
        }
        EditorAudioEvent::Unavailable { error, .. } => panic!("late open failed: {error}"),
        _ => panic!("unexpected late output result"),
    };
    let files = Files::new();
    let saved = files.0.join("edited-during-open.hex");
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    let after = next();
    assert_eq!(after.state.revision, edited.state.revision);
    assert_eq!(after.state.project, edited.state.project);
    assert!(saved.is_file());
    drop(service);
    drop(audio);
    drop(backend.lock().unwrap().take());
}
