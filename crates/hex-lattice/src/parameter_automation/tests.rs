use super::*;
use hex_lattice_contract::*;
fn descriptor() -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(1),
        label: "value",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 9.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    }
}
fn semantics() -> ParameterSemantics {
    ParameterSemantics {
        access: ParameterAccess::ReadWrite,
        normalization: ParameterNormalization::Linear,
        stepping: ParameterStepping::Continuous,
        automation: ParameterAutomation::DeviceArrangementEnvelope,
        modulation: ParameterModulation::Unsupported,
        smoothing: ParameterSmoothing::None,
        change_impact: ParameterChangeImpact::ValueOnly,
    }
}
fn program(
    descriptor: ParameterDescriptor,
    semantics: ParameterSemantics,
) -> PreparedParameterAutomation {
    PreparedParameterAutomation::new(
        vec![ParameterAutomationLane {
            descriptor,
            semantics,
            points: 0..2,
        }],
        vec![
            ParameterAutomationPoint {
                beat_raw: 10,
                normalized: 0.0,
                step: false,
            },
            ParameterAutomationPoint {
                beat_raw: 20,
                normalized: 1.0,
                step: true,
            },
        ],
    )
    .unwrap()
}
struct Clock;
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, _: usize) -> Option<u64> {
        Some(99999)
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        Some(offset as u64)
    }
}
#[test]
fn content_origin_linear_increment_discrete_and_absent_laws_are_partition_independent() {
    let id = descriptor().id;
    let continuous = program(descriptor(), semantics());
    let values = continuous.block(Some(&Clock), 32);
    for (offset, value) in [(0, 1.0), (10, 1.0), (15, 5.0), (20, 9.0), (31, 9.0)] {
        assert_eq!(
            values.value_at(id, offset),
            Some(ParameterValue::Float(value))
        );
    }
    assert_eq!(values.value_at(id, 32), None);
    assert_eq!(continuous.block(None, 32).value_at(id, 15), None);
    let stepped = program(
        descriptor(),
        ParameterSemantics {
            stepping: ParameterStepping::Increment(3.0),
            ..semantics()
        },
    );
    assert_eq!(
        stepped.block(Some(&Clock), 32).value_at(id, 15),
        Some(ParameterValue::Float(4.0))
    );
    let logarithmic = program(
        descriptor(),
        ParameterSemantics {
            normalization: ParameterNormalization::PositiveLogarithmic,
            ..semantics()
        },
    );
    assert_eq!(
        logarithmic.block(Some(&Clock), 32).value_at(id, 15),
        Some(ParameterValue::Float(3.0))
    );
    let choice = program(
        ParameterDescriptor {
            kind: ParameterKind::Choice {
                labels: &["one", "two", "three"],
            },
            default: ParameterValue::Choice(0),
            ..descriptor()
        },
        ParameterSemantics {
            normalization: ParameterNormalization::Discrete,
            stepping: ParameterStepping::Choice,
            ..semantics()
        },
    );
    assert_eq!(
        choice.block(Some(&Clock), 32).value_at(id, 19),
        Some(ParameterValue::Choice(0))
    );
    assert_eq!(
        choice.block(Some(&Clock), 32).value_at(id, 20),
        Some(ParameterValue::Choice(2))
    );
    let boolean = program(
        ParameterDescriptor {
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            ..descriptor()
        },
        ParameterSemantics {
            normalization: ParameterNormalization::Discrete,
            stepping: ParameterStepping::Boolean,
            ..semantics()
        },
    );
    assert_eq!(
        boolean.block(Some(&Clock), 32).value_at(id, 19),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        boolean.block(Some(&Clock), 32).value_at(id, 20),
        Some(ParameterValue::Bool(true))
    );
}
#[test]
fn constructor_refuses_invalid_domains_ranges_and_order_before_processing() {
    let lane = ParameterAutomationLane {
        descriptor: descriptor(),
        semantics: semantics(),
        points: 0..1,
    };
    let point = ParameterAutomationPoint {
        beat_raw: 0,
        normalized: 0.5,
        step: false,
    };
    for normalized in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(
            PreparedParameterAutomation::new(
                vec![lane.clone()],
                vec![ParameterAutomationPoint {
                    normalized,
                    ..point
                }]
            )
            .is_err()
        );
    }
    assert!(
        PreparedParameterAutomation::new(
            vec![ParameterAutomationLane {
                points: 1..2,
                ..lane.clone()
            }],
            vec![point, point]
        )
        .is_err()
    );
    assert!(
        PreparedParameterAutomation::new(
            vec![ParameterAutomationLane {
                points: 0..2,
                ..lane.clone()
            }],
            vec![point, point]
        )
        .is_err()
    );
    assert!(
        PreparedParameterAutomation::new(vec![lane.clone(); MAX_AUTOMATION_LANES + 1], vec![point])
            .is_err()
    );
    assert!(
        PreparedParameterAutomation::new(
            vec![ParameterAutomationLane {
                points: 0..MAX_AUTOMATION_POINTS_PER_LANE + 1,
                ..lane
            }],
            vec![point; MAX_AUTOMATION_POINTS_PER_LANE + 1]
        )
        .is_err()
    );
    let program = program(descriptor(), semantics());
    let (ids, lanes, points) = program.storage_vectors();
    assert_eq!(
        program.storage_bytes(),
        Some(
            ids.capacity() * std::mem::size_of::<ParameterId>()
                + lanes.capacity() * std::mem::size_of::<PreparedParameterAutomationLane>()
                + points.capacity() * std::mem::size_of::<ParameterAutomationPoint>()
        )
    );
}
#[test]
fn both_actual_runtime_twins_must_confirm_factory_parameter_support() {
    use crate::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Factory(AtomicUsize);
    struct Runtime(bool);
    impl PreparedNativeProcessor for Factory {
        fn parameter_automation_descriptor(&self, _: ParameterId) -> Option<ParameterDescriptor> {
            Some(descriptor())
        }
        fn supports_parameter_automation(&self, _: ParameterId) -> bool {
            true
        }
        fn runtime_version(&self) -> u32 {
            1
        }
        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<Runtime>()
        }
        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            Box::new(Runtime(self.0.fetch_add(1, Ordering::Relaxed) == 0))
        }
    }
    impl NativeProcessorRuntime for Runtime {
        fn supports_parameter_automation(&self, _: ParameterId) -> bool {
            self.0
        }
        fn process_block(&mut self, _: &mut [StereoFrame], _: &dyn DeviceResources) {}
        fn reset(&mut self) {}
        fn can_copy_history_from(&self, _: &dyn NativeProcessorRuntime) -> bool {
            true
        }
        fn copy_history_from(&mut self, _: &dyn NativeProcessorRuntime) {}
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    let prepared = PreparedRuntimeInstance {
        automation: program(descriptor(), semantics()),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 2,
        device_type: DeviceTypeId::from_static("test.parameter-capability"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(PreparedProcessorFactory::Legacy(Arc::new(Factory(
            AtomicUsize::new(0),
        )))),
    };
    assert!(matches!(
        NativeRuntimeChain::new_prepared(vec![prepared]),
        Err(DeviceContractError::UnsupportedProcessingClass)
    ));
}
