//! Zero-allocation callback-boundary probe for `hex_midi::OutputProducer`.
//! Construction, connection, and worker shutdown remain outside measured scopes.

use hex_midi::{
    EncodedMidiMessage, MpeConfig, MpeInitialExpression, MpeNoteOn, MpeOutputAllocator,
    MpeOutputError, MpeOutputOccurrence, MpeZone, OUTPUT_NORMAL_CAPACITY, OutputBackend,
    OutputCommand, OutputConnectionState, OutputEnqueueResult, OutputError, OutputEvent,
    OutputMessage, OutputPort, OutputSendError, OutputWorker, TimestampedOutputMessage,
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
#[test]
fn mpe_output_allocator_callback_path_has_zero_total_and_net_allocations() {
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

    let config = MpeConfig::with_ranges(MpeZone::Lower { member_channels: 2 }, 48, 2).unwrap();
    let mut allocator = MpeOutputAllocator::new(config).unwrap();
    let o1 = MpeOutputOccurrence {
        source_id: 1,
        occurrence_id: 1,
    };
    let o2 = MpeOutputOccurrence {
        source_id: 1,
        occurrence_id: 2,
    };
    let o3 = MpeOutputOccurrence {
        source_id: 1,
        occurrence_id: 3,
    };

    let measured = allocation_counter::measure(|| {
        allocator.send_zone_setup(1, &producer, 100).unwrap();
        let ch1 = allocator
            .note_on(
                1,
                &producer,
                200,
                MpeNoteOn {
                    occurrence: o1,
                    pitch: 60,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        let ch2 = allocator
            .note_on(
                1,
                &producer,
                210,
                MpeNoteOn {
                    occurrence: o2,
                    pitch: 60,
                    velocity: 110,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        assert_ne!(ch1, ch2);
        assert_eq!(
            allocator.note_on(
                1,
                &producer,
                220,
                MpeNoteOn {
                    occurrence: o3,
                    pitch: 64,
                    velocity: 90,
                    initial: MpeInitialExpression::default(),
                },
            ),
            Err(MpeOutputError::ChannelsExhausted)
        );
        allocator
            .send_note_pitch_bend(1, &producer, 230, o1, 600)
            .unwrap();
        allocator
            .send_note_pressure(1, &producer, 240, o1, 80)
            .unwrap();
        allocator
            .send_note_timbre(1, &producer, 250, o2, 95)
            .unwrap();
        allocator.note_off(1, &producer, 300, o1, 0).unwrap();
        assert_eq!(
            allocator.send_note_pressure(1, &producer, 310, o1, 40),
            Err(MpeOutputError::UnknownOccurrence)
        );
        allocator.stop(1, &producer).unwrap();
    });

    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current,
        ),
        (0, 0, 0, 0),
        "MpeOutputAllocator callback path must allocate and free zero bytes"
    );

    drop(worker);
}

#[test]
fn external_instrument_midi_and_audio_loopback_stop_disconnect_and_zero_callback_allocs() {
    use crate::native_audio::external_instrument::MidiOutputProducerBridge;
    use hex_audio::external_instrument::{
        ExternalInstrumentAudioBackend, ExternalInstrumentHost, ExternalInstrumentReturnEndpoint,
        ExternalInstrumentState, ExternalInstrumentStatus, ExternalMidiEvent,
    };
    use hex_audio::hardware_send_return::{
        ClockPolicy, HardwareAudioPortId, HardwareChannelSelection, HardwareDeviceCapabilities,
        HardwareLatencyPolicy, HardwareRouteError,
    };

    struct SynthReturnStream {
        endpoint: ExternalInstrumentReturnEndpoint,
        drops: Arc<std::sync::atomic::AtomicU64>,
    }
    impl Drop for SynthReturnStream {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct SynthAudioBackend {
        drops: Arc<std::sync::atomic::AtomicU64>,
    }
    impl ExternalInstrumentAudioBackend for SynthAudioBackend {
        type ReturnStream = SynthReturnStream;

        fn resolve_return_device(
            &mut self,
            port: &HardwareAudioPortId,
        ) -> Result<HardwareDeviceCapabilities, HardwareRouteError> {
            if port.as_str() == "coreaudio:synth-in" {
                Ok(HardwareDeviceCapabilities {
                    port: port.clone(),
                    channels: 6,
                    supported_sample_rates: vec![48_000],
                    is_software_loopback: false,
                })
            } else {
                Err(HardwareRouteError::MissingReturnDevice)
            }
        }

        fn open_return_stream(
            &mut self,
            _port: &HardwareAudioPortId,
            endpoint: ExternalInstrumentReturnEndpoint,
        ) -> Result<Self::ReturnStream, HardwareRouteError> {
            Ok(SynthReturnStream {
                endpoint,
                drops: Arc::clone(&self.drops),
            })
        }
    }

    // 1. Start existing MIDI OutputWorker with synthetic endpoint (port 1).
    let paused = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut midi_worker = OutputWorker::start_with_backend(ProbeBackend {
        paused: Arc::clone(&paused),
        sent: Arc::clone(&sent),
    })
    .unwrap();
    let midi_handle = midi_worker.handle();
    let midi_producer = midi_worker.take_producer().unwrap();
    midi_handle
        .send(OutputCommand::Connect {
            activation: 1,
            port: OutputPort {
                id: 1,
                name: "synth".into(),
            },
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match midi_handle.try_event() {
            Ok(OutputEvent::Connection {
                activation: 1,
                result: Ok(OutputConnectionState::Connected(_)),
            }) => break,
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            other => panic!("expected connected synth midi port: {other:?}"),
        }
    }

    // 2. Prepare External Instrument audio return (non-default channels 4..5 at 48 kHz).
    let drops = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (mut host, mut cb) = ExternalInstrumentHost::new(SynthAudioBackend {
        drops: Arc::clone(&drops),
    });
    let state = ExternalInstrumentState {
        midi_port_id: Some(1),
        midi_port_name: "synth".into(),
        midi_channel: 2,
        return_port_uid: "coreaudio:synth-in".into(),
        return_channels: HardwareChannelSelection::StereoPair {
            left_index: 4,
            right_index: 5,
        },
        return_gain_db: 0.0,
        latency_policy: HardwareLatencyPolicy {
            declared_round_trip_frames: 192,
            manual_correction_frames: 0,
        },
        clock_policy: ClockPolicy::IndependentClock {
            max_drift_frames: 64,
        },
        monitoring_enabled: true,
    };

    // Offline render must be refused safely.
    assert_eq!(
        host.prepare_and_publish(1, 1, &state, 48_000, 64, None, true),
        Err(HardwareRouteError::OfflineRenderUnsupported)
    );
    let pdc = host
        .prepare_and_publish(1, 1, &state, 48_000, 64, None, false)
        .unwrap();
    assert_eq!(pdc, 192);

    // Feed synthetic hardware return audio (impulse on channels 4 & 5).
    let mut hw_in = vec![0.0_f32; 64 * 6];
    hw_in[4] = 0.75;
    hw_in[5] = -0.25;
    host.active_stream_mut()
        .unwrap()
        .endpoint
        .push_interleaved_return_buffer(&hw_in);

    let midi_events = [
        ExternalMidiEvent::NoteOn {
            frame_offset: 0,
            pitch: 60,
            velocity: 100,
        },
        ExternalMidiEvent::NoteOff {
            frame_offset: 48,
            pitch: 60,
            velocity: 0,
        },
    ];
    let mut audio_out = [[0.0_f32; 2]; 64];
    let bridge = MidiOutputProducerBridge(&midi_producer);

    // 3. Verify zero total and net callback allocations during live MIDI+audio processing + Stop.
    let measured = allocation_counter::measure(|| {
        let status = cb.process_block(10_000_000, &midi_events, &mut audio_out, &bridge);
        assert_eq!(
            status,
            ExternalInstrumentStatus::Active {
                generation: 1,
                effective_pdc_frames: 192,
                midi_sent: 2,
                midi_overflowed: 0,
                returned_frames: 64,
                underrun_frames: 0,
            }
        );
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current,
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(audio_out[0], [0.75, -0.25]);

    // Wait for MIDI worker to deliver NoteOn and NoteOff on channel 2.
    let deadline = Instant::now() + Duration::from_secs(2);
    while sent.lock().unwrap().len() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        *sent.lock().unwrap(),
        vec![vec![0x92, 60, 100], vec![0x82, 60, 0]]
    );

    // 4. Stop under load silences audio immediately and forwards Stop to MIDI without allocating.
    let stop_measured = allocation_counter::measure(|| {
        host.request_stop();
        let status = cb.process_block(12_000_000, &midi_events, &mut audio_out, &bridge);
        assert_eq!(status, ExternalInstrumentStatus::Stopped { generation: 1 });
    });
    assert_eq!(
        (
            stop_measured.count_total,
            stop_measured.bytes_total,
            stop_measured.count_current,
            stop_measured.bytes_current,
        ),
        (0, 0, 0, 0)
    );
    assert!(audio_out.iter().all(|&f| f == [0.0, 0.0]));

    // 5. Reconnect with generation 2; generation 1 drops strictly on control reclaim.
    host.prepare_and_publish(2, 1, &state, 48_000, 64, None, false)
        .unwrap();
    let status = cb.process_block(14_000_000, &[], &mut audio_out, &bridge);
    assert!(matches!(
        status,
        ExternalInstrumentStatus::Active { generation: 2, .. }
    ));
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert_eq!(host.reclaim_retired(), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 1);

    // Audio disconnect transitions cleanly while keeping authored state recoverable.
    host.active_state().unwrap().mark_audio_disconnected();
    assert_eq!(
        cb.process_block(16_000_000, &[], &mut audio_out, &bridge),
        ExternalInstrumentStatus::Disconnected { generation: 2 }
    );
    assert_eq!(host.authored_state(), &state);

    cb.retire_active_for_shutdown();
    assert_eq!(host.reclaim_retired(), 1);
    drop(midi_worker);
}
