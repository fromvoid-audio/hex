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
