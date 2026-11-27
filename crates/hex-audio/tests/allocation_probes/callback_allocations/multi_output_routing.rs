use allocation_counter::measure;
use hex_lattice::{
    EMPTY_DEVICE_RESOURCES, MultiOutputDestinationBuffer, MultiOutputInstrumentRuntime,
    NativeGraphEdge, NativeInstrumentEvent, NativeMultiOutputLimits, NativeMultiOutputPlan,
    NativeMultiOutputRoute, NativeMultiOutputTopology, NativeOutputChannelLayout,
    NativeOutputDestination, NativeOutputDestinationInfo, NativeOutputPortDescriptor,
    NativeOutputPortId, PreparedMultiOutputRouter, StereoFrame,
};
use std::num::NonZeroU64;

fn id(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v).unwrap()
}

const PORTS: [NativeOutputPortDescriptor; 2] = [
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

struct TwoPortProbeInstrument {
    phase_main: f32,
    phase_aux: f32,
    active_main: bool,
    active_aux: bool,
}

impl MultiOutputInstrumentRuntime for TwoPortProbeInstrument {
    fn output_port_descriptors(&self) -> &[NativeOutputPortDescriptor] {
        &PORTS
    }

    fn preflight_events(
        &self,
        _events: &[NativeInstrumentEvent],
        _frame_count: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        Ok(())
    }

    fn process_block_multi_output(
        &mut self,
        events: &[NativeInstrumentEvent],
        outputs: &mut [&mut [StereoFrame]],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        let (main_out, rest) = outputs.split_at_mut(1);
        let aux_out = &mut rest[0];
        let mut idx = 0;
        for (frame, (main_frame, aux_frame)) in
            main_out[0].iter_mut().zip(aux_out.iter_mut()).enumerate()
        {
            while idx < events.len() && events[idx].frame_offset() == frame {
                match events[idx] {
                    NativeInstrumentEvent::NoteOn { pitch, .. } => {
                        if pitch < 60 {
                            self.active_main = true;
                        } else {
                            self.active_aux = true;
                        }
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        if occurrence_id == 1 {
                            self.active_main = false;
                        } else {
                            self.active_aux = false;
                        }
                    }
                    _ => {}
                }
                idx += 1;
            }
            if self.active_main {
                let s = (self.phase_main * std::f32::consts::TAU).sin() * 0.4;
                self.phase_main = (self.phase_main + 0.01) % 1.0;
                *main_frame = [s, -s];
            } else {
                *main_frame = [0.0; 2];
            }
            if self.active_aux {
                let s = (self.phase_aux * std::f32::consts::TAU).sin() * 0.3;
                self.phase_aux = (self.phase_aux + 0.02) % 1.0;
                *aux_frame = [s, s];
            } else {
                *aux_frame = [0.0; 2];
            }
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.phase_main = 0.0;
        self.phase_aux = 0.0;
        self.active_main = false;
        self.active_aux = false;
    }

    fn is_quiescent(&self) -> bool {
        !self.active_main && !self.active_aux
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[test]
fn two_port_multi_output_routing_pdc_handoff_and_capture_allocate_nothing_on_callback() {
    let source = id(100);
    let topology = NativeMultiOutputTopology {
        source_host: NativeOutputDestination::Track(id(1)),
        destinations: vec![
            NativeOutputDestinationInfo {
                destination: NativeOutputDestination::Track(id(2)),
                latency_frames: 4,
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
    };
    let plan = NativeMultiOutputPlan {
        source_id: source,
        routes: vec![
            NativeMultiOutputRoute {
                route_id: id(1),
                source_id: source,
                port_id: NativeOutputPortId::MAIN_STEREO,
                destination: NativeOutputDestination::Track(id(2)),
                gain: 0.8,
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
        ],
    };
    let limits = NativeMultiOutputLimits {
        max_block_frames: 256,
        ..NativeMultiOutputLimits::default()
    };

    // All control-side preparation & preallocation occur strictly outside callback measurement.
    let mut inst = TwoPortProbeInstrument {
        phase_main: 0.0,
        phase_aux: 0.0,
        active_main: false,
        active_aux: false,
    };
    let mut router =
        PreparedMultiOutputRouter::prepare_on_control(&plan, &PORTS, &topology, limits).unwrap();
    let mut successor =
        PreparedMultiOutputRouter::prepare_on_control(&plan, &PORTS, &topology, limits).unwrap();

    let mut p1 = vec![[0.0_f32; 2]; 256];
    let mut p2 = vec![[0.0_f32; 2]; 256];
    let mut track2 = vec![[0.0_f32; 2]; 256];
    let mut ret10 = vec![[0.0_f32; 2]; 256];
    let mut capture = vec![[0.0_f32; 2]; 256];

    let events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 36,
            velocity: 100,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 4,
            occurrence_id: 2,
            pitch: 72,
            velocity: 100,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 120,
            occurrence_id: 1,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 128,
            occurrence_id: 2,
        },
    ];

    let mut inherited = 0_usize;
    let info = measure(|| {
        inst.preflight_events(&events, 256, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        let mut outs: [&mut [StereoFrame]; 2] = [&mut p1[..], &mut p2[..]];
        inst.process_block_multi_output(&events, &mut outs, None, &EMPTY_DEVICE_RESOURCES)
            .unwrap();

        router.begin_block();
        let inputs: [&[StereoFrame]; 2] = [&p1[..], &p2[..]];
        let mut dests = [
            MultiOutputDestinationBuffer {
                destination: NativeOutputDestination::Track(id(2)),
                frames: &mut track2[..],
            },
            MultiOutputDestinationBuffer {
                destination: NativeOutputDestination::Return(id(10)),
                frames: &mut ret10[..],
            },
        ];
        router.route_ports(&inputs, &mut dests).unwrap();
        router.commit_block();

        // Rollback test + compatible handoff + capture mix + Stop reset.
        router.begin_block();
        router.rollback_block();
        inherited = successor.inherit_compatible_routes_from(&router).unwrap();
        for i in 0..256 {
            capture[i] = [track2[i][0] + ret10[i][0], track2[i][1] + ret10[i][1]];
        }
        router.reset();
        successor.reset();
        inst.reset();
    });

    assert_eq!(
        info.count_total, 0,
        "multi-output callback must allocate nothing: {info:?}"
    );
    assert_eq!(
        info.bytes_total, 0,
        "multi-output callback must allocate zero bytes: {info:?}"
    );
    assert_eq!(
        info.count_current, 0,
        "multi-output signed-net count must be zero: {info:?}"
    );
    assert_eq!(
        info.bytes_current, 0,
        "multi-output signed-net bytes must be zero: {info:?}"
    );
    assert_eq!(inherited, 2);
    assert!(capture.iter().flatten().any(|s| s.abs() > 1e-3));
    assert!(router.committed_is_quiescent());
    assert!(successor.committed_is_quiescent());
    assert!(inst.is_quiescent());

    // Control-side retirement outside measured callback scope.
    drop(successor);
    drop(router);
}
