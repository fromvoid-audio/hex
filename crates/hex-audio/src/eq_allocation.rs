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
