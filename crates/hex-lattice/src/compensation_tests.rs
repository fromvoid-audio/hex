use super::*;

struct FixedPrepared {
    latency: Option<u32>,
    residual: OutputTail,
}

impl PreparedNativeProcessor for FixedPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<FixedRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(FixedRuntime {
            history: [[0.0; 2]; 3],
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.latency
    }
    fn output_tail(&self) -> OutputTail {
        self.residual
    }
}

struct FixedRuntime {
    history: [StereoFrame; 3],
}

impl NativeProcessorRuntime for FixedRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        for frame in frames {
            let delayed = self.history[0];
            self.history.rotate_left(1);
            self.history[2] = *frame;
            *frame = delayed;
        }
    }
    fn reset(&mut self) {
        self.history.fill([0.0; 2]);
    }
    fn is_quiescent(&self) -> bool {
        self.history.iter().all(|frame| *frame == [0.0; 2])
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.history = previous.history;
        }
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

fn instance(
    id: u64,
    definition: Option<std::sync::Arc<dyn PreparedNativeProcessor>>,
    bypassed: bool,
) -> PreparedProcessorInstance {
    PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: id,
        device_type: DeviceTypeId::from_static("test.compensation"),
        matrix: StereoMatrix::identity(),
        bypassed,
        sample_rate: 48_000,
        configuration: None,
        definition,
    }
}

#[test]
fn paired_taps_bracket_whole_entry_and_preserve_bypass_pcm() {
    for owner in [
        ProcessorChainOwner::Track(1),
        ProcessorChainOwner::Return(1),
    ] {
        for bypassed in [false, true] {
            let make_chain = || {
                let mut prepared = instance(9, None, bypassed);
                prepared.owner = owner;
                prepared.matrix = StereoMatrix::new([[0.5, 0.0], [0.0, -0.5]]).unwrap();
                let mut chain = NativeRuntimeChain::new(vec![prepared]).unwrap();
                chain.entries[0].input_gain = 2.0;
                chain.entries[0].output_gain = 0.25;
                chain
            };
            let input = [[0.5, -0.25], [0.0, 0.0], [-0.5, 0.25]];
            let mut expected = input;
            make_chain().process_block(&mut expected).unwrap();
            let mut actual = input;
            let mut observations = Vec::new();
            make_chain()
                .process_range_tapped_with_clock(
                    0..1,
                    &mut actual,
                    17,
                    Some(&Clock),
                    Some(&Clock),
                    &mut |got_owner, device, _, phase, offset, audio| {
                        assert_eq!((got_owner, device, offset), (owner, 9, 17));
                        observations.push((phase, audio.to_vec()));
                    },
                )
                .unwrap();
            assert_eq!(actual, expected, "observation cannot change PCM");
            assert_eq!(
                observations,
                vec![
                    (NodeSignalPhase::Input, input.to_vec()),
                    (NodeSignalPhase::Output, actual.to_vec()),
                ]
            );
            if bypassed {
                assert_eq!(actual, input);
            } else {
                assert_eq!(actual, [[0.125, 0.0625], [0.0, 0.0], [-0.125, -0.0625]]);
            }
        }
    }
}

fn fixed(
    id: u64,
    latency: Option<u32>,
    residual: u64,
    bypassed: bool,
) -> PreparedProcessorInstance {
    instance(
        id,
        Some(std::sync::Arc::new(FixedPrepared {
            latency,
            residual: OutputTail::Finite {
                max_tail_frames: residual,
            },
        })),
        bypassed,
    )
}

#[test]
fn intrinsic_latency_and_residual_together_bound_zero_input_drain() {
    let active = fixed(1, Some(3), 5, false);
    let bypassed = fixed(2, Some(3), 5, true);
    let unknown = fixed(3, None, 5, false);
    let matrix = instance(4, None, false);
    assert_eq!(active.algorithmic_latency_frames(), Some(3));
    assert_eq!(bypassed.algorithmic_latency_frames(), Some(0));
    assert_eq!(unknown.algorithmic_latency_frames(), None);
    assert_eq!(matrix.algorithmic_latency_frames(), Some(0));
    let chain = NativeRuntimeChain::new(vec![active, bypassed, unknown, matrix]).unwrap();
    assert_eq!(
        chain.algorithmic_latency_frames_for_entry(0).unwrap(),
        Some(3)
    );
    assert_eq!(
        chain.algorithmic_latency_frames_for_entry(1).unwrap(),
        Some(0)
    );
    assert_eq!(chain.algorithmic_latency_frames_for_entry(2).unwrap(), None);
    assert_eq!(
        chain.algorithmic_latency_frames_for_entry(3).unwrap(),
        Some(0)
    );
    assert_eq!(
        chain.algorithmic_latency_frames_for_entry(4),
        Err(DeviceContractError::InvalidDescriptor)
    );
    assert_eq!(
        chain.output_tail_for_range(0..1),
        Some(OutputTail::Finite { max_tail_frames: 5 })
    );
    assert_eq!(
        chain.zero_input_drain_for_range(0..1),
        Some(OutputTail::Finite { max_tail_frames: 8 })
    );
    assert_eq!(
        chain.zero_input_drain_for_range(1..2),
        Some(OutputTail::Finite { max_tail_frames: 8 })
    );
    assert_eq!(
        chain.zero_input_drain_for_range(0..2),
        Some(OutputTail::Finite {
            max_tail_frames: 16
        })
    );
    assert_eq!(
        chain.zero_input_drain_for_range(0..3),
        Some(OutputTail::Unbounded)
    );
    assert_eq!(
        chain.zero_input_drain_for_range(3..4),
        Some(OutputTail::Finite { max_tail_frames: 0 })
    );
    assert_eq!(
        chain.zero_input_drain_for_range(4..4),
        Some(OutputTail::Finite { max_tail_frames: 0 })
    );
    assert_eq!(chain.zero_input_drain_for_range(0..5), None);
    assert_eq!(
        chain.zero_input_drain_for_range(std::ops::Range { start: 2, end: 1 }),
        None
    );
    let overflow = NativeRuntimeChain::new(vec![fixed(1, Some(3), u64::MAX, false)]).unwrap();
    assert_eq!(overflow.zero_input_drain_for_range(0..1), None);
}

#[test]
fn finite_fixed_delay_remains_pending_under_initial_zero_output() {
    let mut chain = NativeRuntimeChain::new(vec![fixed(1, Some(3), 0, false)]).unwrap();
    assert_eq!(
        chain.zero_input_drain_for_range(0..1),
        Some(OutputTail::Finite { max_tail_frames: 3 })
    );
    chain.begin_block();
    let mut impulse = [[1.0, -0.5]];
    chain.process_range(0..1, &mut impulse).unwrap();
    assert_eq!(impulse, [[0.0; 2]]);
    chain.commit_block();
    assert!(!chain.committed_range_is_quiescent(0..1));
    chain.begin_block();
    let mut zeros = [[0.0; 2]; 3];
    chain.process_range(0..1, &mut zeros).unwrap();
    assert_eq!(zeros, [[0.0; 2], [0.0; 2], [1.0, -0.5]]);
    chain.commit_block();
    assert!(chain.committed_range_is_quiescent(0..1));
}

struct ClockPrepared;
impl PreparedNativeProcessor for ClockPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<ClockRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(ClockRuntime { last_beat: 0 })
    }
}

struct ClockRuntime {
    last_beat: u64,
}
impl NativeProcessorRuntime for ClockRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        frames.fill([7.0, 9.0]);
    }
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn crate::DeviceResources,
    ) {
        let Some(clock) = clock else {
            self.process_block(frames, _resources);
            return;
        };
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.last_beat = clock.beat_at_offset(offset).unwrap_or(0);
            *frame = [
                self.last_beat as f32,
                (clock.arrangement_frame_at_offset(offset).unwrap_or(0)
                    + 100 * clock.loop_pass_at_offset(offset).unwrap_or(0)) as f32,
            ];
        }
    }
    fn process_bypassed_block(
        &mut self,
        _frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        self.last_beat = 99;
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn crate::DeviceResources,
    ) {
        self.last_beat = clock
            .and_then(|clock| clock.beat_at_offset(frames.len().saturating_sub(1)))
            .unwrap_or(99);
    }
    fn reset(&mut self) {
        self.last_beat = 0;
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.last_beat = previous.last_beat;
        }
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

struct Clock;
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        Some(10 + offset as u64)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        Some(1000 + offset as u64)
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(false)
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        Some(offset as u64 % 2)
    }
    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        Some(offset as u64 / 2)
    }
}

#[test]
fn per_node_clock_reaches_processors_and_observers_without_changing_legacy_calls() {
    let mut chain = NativeRuntimeChain::new(vec![instance(
        1,
        Some(std::sync::Arc::new(ClockPrepared)),
        false,
    )])
    .unwrap();
    chain.begin_block();
    let mut frames = [[0.0; 2]; 4];
    chain
        .process_range_with_clock(0..1, &mut frames, Some(&Clock), Some(&Clock))
        .unwrap();
    assert_eq!(
        frames,
        [[10.0, 0.0], [11.0, 1.0], [12.0, 100.0], [13.0, 101.0]]
    );
    let mut observed = [[0.0; 2]; 4];
    chain
        .process_range_observed_with_clock(
            0..1,
            &mut frames,
            17,
            Some(&Clock),
            Some(&Clock),
            &mut |owner, id, _, offset, audio| {
                assert_eq!(owner, ProcessorChainOwner::Track(1));
                assert_eq!(id, 1);
                assert_eq!(offset, 17);
                observed.copy_from_slice(audio);
            },
        )
        .unwrap();
    assert_eq!(observed, frames);
    chain.process_range(0..1, &mut frames).unwrap();
    assert_eq!(frames, [[7.0, 9.0]; 4]);
    chain.process_block(&mut frames).unwrap();
    assert_eq!(frames, [[7.0, 9.0]; 4]);
    assert_eq!(
        chain.process_range_with_clock(0..2, &mut frames, Some(&Clock), Some(&Clock)),
        Err(DeviceContractError::InvalidDescriptor)
    );
}

#[test]
fn bypass_private_clock_uses_offsets_while_audible_samples_are_preserved() {
    for matrix in [
        StereoMatrix::identity(),
        StereoMatrix::new([[0.5, 0.0], [0.0, 0.5]]).unwrap(),
    ] {
        let mut prepared = instance(1, Some(std::sync::Arc::new(ClockPrepared)), true);
        prepared.matrix = matrix;
        let mut chain = NativeRuntimeChain::new(vec![prepared]).unwrap();
        chain.begin_block();
        let mut frames = [[1.0, -0.5]; 4];
        chain
            .process_range_with_clock(0..1, &mut frames, Some(&Clock), Some(&Clock))
            .unwrap();
        assert_eq!(frames, [[1.0, -0.5]; 4]);
        let runtime = chain.entries[0]
            .runtime
            .legacy()
            .unwrap()
            .1
            .as_any()
            .downcast_ref::<ClockRuntime>()
            .unwrap();
        assert_eq!(runtime.last_beat, 13);
        chain.process_range(0..1, &mut frames).unwrap();
        let runtime = chain.entries[0]
            .runtime
            .legacy()
            .unwrap()
            .1
            .as_any()
            .downcast_ref::<ClockRuntime>()
            .unwrap();
        assert_eq!(runtime.last_beat, 99);
    }
}

#[test]
fn offset_clock_preserves_host_origin_state_and_explicit_unknowns() {
    let clock = OffsetClock {
        clock: &Clock,
        offset: 3,
    };
    assert_eq!(clock.performance_frame_at_offset(4), Some(1007));
    assert_eq!(clock.beat_at_offset(4), Some(17));
    assert_eq!(clock.arrangement_frame_at_offset(4), Some(1));
    assert_eq!(clock.loop_pass_at_offset(4), Some(3));
    assert_eq!(clock.transport_is_running(), Some(false));
    assert_eq!(clock.performance_frame_at_offset(usize::MAX), None);
    struct Unknown;
    impl NativeInstrumentClock for Unknown {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
    }
    let unknown = OffsetClock {
        clock: &Unknown,
        offset: 3,
    };
    assert_eq!(unknown.performance_frame_at_offset(4), None);
    assert_eq!(unknown.transport_is_running(), None);
}
