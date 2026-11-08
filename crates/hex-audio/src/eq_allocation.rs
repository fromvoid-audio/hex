//! Instrumented bounded EQ callback work, without physical-device claims.
use hex_engine::eq::{
    AutomationLane, AutomationPoint, EQ_DEFINITION, EqState, Node, NodeId, NodeParameter,
};
use hex_lattice::{NativeDeviceDefinition, NativeInstrumentClock};
struct Clock;
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        Some((offset as u64) << 20)
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.beat_at_offset(offset)
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        Some(offset as u64)
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(true)
    }
}
#[test]
fn max_density_eq_processing_bypass_history_copy_and_reset_allocate_and_free_nothing() {
    let mut settings = EqState::default();
    for value in 1..=64 {
        let id = NodeId::new(value).unwrap();
        settings
            .add(Node::new(id, 1000.0, 0.0, 0).unwrap())
            .unwrap();
        settings
            .set_lane(AutomationLane {
                node: id,
                parameter: NodeParameter::Gain,
                points: vec![
                    AutomationPoint {
                        beat: 0,
                        normalized: 0.4,
                    },
                    AutomationPoint {
                        beat: 1 << 32,
                        normalized: 0.6,
                    },
                ],
            })
            .unwrap();
    }
    let prepared = EQ_DEFINITION
        .prepare_processor(
            &settings.encode().unwrap(),
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let mut a = prepared.create_runtime();
    let mut b = prepared.create_runtime();
    let mut frames = [[0.001, -0.001]; 1024];
    let clock = Clock;
    let measurement = allocation_counter::measure(|| {
        a.process_block_with_clock(
            &mut frames,
            Some(&clock),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        b.copy_history_from(a.as_ref());
        b.process_bypassed_block_with_clock(
            &mut frames,
            Some(&clock),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        b.reset();
        a.reset();
    });
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn combined_eq_retarget_and_bypass_handoff_does_not_allocate_or_free() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorInstance, ProcessorChainOwner, StereoMatrix,
    };
    use std::sync::Arc;
    let make = |gain, bypassed| {
        let mut settings = EqState {
            output_gain: gain,
            ..Default::default()
        };
        settings
            .add(Node::new(NodeId::new(1).unwrap(), 1_000.0, 6.0, 0).unwrap())
            .unwrap();
        let state = settings.encode().unwrap();
        let prepared = EQ_DEFINITION
            .prepare_processor(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 1,
            device_type: state.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state)),
            definition: Some(Arc::from(prepared)),
        }])
        .unwrap()
    };
    let mut old = make(1.0, false);
    let mut candidate = make(2.0, true);
    let mut frames = [[0.2, -0.1]; 64];
    old.begin_block();
    old.process_block(&mut frames).unwrap();
    old.commit_block();
    let mut admitted = false;
    let measurement = allocation_counter::measure(|| {
        admitted = candidate.can_retarget_parameters_from_range(&old, 0..1);
        candidate.transfer_history_from(&mut old);
        candidate.begin_block();
        candidate.process_block(&mut frames).unwrap();
        candidate.commit_block();
    });
    assert!(admitted);
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

mod node_automation;
