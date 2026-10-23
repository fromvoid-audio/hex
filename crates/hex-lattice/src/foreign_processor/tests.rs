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
                *frame = [frame[0] * 0.25, frame[1] * 0.25];
            }
        }
        Ok(())
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
            let mut foreign = endpoint(&instance);
            let mut chain = NativeRuntimeChain::new_prepared(vec![instance]).unwrap();
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
            assert_eq!(
                foreign.first_input,
                Some(if bypassed { [1.0, -0.5] } else { [10.0, -5.0] })
            );
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
            &mut foreign,
            &mut |_, _, _, _, _, _| {},
        ),
        Err(DeviceContractError::ForeignProcessingFailed)
    );
    assert_eq!((foreign.segments, foreign.rejections), (1, 2));
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
