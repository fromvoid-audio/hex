//! Zero-allocation callback-boundary probe for `hex_midi::OutputProducer`.
//! Construction, connection, and worker shutdown remain outside measured scopes.

use hex_midi::{
    EncodedMidiMessage, OUTPUT_NORMAL_CAPACITY, OutputBackend, OutputCommand,
    OutputConnectionState, OutputEnqueueResult, OutputError, OutputEvent, OutputMessage,
    OutputPort, OutputSendError, OutputWorker, TimestampedOutputMessage,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::TryRecvError,
};
use std::time::{Duration, Instant};

struct ProbeBackend {
    paused: Arc<AtomicBool>,
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl OutputBackend for ProbeBackend {
    type Endpoint = u32;

    fn discover(&mut self) -> Result<Vec<OutputPort>, String> {
        Ok(vec![OutputPort {
            id: 1,
            name: "probe".into(),
        }])
    }

    fn connect(&mut self, port: &OutputPort) -> Result<Self::Endpoint, String> {
        if port.id == 1 {
            Ok(1)
        } else {
            Err("missing".into())
        }
    }

    fn is_present(&mut self, endpoint: &Self::Endpoint) -> bool {
        *endpoint == 1
    }

    fn send(
        &mut self,
        _endpoint: &mut Self::Endpoint,
        _timestamp_ns: u64,
        message: EncodedMidiMessage,
    ) -> Result<(), OutputSendError> {
        while self.paused.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.sent.lock().unwrap().push(message.as_slice().to_vec());
        Ok(())
    }

    fn close(&mut self, _endpoint: Self::Endpoint) {}
}

#[test]
fn midi_output_producer_handoff_has_zero_total_and_net_allocations() {
    // Sensitivity calibration: confirm the global allocation counter observes heap work.
    let cal = allocation_counter::measure(|| {
        let mut v: Vec<u8> = Vec::with_capacity(32);
        v.push(1);
        std::hint::black_box(&v);
    });
    assert!(cal.count_total > 0 && cal.bytes_total >= 32);

    let paused = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut worker = OutputWorker::start_with_backend(ProbeBackend {
        paused: Arc::clone(&paused),
        sent: Arc::clone(&sent),
    })
    .unwrap();
    let handle = worker.handle();
    let producer = worker.take_producer().unwrap();

    handle
        .send(OutputCommand::Connect {
            activation: 1,
            port: OutputPort {
                id: 1,
                name: "probe".into(),
            },
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match handle.try_event() {
            Ok(OutputEvent::Connection {
                activation: 1,
                result: Ok(OutputConnectionState::Connected(_)),
            }) => break,
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            other => panic!("expected connected probe endpoint: {other:?}"),
        }
    }

    // Pause backend and send one event to hold the worker inside send so we can
    // exercise normal enqueue, saturation overflow, priority NoteOff, stale/invalid
    // refusals, and priority Stop all inside one allocation-free callback scope.
    paused.store(true, Ordering::Release);
    assert_eq!(
        producer.try_send(
            1,
            TimestampedOutputMessage {
                timestamp_ns: 1,
                message: OutputMessage::NoteOn {
                    channel: 0,
                    pitch: 60,
                    velocity: 100,
                },
            },
        ),
        OutputEnqueueResult::Queued
    );
    std::thread::sleep(Duration::from_millis(10));

    let measured = allocation_counter::measure(|| {
        assert_eq!(
            producer.try_send(
                99,
                TimestampedOutputMessage {
                    timestamp_ns: 2,
                    message: OutputMessage::NoteOn {
                        channel: 0,
                        pitch: 60,
                        velocity: 100,
                    },
                },
            ),
            OutputEnqueueResult::StaleActivation
        );
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 3,
                    message: OutputMessage::NoteOn {
                        channel: 16,
                        pitch: 60,
                        velocity: 100,
                    },
                },
            ),
            OutputEnqueueResult::InvalidMessage(OutputError::InvalidChannel)
        );
        let mut saw_overflow = false;
        for i in 0..(OUTPUT_NORMAL_CAPACITY + 4) {
            if producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 10 + i as u64,
                    message: OutputMessage::ControlChange {
                        channel: 0,
                        cc: 1,
                        value: (i % 128) as u8,
                    },
                },
            ) == OutputEnqueueResult::Overflowed
            {
                saw_overflow = true;
                break;
            }
        }
        assert!(saw_overflow);
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 9_000,
                    message: OutputMessage::NoteOff {
                        channel: 0,
                        pitch: 60,
                        velocity: 0,
                    },
                },
            ),
            OutputEnqueueResult::QueuedPriority
        );
        assert_eq!(
            producer.request_stop(1),
            OutputEnqueueResult::QueuedPriority
        );
    });

    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current,
        ),
        (0, 0, 0, 0),
        "OutputProducer handoff must allocate and free zero bytes on callback thread"
    );

    paused.store(false, Ordering::Release);
    drop(worker);
}
