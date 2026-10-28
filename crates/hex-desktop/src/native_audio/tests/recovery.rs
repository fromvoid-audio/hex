use super::*;
use hex_audio::AudioLifecycleFailure;

fn next_source(service: &Service) -> Arc<CanonicalAudioSourceSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event.state.canonical_audio_source.clone().unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn pending_open_and_confirmed_lifecycle_failures_have_distinct_recovery_results() {
    let service = Service::new(48000, 16, 0).unwrap();
    let source = next_source(&service);
    for status in [
        (|| Some(AudioLifecycleFailure::Retirement)) as fn() -> Option<AudioLifecycleFailure>,
        || Some(AudioLifecycleFailure::Cleanup),
        || Some(AudioLifecycleFailure::MismatchedOwner),
    ] {
        let (events, receiver) = mpsc::sync_channel(1);
        let mut sink = NativeAudioSink::new(events);
        sink.canonical_changed(source.clone());
        sink.lifecycle_status = Some(|| None);
        let attempts = Arc::new(AtomicU64::new(0));
        let calls = attempts.clone();
        sink.output_factory = Some(Box::new(move |_| {
            calls.fetch_add(1, Acquire);
            Ok(None)
        }));
        sink.request_open(Some(48000), Some(17));
        sink.poll();
        assert!(sink.open_pending);
        assert!(sink.open_error.is_none());
        assert!(receiver.try_recv().is_err());
        assert_eq!(attempts.load(Acquire), 1);
        sink.lifecycle_status = Some(status);
        sink.poll();
        let EditorAudioEvent::Unavailable {
            project_generation,
            restart_request,
            error,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected confirmed recovery failure");
        };
        assert_eq!(project_generation, source.context().project_generation);
        assert_eq!(restart_request, Some(17));
        assert_eq!(error, error.to_lowercase());
        assert!(error.contains("save your project and restart hex"));
        assert!(!sink.open_pending);
        assert!(sink.opening.is_none());
        sink.poll();
        assert!(receiver.try_recv().is_err());
        assert_eq!(attempts.load(Acquire), 1); // no retry or new output
        assert!(Arc::ptr_eq(sink.desired.as_ref().unwrap(), &source));
    }
}

#[test]
fn replacement_discards_pending_old_failure_and_correlates_new_result_under_backpressure() {
    let service = Service::new(48000, 16, 0).unwrap();
    let old = next_source(&service);
    let (events, receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events.clone());
    sink.lifecycle_status = Some(|| Some(AudioLifecycleFailure::Cleanup));
    sink.output_factory = Some(Box::new(|_| panic!("retained failure must not reopen")));
    sink.canonical_changed(old.clone());
    sink.request_open(Some(48000), Some(1));
    // Fill the result lane with a stale event already owned by the UI queue.
    assert!(
        events
            .send(EditorAudioEvent::Unavailable {
                project_generation: old.context().project_generation,
                restart_request: Some(0),
                error: "older recovery result".into(),
            })
            .is_ok()
    );
    sink.poll();
    assert!(matches!(
        sink.pending_open_event,
        Some(EditorAudioEvent::Unavailable {
            restart_request: Some(1),
            ..
        })
    ));
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    let current = next_source(&service);
    assert_ne!(
        old.context().project_generation,
        current.context().project_generation
    );
    sink.canonical_changed(current.clone());
    assert!(sink.pending_open_event.is_none()); // stale undelivered result retired
    sink.poll();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        EditorAudioEvent::Unavailable {
            restart_request: Some(0),
            ..
        }
    ));
    sink.flush_events();
    let EditorAudioEvent::Unavailable {
        project_generation,
        restart_request,
        error,
    } = receiver.try_recv().unwrap()
    else {
        panic!("expected current recovery result");
    };
    assert_eq!(project_generation, current.context().project_generation);
    assert_eq!(restart_request, None);
    assert!(error.starts_with("audio resource cleanup failed"));
    assert!(Arc::ptr_eq(sink.desired.as_ref().unwrap(), &current));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn successful_open_without_confirmed_failure_keeps_output_and_emits_opened() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let service = Service::new(48_000, 16, MAX_RESIDENT_FRAMES).unwrap();
    let source = next_source(&service);
    let (events, receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    let deadline = Instant::now() + Duration::from_secs(3);
    let (output, backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(opened) => break opened,
            Err(error) if error == "audio output is open or still shutting down" => {
                assert!(
                    Instant::now() < deadline,
                    "prior fixture output did not retire"
                );
                std::thread::yield_now();
            }
            Err(error) => panic!("{error}"),
        }
    };
    let mut output = Some(output);
    sink.output_factory = Some(Box::new(move |_| Ok(output.take())));
    sink.lifecycle_status = Some(|| None);
    sink.canonical_changed(source.clone());
    sink.request_open(Some(48_000), Some(7));
    sink.poll_open();
    assert!(!sink.open_pending && sink.open_error.is_none());
    sink.deliver_open();
    sink.flush_events();
    let EditorAudioEvent::Opened {
        project_generation,
        restart_request,
        audio,
    } = receiver.try_recv().unwrap()
    else {
        panic!("successful completion must produce opened");
    };
    assert_eq!(project_generation, source.context().project_generation);
    assert_eq!(restart_request, Some(7));
    sink.request_open(Some(48_000), Some(8));
    sink.lifecycle_status = Some(|| Some(AudioLifecycleFailure::Retirement));
    sink.poll_open();
    assert!(sink.output.is_some()); // observation cannot release its retained output
    assert!(!sink.open_pending);
    drop(audio);
    drop(backend);
    sink.close();
}
