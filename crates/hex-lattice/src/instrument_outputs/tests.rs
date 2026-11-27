use super::*;
use crate::EMPTY_DEVICE_RESOURCES;

fn id(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v).unwrap()
}

const TWO_PORTS: [NativeOutputPortDescriptor; 2] = [
    NativeOutputPortDescriptor {
        id: NativeOutputPortId::MAIN_STEREO,
        label: "main",
        layout: NativeOutputChannelLayout::Stereo,
        algorithmic_latency_frames: 8,
    },
    NativeOutputPortDescriptor {
        id: NativeOutputPortId::AUX_STEREO,
        label: "aux",
        layout: NativeOutputChannelLayout::Stereo,
        algorithmic_latency_frames: 2,
    },
];

/// Deterministic two-port drum-split fixture: pitches < 60 emit on Port 1 (`main`),
/// pitches >= 60 emit on Port 2 (`aux`) with distinct port algorithmic latencies.
struct TwoPortSplitRuntime {
    main_delay: [[f32; 2]; 16],
    main_next: usize,
    aux_delay: [[f32; 2]; 16],
    aux_next: usize,
}

impl TwoPortSplitRuntime {
    fn new() -> Self {
        Self {
            main_delay: [[0.0; 2]; 16],
            main_next: 0,
            aux_delay: [[0.0; 2]; 16],
            aux_next: 0,
        }
    }
}

impl MultiOutputInstrumentRuntime for TwoPortSplitRuntime {
    fn output_port_descriptors(&self) -> &[NativeOutputPortDescriptor] {
        &TWO_PORTS
    }

    fn preflight_events(
        &self,
        _events: &[NativeInstrumentEvent],
        _frame_count: usize,
        _resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }

    fn process_block_multi_output(
        &mut self,
        events: &[NativeInstrumentEvent],
        outputs: &mut [&mut [StereoFrame]],
        _clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let [out0, out1] = outputs else {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        };
        if out0.len() != out1.len() {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        let mut idx = 0;
        let main_lat = TWO_PORTS[0].algorithmic_latency_frames as usize;
        let aux_lat = TWO_PORTS[1].algorithmic_latency_frames as usize;
        for (frame, (o0, o1)) in out0.iter_mut().zip(out1.iter_mut()).enumerate() {
            let mut main_in = [0.0_f32; 2];
            let mut aux_in = [0.0_f32; 2];
            while idx < events.len() && events[idx].frame_offset() == frame {
                if let NativeInstrumentEvent::NoteOn {
                    pitch, velocity, ..
                } = events[idx]
                {
                    let amp = f32::from(velocity) / 100.0;
                    if pitch < 60 {
                        main_in[0] += amp;
                        main_in[1] -= amp;
                    } else {
                        aux_in[0] += amp;
                        aux_in[1] += amp;
                    }
                }
                idx += 1;
            }
            let main_out = self.main_delay[self.main_next];
            self.main_delay[self.main_next] = main_in;
            self.main_next = (self.main_next + 1) % main_lat;
            o0[0] += main_out[0];
            o0[1] += main_out[1];

            let aux_out = self.aux_delay[self.aux_next];
            self.aux_delay[self.aux_next] = aux_in;
            self.aux_next = (self.aux_next + 1) % aux_lat;
            o1[0] += aux_out[0];
            o1[1] += aux_out[1];
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.main_delay.fill([0.0; 2]);
        self.main_next = 0;
        self.aux_delay.fill([0.0; 2]);
        self.aux_next = 0;
    }

    fn is_quiescent(&self) -> bool {
        let main_lat = TWO_PORTS[0].algorithmic_latency_frames as usize;
        let aux_lat = TWO_PORTS[1].algorithmic_latency_frames as usize;
        self.main_delay[..main_lat]
            .iter()
            .all(|f| f[0] == 0.0 && f[1] == 0.0)
            && self.aux_delay[..aux_lat]
                .iter()
                .all(|f| f[0] == 0.0 && f[1] == 0.0)
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

fn sample_topology() -> NativeMultiOutputTopology {
    NativeMultiOutputTopology {
        source_host: NativeOutputDestination::Track(id(1)),
        destinations: vec![
            NativeOutputDestinationInfo {
                destination: NativeOutputDestination::Track(id(2)),
                latency_frames: 4,
            },
            NativeOutputDestinationInfo {
                destination: NativeOutputDestination::Track(id(3)),
                latency_frames: 6,
            },
            NativeOutputDestinationInfo {
                destination: NativeOutputDestination::Return(id(10)),
                latency_frames: 0,
            },
        ],
        edges: vec![NativeGraphEdge {
            from: NativeOutputDestination::Track(id(2)),
            to: NativeOutputDestination::Return(id(10)),
        }],
    }
}

#[test]
fn two_port_track_return_fanout_pdc_alignment_and_capture_export_match_exact_oracle() {
    let topology = sample_topology();
    let limits = NativeMultiOutputLimits::default();
    let source = id(50);

    // Port 1 (lat 8) -> Track 2 (lat 4): path = 12 (target arrival = 12, comp delay = 0)
    // Port 2 (lat 2) -> Return 10 (lat 0): path = 2 (comp delay = 10), gain 0.5
    // Port 2 (lat 2) -> Track 3 (lat 6): path = 8 (comp delay = 4), gain 0.25 (fanout!)
    let plan = NativeMultiOutputPlan {
        source_id: source,
        routes: vec![
            NativeMultiOutputRoute {
                route_id: id(1),
                source_id: source,
                port_id: NativeOutputPortId::MAIN_STEREO,
                destination: NativeOutputDestination::Track(id(2)),
                gain: 1.0,
                enabled: true,
            },
            NativeMultiOutputRoute {
                route_id: id(2),
                source_id: source,
                port_id: NativeOutputPortId::AUX_STEREO,
                destination: NativeOutputDestination::Return(id(10)),
                gain: 0.5,
                enabled: true,
            },
            NativeMultiOutputRoute {
                route_id: id(3),
                source_id: source,
                port_id: NativeOutputPortId::AUX_STEREO,
                destination: NativeOutputDestination::Track(id(3)),
                gain: 0.25,
                enabled: true,
            },
        ],
    };

    let router =
        PreparedMultiOutputRouter::prepare_on_control(&plan, &TWO_PORTS, &topology, limits)
            .unwrap();
    assert_eq!(router.target_arrival_frames(), 12);
    assert_eq!(router.route_compensation_delay_frames(id(1)), Some(0));
    assert_eq!(router.route_compensation_delay_frames(id(2)), Some(10));
    assert_eq!(router.route_compensation_delay_frames(id(3)), Some(4));

    let total = 32_usize;
    let events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 36, // kick -> Port 1 (main)
            velocity: 100,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 2,
            pitch: 72, // hat -> Port 2 (aux)
            velocity: 100,
        },
    ];

    let run = |partitions: &[usize]| {
        let mut inst = TwoPortSplitRuntime::new();
        let mut router =
            PreparedMultiOutputRouter::prepare_on_control(&plan, &TWO_PORTS, &topology, limits)
                .unwrap();
        let mut track2 = vec![[0.0_f32; 2]; total];
        let mut track3 = vec![[0.0_f32; 2]; total];
        let mut ret10 = vec![[0.0_f32; 2]; total];
        let mut p1 = vec![[0.0_f32; 2]; total];
        let mut p2 = vec![[0.0_f32; 2]; total];

        let mut cursor = 0;
        let mut step = 0;
        while cursor < total {
            let len = partitions[step % partitions.len()].min(total - cursor);
            let end = cursor + len;
            let local_events: Vec<_> = events
                .iter()
                .copied()
                .filter(|e| e.frame_offset() >= cursor && e.frame_offset() < end)
                .collect();
            let mut outs: [&mut [StereoFrame]; 2] = [&mut p1[cursor..end], &mut p2[cursor..end]];
            inst.process_block_multi_output(
                &local_events,
                &mut outs,
                None,
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();

            router.begin_block();
            let inputs: [&[StereoFrame]; 2] = [&p1[cursor..end], &p2[cursor..end]];
            let mut dests = [
                MultiOutputDestinationBuffer {
                    destination: NativeOutputDestination::Track(id(2)),
                    frames: &mut track2[cursor..end],
                },
                MultiOutputDestinationBuffer {
                    destination: NativeOutputDestination::Track(id(3)),
                    frames: &mut track3[cursor..end],
                },
                MultiOutputDestinationBuffer {
                    destination: NativeOutputDestination::Return(id(10)),
                    frames: &mut ret10[cursor..end],
                },
            ];
            router.route_ports(&inputs, &mut dests).unwrap();
            router.commit_block();
            cursor = end;
            step += 1;
        }
        assert!(inst.is_quiescent());
        assert!(router.committed_is_quiescent());
        (track2, track3, ret10)
    };

    let (t2_whole, t3_whole, r10_whole) = run(&[32]);
    let (t2_chunk, t3_chunk, r10_chunk) = run(&[1, 3, 7, 13, 8]);
    assert_eq!(t2_chunk, t2_whole);
    assert_eq!(t3_chunk, t3_whole);
    assert_eq!(r10_chunk, r10_whole);

    // Arrival verification:
    // - Track 2 receives Port 1 at frame 8 (plus Track 2's own 4-frame insert latency = frame 12!).
    // - Track 3 receives Port 2 at frame 2 + 4 (PDC) = frame 6 (plus Track 3's 6-frame latency = frame 12!).
    // - Return 10 receives Port 2 at frame 2 + 10 (PDC) = frame 12 (plus Return 10's 0 latency = frame 12!).
    assert_eq!(t2_whole[8], [1.0, -1.0]);
    assert_eq!(t3_whole[6], [0.25, 0.25]);
    assert_eq!(r10_whole[12], [0.5, 0.5]);

    // Simulated downstream capture/export alignment check: delay Track 2 by 4, Track 3 by 6, Return 10 by 0.
    // All 3 destinations arrive in exact sample alignment at global export frame 12!
    let mut capture = vec![[0.0_f32; 2]; total];
    for i in 0..total {
        let s_t2 = if i >= 4 { t2_whole[i - 4] } else { [0.0; 2] };
        let s_t3 = if i >= 6 { t3_whole[i - 6] } else { [0.0; 2] };
        let s_r10 = r10_whole[i];
        capture[i] = [s_t2[0] + s_t3[0] + s_r10[0], s_t2[1] + s_t3[1] + s_r10[1]];
    }
    for (frame, &sample) in capture.iter().enumerate() {
        if frame == 12 {
            assert_eq!(sample, [1.75, -0.25]);
        } else {
            assert_eq!(sample, [0.0, 0.0], "misaligned arrival at frame {frame}");
        }
    }
}

#[test]
fn stale_deleted_cyclic_saturated_routes_disconnect_history_and_persistence_preserve_state() {
    let mut topology = sample_topology();
    let limits = NativeMultiOutputLimits {
        max_fanout_per_port: 2,
        max_routes: 3,
        ..NativeMultiOutputLimits::default()
    };
    let source = id(50);

    let initial = NativeMultiOutputPlan {
        source_id: source,
        routes: vec![NativeMultiOutputRoute {
            route_id: id(1),
            source_id: source,
            port_id: NativeOutputPortId::MAIN_STEREO,
            destination: NativeOutputDestination::Track(id(2)),
            gain: 1.0,
            enabled: true,
        }],
    };
    let mut history =
        NativeMultiOutputHistory::new(initial.clone(), &TWO_PORTS, &topology, &limits).unwrap();

    // 1. Direct self-cycle (routing into source_host Track 1) refuses atomically.
    topology.destinations.push(NativeOutputDestinationInfo {
        destination: NativeOutputDestination::Track(id(1)),
        latency_frames: 0,
    });
    assert_eq!(
        history.apply(
            NativeMultiOutputEdit::AddRoute(NativeMultiOutputRoute {
                route_id: id(2),
                source_id: source,
                port_id: NativeOutputPortId::AUX_STEREO,
                destination: NativeOutputDestination::Track(id(1)),
                gain: 1.0,
                enabled: true,
            }),
            &TWO_PORTS,
            &topology,
            &limits,
        ),
        Err(MultiOutputRoutingError::CyclicRoute)
    );
    assert_eq!(history.current(), &initial);

    // 2. Transitive cycle (Track 3 -> Return 10 -> Track 1 source_host) refuses atomically.
    let mut cyclic_topo = topology.clone();
    cyclic_topo.edges.push(NativeGraphEdge {
        from: NativeOutputDestination::Track(id(3)),
        to: NativeOutputDestination::Return(id(10)),
    });
    cyclic_topo.edges.push(NativeGraphEdge {
        from: NativeOutputDestination::Return(id(10)),
        to: NativeOutputDestination::Track(id(1)),
    });
    assert_eq!(
        history.apply(
            NativeMultiOutputEdit::AddRoute(NativeMultiOutputRoute {
                route_id: id(2),
                source_id: source,
                port_id: NativeOutputPortId::AUX_STEREO,
                destination: NativeOutputDestination::Track(id(3)),
                gain: 1.0,
                enabled: true,
            }),
            &TWO_PORTS,
            &cyclic_topo,
            &limits,
        ),
        Err(MultiOutputRoutingError::CyclicRoute)
    );

    // 3. Stale source ID & deleted destination refuse atomically.
    assert_eq!(
        history.apply(
            NativeMultiOutputEdit::AddRoute(NativeMultiOutputRoute {
                route_id: id(2),
                source_id: id(999),
                port_id: NativeOutputPortId::AUX_STEREO,
                destination: NativeOutputDestination::Track(id(3)),
                gain: 1.0,
                enabled: true,
            }),
            &TWO_PORTS,
            &topology,
            &limits,
        ),
        Err(MultiOutputRoutingError::StaleSourceIdentity)
    );
    assert_eq!(
        history.apply(
            NativeMultiOutputEdit::AddRoute(NativeMultiOutputRoute {
                route_id: id(2),
                source_id: source,
                port_id: NativeOutputPortId::AUX_STEREO,
                destination: NativeOutputDestination::Track(id(404)),
                gain: 1.0,
                enabled: true,
            }),
            &TWO_PORTS,
            &topology,
            &limits,
        ),
        Err(MultiOutputRoutingError::UnknownDestination)
    );

    // 4. Fanout saturation (max_fanout_per_port = 2) refuses 3rd destination on Port 1.
    history
        .apply(
            NativeMultiOutputEdit::AddRoute(NativeMultiOutputRoute {
                route_id: id(2),
                source_id: source,
                port_id: NativeOutputPortId::MAIN_STEREO,
                destination: NativeOutputDestination::Return(id(10)),
                gain: 0.5,
                enabled: true,
            }),
            &TWO_PORTS,
            &topology,
            &limits,
        )
        .unwrap();
    assert_eq!(
        history.apply(
            NativeMultiOutputEdit::AddRoute(NativeMultiOutputRoute {
                route_id: id(3),
                source_id: source,
                port_id: NativeOutputPortId::MAIN_STEREO,
                destination: NativeOutputDestination::Track(id(3)),
                gain: 0.5,
                enabled: true,
            }),
            &TWO_PORTS,
            &topology,
            &limits,
        ),
        Err(MultiOutputRoutingError::FanoutBudgetExceeded)
    );

    // 5. Disconnect policy + atomic one-step Undo/Redo + strict binary persistence round-trip.
    history
        .apply(
            NativeMultiOutputEdit::DisconnectPort(NativeOutputPortId::MAIN_STEREO),
            &TWO_PORTS,
            &topology,
            &limits,
        )
        .unwrap();
    assert!(
        history
            .current()
            .routes
            .iter()
            .all(|r| r.destination == NativeOutputDestination::Unconnected)
    );
    history.undo().unwrap();
    assert_eq!(history.current().routes.len(), 2);
    assert_eq!(
        history.current().routes[0].destination,
        NativeOutputDestination::Track(id(2))
    );

    let encoded = history.current().encode_bytes().unwrap();
    let decoded = NativeMultiOutputPlan::decode_bytes(&encoded).unwrap();
    assert_eq!(&decoded, history.current());

    // Corrupted trailing byte & unsupported version refuse strictly.
    let mut trailing = encoded.clone();
    trailing.push(0xFF);
    assert_eq!(
        NativeMultiOutputPlan::decode_bytes(&trailing),
        Err(MultiOutputRoutingError::MalformedPlanBytes)
    );
    let mut future_ver = encoded;
    future_ver[4..8].copy_from_slice(&99_u32.to_le_bytes());
    assert_eq!(
        NativeMultiOutputPlan::decode_bytes(&future_ver),
        Err(MultiOutputRoutingError::UnsupportedPlanVersion)
    );

    // Decorative / uppercase port descriptor validation refuses.
    let bad_ports = [NativeOutputPortDescriptor {
        id: NativeOutputPortId::MAIN_STEREO,
        label: "Main",
        layout: NativeOutputChannelLayout::Stereo,
        algorithmic_latency_frames: 0,
    }];
    assert_eq!(
        validate_output_port_descriptors(&bad_ports),
        Err(MultiOutputRoutingError::InvalidPortDescriptors)
    );
}
