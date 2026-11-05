//! Borrowed graph-contract checks, not SDK, resource-admission or deadline evidence.
use super::*;
use crate::{
    NativeRuntimeChain, NodeSignalPhase, PreparedProcessorFactory, PreparedRuntimeInstance,
    StereoMatrix,
};
use std::sync::Arc;

const TYPE: DeviceTypeId = DeviceTypeId::from_static("test.foreign");

fn state() -> Arc<NativeDeviceState> {
    Arc::new(
        NativeDeviceState::new(TYPE, 1, vec![1, 2, 3])
            .unwrap()
            .with_input_gain_db(20.0)
            .unwrap()
            .with_output_gain_db(-20.0)
            .unwrap(),
    )
}
fn declaration(owner: ProcessorChainOwner, bypassed: bool) -> PreparedRuntimeInstance {
    PreparedRuntimeInstance {
        automation: Default::default(),
        meter: Default::default(),
        owner,
        device_id: 17,
        device_type: TYPE,
        matrix: StereoMatrix::identity(),
        bypassed,
        sample_rate: 48_000,
        configuration: Some(state()),
        definition: Some(PreparedProcessorFactory::Foreign(
            PreparedForeignProcessor::new(8, 3, 16).unwrap(),
        )),
    }
}

struct BorrowedEndpoint {
    configuration: Arc<NativeDeviceState>,
    owner: ProcessorChainOwner,
    next: usize,
    segments: usize,
    first_input: Option<StereoFrame>,
    fail: bool,
    revoked: bool,
    rejections: usize,
    drained: bool,
}
impl ForeignProcessors for BorrowedEndpoint {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        if self.revoked {
            return Err(DeviceContractError::ForeignProcessingFailed);
        }
        assert_eq!(
            (node.owner, node.device_id, node.device_type),
            (self.owner, 17, TYPE)
        );
        assert_eq!(node.sample_rate, 48_000);
        assert!(std::ptr::eq(
            node.configuration,
            self.configuration.as_ref()
        ));
        assert_eq!(node.prepared.quantum_frames(), 8);
        assert_eq!(node.input_gain, 10.0);
        assert_eq!(frame_offset, self.next);
        self.next += frames.len();
        self.segments += 1;
        if self.first_input.is_none() {
            self.first_input = frames.first().copied();
        }
        if self.fail {
            return Err(DeviceContractError::ForeignProcessingFailed);
        }
        if !node.bypassed {
            for frame in frames {
                frame[0] *= node.input_gain;
                frame[1] *= node.input_gain;
                *frame = [frame[0] * 0.25, frame[1] * 0.25];
            }
        }
        Ok(())
    }
    fn committed_tail_is_drained(&self, _node: ForeignProcessorNode<'_>) -> bool {
        self.drained && !self.revoked
    }
    fn reject_callback(&mut self) {
        self.revoked = true;
        self.rejections += 1;
    }
}
fn endpoint(instance: &PreparedRuntimeInstance) -> BorrowedEndpoint {
    BorrowedEndpoint {
        configuration: Arc::clone(instance.configuration.as_ref().unwrap()),
        owner: instance.owner,
        next: 0,
        segments: 0,
        first_input: None,
        fail: false,
        revoked: false,
        rejections: 0,
        drained: false,
    }
}

#[test]
fn post_trim_endpoint_applies_output_trim_once_and_keeps_bypass_dry() {
    struct PostTrim;
    impl ForeignProcessors for PostTrim {
        fn returns_post_trim(&self, _: ForeignProcessorNode<'_>) -> bool {
            true
        }
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [StereoFrame],
            _: Option<&dyn NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            if !node.bypassed {
                for frame in frames {
                    frame[0] *= node.output_gain;
                    frame[1] *= node.output_gain;
                }
            }
            Ok(())
        }
        fn reject_callback(&mut self) {}
    }
    for bypassed in [false, true] {
        let instance = declaration(ProcessorChainOwner::Track(5), bypassed);
        let gain = instance
            .configuration
            .as_ref()
            .unwrap()
            .output_gain_linear();
        let mut chain = NativeRuntimeChain::new_prepared(vec![instance]).unwrap();
        let mut frames = [[0.25, -0.5]];
        chain.begin_block();
        chain
            .process_range_with_foreign(0..1, &mut frames, 0, None, None, &mut PostTrim)
            .unwrap();
        chain.commit_block();
        let expected = if bypassed { 1.0 } else { gain };
        assert!((frames[0][0] - 0.25 * expected).abs() < 1e-7);
    }
}

#[test]
fn foreign_nodes_borrow_exact_configuration_and_route_segments_for_all_chain_owners() {
    for owner in [
        ProcessorChainOwner::Track(5),
        ProcessorChainOwner::Return(9),
        ProcessorChainOwner::Main,
    ] {
        for bypassed in [false, true] {
            let instance = declaration(owner, bypassed);
            let meter = Arc::clone(&instance.meter);
            let mut foreign = endpoint(&instance);
            let mut chain = NativeRuntimeChain::new_prepared(vec![instance]).unwrap();
            chain.set_metering(true);
            assert_eq!(
                chain.algorithmic_latency_frames_for_entry(0).unwrap(),
                Some(19)
            );
            let mut frames = [[1.0, -0.5]; 5];
            let mut taps = Vec::new();
            chain.begin_block();
            let (first, second) = frames.split_at_mut(3);
            for (offset, segment) in [(0, first), (3, second)] {
                chain
                    .process_range_tapped_with_foreign(
                        0..1,
                        segment,
                        offset,
                        None,
                        None,
                        &mut foreign,
                        &mut |actual_owner, device, kind, phase, at, samples| {
                            assert_eq!((actual_owner, device, kind), (owner, 17, TYPE));
                            taps.push((phase, at, samples.len()));
                        },
                    )
                    .unwrap();
                let expected = if bypassed {
                    [1.0, -0.5]
                } else {
                    [0.25, -0.125]
                };
                for actual in segment {
                    for (actual, expected) in actual.iter().zip(expected) {
                        assert!((actual - expected).abs() < 1.0e-6);
                    }
                }
            }
            chain.commit_block();
            let peaks = meter.take_peaks();
            assert_eq!(peaks.input, 10.0);
            let output = peaks.output[0].max(peaks.output[1]);
            assert!((output - if bypassed { 1.0 } else { 0.25 }).abs() < 1.0e-6);
            assert_eq!(foreign.first_input, Some([1.0, -0.5]));
            assert_eq!(
                (foreign.segments, foreign.next, foreign.rejections),
                (2, 5, 0)
            );
            assert_eq!(
                taps,
                vec![
                    (NodeSignalPhase::Input, 0, 3),
                    (NodeSignalPhase::Output, 0, 3),
                    (NodeSignalPhase::Input, 3, 2),
                    (NodeSignalPhase::Output, 3, 2)
                ]
            );
        }
    }
}

#[test]
fn missing_foreign_context_refuses_and_failed_processing_revokes_without_replaying_history() {
    let instance = declaration(ProcessorChainOwner::Main, false);
    let mut foreign = endpoint(&instance);
    foreign.fail = true;
    let mut chain = NativeRuntimeChain::new_prepared(vec![instance]).unwrap();
    assert_eq!(
        chain.process_block(&mut [[1.0; 2]]),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    let mut taps = Vec::new();
    chain.begin_block();
    assert_eq!(
        chain.process_range_tapped_with_foreign(
            0..1,
            &mut [[1.0; 2]],
            0,
            None,
            None,
            &mut foreign,
            &mut |_, _, _, phase, _, _| taps.push(phase),
        ),
        Err(DeviceContractError::ForeignProcessingFailed)
    );
    chain.rollback_block();
    assert_eq!((foreign.segments, foreign.rejections), (1, 1));
    assert_eq!(taps, vec![NodeSignalPhase::Input]);
    chain.begin_block();
    assert_eq!(
        chain.process_range_tapped_with_foreign(
            0..1,
            &mut [[1.0; 2]],
            0,
            None,
            None,
            &mut foreign,
            &mut |_, _, _, _, _, _| {},
        ),
        Err(DeviceContractError::ForeignProcessingFailed)
    );
    assert_eq!((foreign.segments, foreign.rejections), (1, 2));
}

#[test]
fn aligned_foreign_bypass_keeps_only_the_same_graph_configuration_not_tail_or_source_authority() {
    let old = NativeRuntimeChain::new_prepared(vec![declaration(ProcessorChainOwner::Main, false)])
        .unwrap();
    let mut next =
        NativeRuntimeChain::new_prepared(vec![declaration(ProcessorChainOwner::Main, true)])
            .unwrap();
    assert!(old.configuration_matches_range(&next, 0..1));
    assert_eq!(
        old.zero_input_drain_for_range(0..1),
        Some(crate::OutputTail::Unbounded)
    );
    assert_eq!(
        next.process_block(&mut [[1.0; 2]]),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    for change in 0..3 {
        let mut changed = declaration(ProcessorChainOwner::Main, true);
        match change {
            0 => changed.device_id += 1,
            1 => changed.owner = ProcessorChainOwner::Track(5),
            _ => {
                changed.definition = Some(PreparedProcessorFactory::Foreign(
                    PreparedForeignProcessor::new(8, 4, 16).unwrap(),
                ))
            }
        }
        let changed = NativeRuntimeChain::new_prepared(vec![changed]).unwrap();
        assert!(!old.configuration_matches_range(&changed, 0..1));
    }
    let removed = NativeRuntimeChain::new_prepared(Vec::new()).unwrap();
    assert!(!old.configuration_matches_range(&removed, 0..1));
}

#[test]
fn graph_configuration_and_retargeting_keep_the_exact_shared_source_epoch() {
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: 17,
        graph: 2,
        epoch: 3,
    };
    let mut before = declaration(ProcessorChainOwner::Main, false);
    let facts = PreparedForeignProcessor::new(8, 3, 16).unwrap();
    before.definition = Some(PreparedProcessorFactory::Foreign(
        facts.with_binding(binding).unwrap(),
    ));
    let before = NativeRuntimeChain::new_prepared(vec![before]).unwrap();
    let mut after = declaration(ProcessorChainOwner::Main, false);
    after.definition = Some(PreparedProcessorFactory::Foreign(
        facts
            .with_binding(hex_lattice_contract::ForeignProcessorBinding {
                epoch: 4,
                ..binding
            })
            .unwrap(),
    ));
    let after = NativeRuntimeChain::new_prepared(vec![after]).unwrap();
    assert!(!before.configuration_matches_range(&after, 0..1));
    assert!(!after.can_retarget_parameters_from_range(&before, 0..1));
    assert!(
        facts
            .with_binding(hex_lattice_contract::ForeignProcessorBinding { root: 0, ..binding })
            .is_err()
    );
}

#[test]
fn malformed_foreign_declarations_refuse_before_any_endpoint_is_borrowed() {
    for args in [(0, 3, 16), (8, 3, 0), (8, u32::MAX, 1)] {
        assert!(PreparedForeignProcessor::new(args.0, args.1, args.2).is_err());
    }
    for variant in 0..3 {
        let mut instance = declaration(ProcessorChainOwner::Track(5), false);
        match variant {
            0 => instance.configuration = None,
            1 => {
                instance.configuration = Some(Arc::new(
                    NativeDeviceState::new(DeviceTypeId::from_static("test.other"), 1, vec![])
                        .unwrap(),
                ))
            }
            _ => instance.matrix = StereoMatrix::new([[0.5, 0.0], [0.0, 0.5]]).unwrap(),
        }
        assert!(matches!(
            NativeRuntimeChain::new_prepared(vec![instance]),
            Err(DeviceContractError::ForeignProcessingUnavailable)
        ));
    }
}

#[test]
fn finite_foreign_tail_requires_original_provider_observation_in_chain_transaction() {
    let mut instance = declaration(ProcessorChainOwner::Main, false);
    let facts = PreparedForeignProcessor::new(8, 3, 16)
        .unwrap()
        .with_output_tail(crate::OutputTail::Finite { max_tail_frames: 3 });
    instance.definition = Some(PreparedProcessorFactory::Foreign(facts));
    let mut foreign = endpoint(&instance);
    let mut chain = NativeRuntimeChain::new_prepared(vec![instance]).unwrap();
    let mut next = declaration(ProcessorChainOwner::Main, false);
    next.definition = Some(PreparedProcessorFactory::Foreign(
        PreparedForeignProcessor::new(8, 4, 16)
            .unwrap()
            .with_output_tail(crate::OutputTail::Finite { max_tail_frames: 3 }),
    ));
    let successor = NativeRuntimeChain::new_prepared(vec![next]).unwrap();
    assert!(!successor.can_inherit_with_completed_foreign_range(&chain, 0..1));
    assert_eq!(
        chain.zero_input_drain_for_range(0..1),
        Some(crate::OutputTail::Finite {
            max_tail_frames: 22
        })
    );
    assert!(!chain.committed_range_is_quiescent(0..1));
    chain.begin_block();
    chain
        .process_range_with_foreign(0..1, &mut [[0.0; 2]], 0, None, None, &mut foreign)
        .unwrap();
    chain.commit_block();
    assert!(
        !chain.committed_range_is_quiescent(0..1),
        "silence is not a host acceptance proof"
    );
    foreign.drained = true;
    chain.begin_block();
    chain
        .process_range_with_foreign(0..1, &mut [[0.0; 2]], 1, None, None, &mut foreign)
        .unwrap();
    assert!(chain.speculative_range_is_quiescent(0..1));
    assert!(!chain.committed_range_is_quiescent(0..1));
    assert!(!successor.can_inherit_with_completed_foreign_range(&chain, 0..1));
    chain.rollback_block();
    assert!(!chain.speculative_range_is_quiescent(0..1));
    chain.begin_block();
    chain
        .process_range_with_foreign(0..1, &mut [[0.0; 2]], 2, None, None, &mut foreign)
        .unwrap();
    chain.commit_block();
    assert!(chain.committed_range_is_quiescent(0..1));
    assert!(successor.can_inherit_with_completed_foreign_range(&chain, 0..1));
    assert!(!successor.can_inherit_from(&chain));
    foreign.drained = false;
    chain.begin_block();
    chain
        .process_range_with_foreign(0..1, &mut [[1.0; 2]], 3, None, None, &mut foreign)
        .unwrap();
    assert!(!chain.speculative_range_is_quiescent(0..1));
    chain.commit_block();
    assert!(!chain.committed_range_is_quiescent(0..1));
    assert!(!successor.can_inherit_with_completed_foreign_range(&chain, 0..1));
}

#[test]
fn parameter_capability_default_refuses_nonempty_without_silent_loss_and_preserves_empty_processing()
 {
    let declaration = declaration(ProcessorChainOwner::Main, false);
    let mut endpoint = endpoint(&declaration);
    let node = ForeignProcessorNode {
        owner: declaration.owner,
        device_id: 17,
        device_type: TYPE,
        sample_rate: 48_000,
        configuration: declaration.configuration.as_deref().unwrap(),
        prepared: PreparedForeignProcessor::new(8, 3, 16).unwrap(),
        bypassed: false,
        input_gain: 10.0,
        output_gain: 0.1,
    };
    let mut frames = [[1.0, -0.5]; 2];
    endpoint
        .process_segment_with_parameters(node, 0, &mut frames, &[], None)
        .unwrap();
    assert_eq!(frames, [[2.5, -1.25]; 2]);
    assert_eq!(endpoint.segments, 1);
    assert_eq!(
        endpoint.process_segment_with_parameters(
            node,
            2,
            &mut frames,
            &[ForeignParameterPoint {
                frame: 0,
                id: 0,
                normalized: 0.5
            }],
            None
        ),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    assert_eq!(frames, [[0.0; 2]; 2]);
    assert_eq!(endpoint.segments, 1);
    assert_eq!(endpoint.rejections, 1);
}
