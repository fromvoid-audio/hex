//! Direct real-provider allocation scopes for lookahead/routing layouts.
//! Routing host publication is a later slice.
//! Construction, expected-output rendering and reclamation stay outside scopes.
use allocation_counter::{AllocationInfo, measure};
use hex_devices::limiter::{DEFINITION as D, LimiterDefinition, parameter::*};
use hex_lattice::{
    EMPTY_DEVICE_RESOURCES as R, NativeParameterAutomation, PreparedOwnedProcessorRuntime,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
const IDS: [ParameterId; 11] = [
    GAIN,
    CEILING,
    RELEASE,
    AUTO_RELEASE,
    LINK,
    LOOKAHEAD,
    ROUTING,
    MAXIMIZE,
    THRESHOLD,
    OUTPUT,
    LINK_MID_SIDE,
];
struct Values {
    start: usize,
    bad: bool,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &IDS
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        let i = self.start + offset;
        Some(match id {
            LOOKAHEAD => ParameterValue::Choice((i / 17 % 3) as u32),
            ROUTING => ParameterValue::Choice(if self.bad && offset == 17 {
                2
            } else {
                (i / 29 % 2) as u32
            }),
            GAIN => ParameterValue::Float(-12.0 + (i % 41) as f32 * 0.6),
            CEILING => ParameterValue::Float(-24.0 + (i % 29) as f32 * 0.8),
            RELEASE => ParameterValue::Float(0.1 + (i % 31) as f32 * 10.0),
            AUTO_RELEASE => ParameterValue::Bool(i / 37 % 2 == 1),
            LINK => ParameterValue::Float((i % 47) as f32 / 46.0),
            MAXIMIZE => ParameterValue::Bool(i / 53 % 2 == 1),
            THRESHOLD => ParameterValue::Float(-12.0 + (i % 43) as f32 * 0.6),
            OUTPUT => ParameterValue::Float(-24.0 + (i % 37) as f32 * 0.6),
            LINK_MID_SIDE => ParameterValue::Float((i % 31) as f32 / 30.0),
            _ => return None,
        })
    }
}
fn zero(info: AllocationInfo) {
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
#[test]
fn limiter_structural_real_provider_transitions_rollback_bypass_and_tail_allocate_nothing() {
    for (rate, mode, route) in [(8000, 0, 0), (48000, 1, 1), (192000, 2, 1)] {
        let mut s = D.default_state().unwrap();
        for (id, value) in [(MODE, mode), (ROUTING, route)] {
            s = D
                .patch_parameter(&s, id, ParameterValue::Choice(value))
                .unwrap();
        }
        let factory =
            LimiterDefinition::prepare_owned_structural_for_parameters(&s, rate, &IDS).unwrap();
        let latency = factory.algorithmic_latency_frames().unwrap() as usize;
        let frames = (latency * 3 + 256 * 3).div_ceil(32) * 32;
        let mut actual = factory.create_runtime().unwrap();
        let mut oracle = factory.create_runtime().unwrap();
        let input = [1.5, -0.3];
        let mut expected = vec![input; frames];
        let mut output = expected.clone();
        for (i, block) in expected.chunks_mut(32).enumerate() {
            oracle.begin_block();
            oracle
                .process_block_with_automation(
                    block,
                    None,
                    &Values {
                        start: i * 32,
                        bad: false,
                    },
                    &R,
                )
                .unwrap();
            oracle.commit_block();
        }
        let mut failed = false;
        zero(measure(|| {
            for (i, block) in output.chunks_mut(32).enumerate() {
                actual.begin_block();
                failed |= actual
                    .process_block_with_automation(
                        block,
                        None,
                        &Values {
                            start: i * 32,
                            bad: false,
                        },
                        &R,
                    )
                    .is_err();
                actual.commit_block();
            }
        }));
        assert!(!failed);
        assert_eq!(output, expected);
        let mut rejected = [input; 32];
        let mut refused = false;
        zero(measure(|| {
            actual.begin_block();
            refused = actual
                .process_block_with_automation(
                    &mut rejected,
                    None,
                    &Values {
                        start: frames,
                        bad: true,
                    },
                    &R,
                )
                .is_err();
            actual.rollback_block();
        }));
        assert!(refused);
        let values = Values {
            start: frames,
            bad: false,
        };
        let mut reference = [input; 64];
        oracle.begin_block();
        oracle
            .process_block_with_automation(&mut reference, None, &values, &R)
            .unwrap();
        oracle.commit_block();
        let mut future = [input; 64];
        zero(measure(|| {
            actual.begin_block();
            failed = actual
                .process_block_with_automation(&mut future, None, &values, &R)
                .is_err();
            actual.commit_block();
        }));
        assert!(!failed);
        assert_eq!(future, reference);
        let mut dry = [input; 64];
        let mut wet = dry;
        oracle.begin_block();
        oracle
            .process_block_with_automation(&mut wet, None, &values, &R)
            .unwrap();
        oracle.commit_block();
        zero(measure(|| {
            actual.begin_block();
            failed = actual
                .process_bypassed_block_with_automation(&mut dry, None, &values, &R)
                .is_err();
            actual.commit_block();
        }));
        assert!(!failed);
        assert_eq!(dry, [input; 64]);
        let mut tail = vec![[0.0; 2]; latency + 256 + 64];
        zero(measure(|| {
            actual.begin_output_tail();
            actual.begin_block();
            actual.process_block(&mut tail, &R);
            actual.commit_block();
        }));
        assert!(actual.committed_is_quiescent());
        assert!(tail[latency..].iter().flatten().all(|v| *v == 0.0));
    }
}
