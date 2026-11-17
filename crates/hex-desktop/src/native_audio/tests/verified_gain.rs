//! Shared fixture for Verified-streaming native worker, publication and callback tests.
use super::*;
mod effects;
mod group;

pub(in crate::native_audio) fn actual_native_harness() -> Harness {
    let backend = Arc::new(Mutex::new(None));
    let factory_backend = Arc::clone(&backend);
    let (events, receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    assert!(sink.resident_publication_enabled());
    sink.output_factory = Some(Box::new(move |rate| {
        let mut slot = factory_backend.lock().unwrap();
        if slot.is_some() {
            return Ok(None);
        }
        match hex_audio::worker_fixture::open_with_reliable_action_outcomes(rate.unwrap_or(48_000))
        {
            Ok((output, callback)) => {
                *slot = Some(callback);
                Ok(Some(output))
            }
            Err(error) if error == "audio output is open or still shutting down" => Ok(None),
            Err(error) => Err(error.into()),
        }
    }));
    let observation = Arc::new(Mutex::new(Observation::default()));
    let service = Service::with_audio_sink(
        16,
        0,
        None,
        Box::new(ObservedSink::rendered(sink, Arc::clone(&observation))),
    )
    .unwrap();
    Harness {
        rendered_setup: None,
        service: Some(service),
        events: receiver,
        backend,
        audio: None,
        context: None,
        observation,
        wait: Duration::from_secs(10),
    }
}
