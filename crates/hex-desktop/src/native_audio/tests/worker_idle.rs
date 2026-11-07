//! The sink's worker idleness contract (#4574): only a fully quiescent,
//! delivered and stopped output lets the Session worker lengthen its wait.
use super::*;
use std::sync::atomic::Ordering::Release;

#[test]
fn only_a_delivered_stopped_quiescent_output_reports_worker_idle() {
    let _fixture = native_fixture();
    let service = Service::new(48000, 16, 0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let source = loop {
        if let Ok(event) = service.try_event() {
            break event.state.canonical_audio_source.clone().unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert_ne!(source.audio_mode(), DocumentAudioMode::VerifiedStreaming);
    let (events, receiver) = mpsc::sync_channel(4);
    let mut sink = NativeAudioSink::new(events);
    let backend = Arc::new(Mutex::new(None));
    let factory_backend = Arc::clone(&backend);
    // Same retry contract as `Harness`: another fixture may still hold the
    // process-wide output lease, so opening can take several polls.
    sink.output_factory = Some(Box::new(move |rate| {
        match hex_audio::worker_fixture::open_with_reliable_action_outcomes(rate.unwrap_or(48000)) {
            Ok((output, callback)) => {
                *factory_backend.lock().unwrap() = Some(callback);
                Ok(Some(output))
            }
            Err(error) if error == "audio output is open or still shutting down" => Ok(None),
            Err(error) => Err(error.into()),
        }
    }));
    assert!(sink.worker_idle(), "nothing to drive without an output");
    sink.canonical_changed(source);
    sink.request_open(Some(48000), None);
    let deadline = Instant::now() + Duration::from_secs(10);
    // Busy for every poll of the open, until the output is delivered.
    while sink.output_generation.is_none() {
        assert!(!sink.worker_idle(), "a pending open keeps the busy cadence");
        assert!(Instant::now() < deadline, "fixture output did not open");
        sink.poll();
        if sink.output_generation.is_none() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert!(sink.output.is_some(), "{:?}", sink.open_error);
    let opened = receiver.try_recv().unwrap();
    assert!(matches!(opened, EditorAudioEvent::Opened { .. }));
    assert!(sink.worker_idle(), "delivered and stopped");

    // The callback's transport report, as a playing render would publish it.
    let set_playing = |sink: &mut NativeAudioSink, playing: bool| {
        sink.output
            .as_mut()
            .and_then(|output| output.publication.control())
            .unwrap()
            .status
            .playing
            .store(playing, Release)
    };
    set_playing(&mut sink, true);
    assert!(!sink.worker_idle(), "playback keeps the busy cadence");
    set_playing(&mut sink, false);
    assert!(sink.worker_idle());

    let output_identity = sink.output_identity.unwrap();
    sink.pending_route_event = Some(EditorAudioEvent::OutputRouteChanged { output_identity });
    assert!(!sink.worker_idle(), "a queued event keeps the busy cadence");
    sink.poll();
    // Earlier observations (the initial degraded-plugin set) precede it.
    assert!(
        receiver
            .try_iter()
            .any(|event| matches!(event, EditorAudioEvent::OutputRouteChanged { .. }))
    );
    assert!(sink.worker_idle(), "flushed events restore idleness");
    drop(opened);
    sink.close();
    drop(backend);
}
