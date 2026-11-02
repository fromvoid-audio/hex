//! Startup failure is observable after the real worker's event channel closes.
use super::*;

struct Sink {
    rate: Result<u32, &'static str>,
    closed: Arc<AtomicBool>,
}
impl WorkerAudioSink for Sink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        self.rate.map_err(str::to_owned)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn poll(&mut self) {}
    fn close(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

#[test]
fn construction_failure_is_published_before_disconnection() {
    for (rate, history, expected) in [
        (Ok(48_000), 0, "history capacity"),
        (Err("device unavailable"), 0, "history capacity"),
    ] {
        let closed = Arc::new(AtomicBool::new(false));
        let mut service = Service::with_audio_sink(
            history,
            0,
            None,
            Box::new(Sink {
                rate,
                closed: Arc::clone(&closed),
            }),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
        loop {
            match service.try_event() {
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                _ => panic!("worker unexpectedly published state or did not terminate"),
            }
        }
        assert!(
            service
                .startup_failure()
                .is_some_and(|error| error.contains(expected))
        );
        service.shutdown(std::time::Duration::from_secs(1)).unwrap();
        assert!(closed.load(Ordering::Acquire));
    }
}

#[test]
fn successful_startup_does_not_report_failure() {
    let service = Service::with_audio_sink(
        8,
        0,
        None,
        Box::new(Sink {
            rate: Ok(48_000),
            closed: Arc::new(AtomicBool::new(false)),
        }),
    )
    .unwrap();
    let event = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(event.error.is_none());
    assert!(service.startup_failure().is_none());
}
