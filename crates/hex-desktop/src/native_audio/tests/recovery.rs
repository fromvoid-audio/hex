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
        let error = error.to_string();
        assert_eq!(error, error.to_lowercase());
        assert!(error.contains("save your project and restart hex"));
        assert!(!sink.open_pending);
        assert!(sink.opening.is_none());
        sink.poll();
        assert!(receiver.try_recv().is_err());
        assert_eq!(attempts.load(Acquire), 1); // no retry or new output
        // #4132: a later manual restart cannot clear a latched failure; it is
        // refused with the same recovery path and never opens an output.
        sink.request_open(Some(48000), Some(18));
        sink.poll();
        let EditorAudioEvent::Unavailable {
            restart_request,
            error,
            ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected the latched failure again");
        };
        assert_eq!(restart_request, Some(18));
        assert!(
            error
                .to_string()
                .contains("save your project and restart hex")
        );
        assert_eq!(attempts.load(Acquire), 1);
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
    assert!(
        error
            .to_string()
            .starts_with("audio resource cleanup failed")
    );
    assert!(Arc::ptr_eq(sink.desired.as_ref().unwrap(), &current));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn successful_open_without_confirmed_failure_keeps_output_and_emits_opened() {
    let _native = native_fixture();
    let service = Service::new(48_000, 16, MAX_RESIDENT_FRAMES).unwrap();
    let source = next_source(&service);
    let (events, receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    let deadline = Instant::now() + Duration::from_secs(3);
    let (output, backend) = loop {
        match hex_audio::worker_fixture::open_with_reliable_action_outcomes(48_000) {
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
    // Observation must not release the actual exclusive lease: a second
    // constructor still refuses while this output and callback remain retained.
    assert!(matches!(
        hex_audio::worker_fixture::open_with_reliable_action_outcomes(48_000),
        Err(error) if error == "audio output is open or still shutting down"
    ));
    assert!(!sink.open_pending);
    drop(audio);
    drop(backend);
    sink.close();
}

#[test]
fn restart_on_a_device_at_another_rate_reports_a_typed_recoverable_mismatch() {
    let service = Service::new(48_000, 16, 0).unwrap();
    let source = next_source(&service);
    let (events, receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    let deadline = Instant::now() + Duration::from_secs(3);
    // The fake factory's device negotiates 44.1 kHz for a 48 kHz project.
    let (output, backend) = loop {
        match hex_audio::worker_fixture::open_with_reliable_action_outcomes(44_100) {
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
    let generation = source.context().project_generation;
    WorkerAudioSink::restart_audio(&mut sink, generation, 9).unwrap();
    sink.poll_open();
    assert!(!sink.open_pending);
    assert!(
        sink.output.is_none(),
        "a mismatched output is never exposed"
    );
    sink.deliver_open();
    sink.flush_events();
    let EditorAudioEvent::Unavailable {
        project_generation,
        restart_request,
        error,
    } = receiver.try_recv().unwrap()
    else {
        panic!("expected the typed rate mismatch");
    };
    assert_eq!(project_generation, generation);
    assert_eq!(restart_request, Some(9));
    assert_eq!(
        error,
        hex_audio::OutputOpenError::RateMismatch(hex_audio::RateMismatch {
            device_hz: 44_100,
            project_hz: 48_000,
        })
    );
    drop(backend);
    sink.close();
}
