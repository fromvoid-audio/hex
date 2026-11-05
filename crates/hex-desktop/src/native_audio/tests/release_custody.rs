//! Output release survives event-channel pressure and successive replacements.
use super::*;

fn queued_release_after_replacements(already_relinquished: bool) {
    let service = hex_session::service::Service::new(48_000, 8, 1024).unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "session event timed out");
            std::thread::yield_now();
        }
    };
    let initial = next();
    let first = Arc::clone(initial.state.canonical_audio_source.as_ref().unwrap());
    let mut snapshots = Vec::new();
    for _ in 0..2 {
        service
            .submit(Request::NewWithAudioMode {
                mode: DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap();
        let event = next();
        assert!(event.error.is_none(), "{:?}", event.error);
        snapshots.push(Arc::clone(
            event.state.canonical_audio_source.as_ref().unwrap(),
        ));
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    sender
        .send(EditorAudioEvent::OutputRebound {
            project_generation: 0,
            output_identity: 999,
        })
        .unwrap();
    let mut sink = NativeAudioSink::new(sender);
    let opened = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&opened);
    sink.output_factory = Some(Box::new(move |_| {
        observed.store(true, std::sync::atomic::Ordering::Release);
        Ok(None)
    }));
    sink.canonical_changed(first);
    sink.output_identity = Some(123);
    if already_relinquished {
        // Sampler preflight may retire its output before the replacement arrives.
        sink.relinquish_output();
    }
    sink.canonical_changed(Arc::clone(&snapshots[0]));
    sink.flush_events();
    sink.canonical_changed(Arc::clone(&snapshots[1]));
    sink.poll_open();
    assert!(!opened.load(std::sync::atomic::Ordering::Acquire));
    assert!(sink.output.is_none());
    assert!(sink.open_pending);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        EditorAudioEvent::OutputRebound { .. }
    ));
    sink.flush_events();
    assert!(
        matches!(receiver.try_recv().unwrap(), EditorAudioEvent::OutputReleased { project_generation, old_output_identity: 123 } if project_generation == snapshots[0].context().project_generation)
    );
    sink.poll_open();
    assert!(opened.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn queued_release_survives_two_replacements_and_blocks_reopen_until_delivered() {
    queued_release_after_replacements(false);
}

#[test]
fn replacement_releases_an_endpoint_already_relinquished_by_sampler_preflight() {
    queued_release_after_replacements(true);
}
