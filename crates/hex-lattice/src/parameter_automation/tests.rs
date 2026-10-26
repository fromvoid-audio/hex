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
    for program in [&continuous, &stepped, &logarithmic, &choice, &boolean] {
        let block = program.block(Some(&Clock), 32);
        let shifted = crate::OffsetAutomation {
            values: &block,
            offset: 7,
        };
        for offset in [0, 3, 8, 13, 20, 2] {
            let mut bulk = [None];
            shifted.values_at(offset, &mut bulk).unwrap();
            assert_eq!(bulk[0], block.value_at(id, offset + 7));
        }
        let mut missing = [Some(ParameterValue::Bool(true))];
        program.block(None, 32).values_at(10, &mut missing).unwrap();
        assert_eq!(missing, [None]);
    }
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
                + points.capacity() * std::mem::size_of::<PreparedParameterAutomationPoint>()
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

#[test]
fn bulk_matches_scalar_with_one_clock_query_and_overwrites_every_missing_slot() {
    use std::cell::Cell;
    struct Count(Cell<usize>);
    impl NativeInstrumentClock for Count {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            Some(999)
        }
        fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
            self.0.set(self.0.get() + 1);
            Some(offset as u64)
        }
    }
    let mut points = Vec::new();
    let mut lanes = Vec::new();
    for i in 1..=3 {
        let start = points.len();
        points.extend([
            ParameterAutomationPoint {
                beat_raw: 10,
                normalized: 0.0,
                step: i == 2,
            },
            ParameterAutomationPoint {
                beat_raw: 20,
                normalized: if i == 3 { 0.0 } else { 1.0 },
                step: true,
            },
        ]);
        lanes.push(ParameterAutomationLane {
            descriptor: ParameterDescriptor {
                id: ParameterId::new(i).unwrap(),
                ..descriptor()
            },
            semantics: semantics(),
            points: start..points.len(),
        });
    }
    let program = PreparedParameterAutomation::new(lanes, points).unwrap();
    let clock = Count(Cell::new(0));
    let block = program.block(Some(&clock), 32);
    for offset in [0, 10, 15, 20, 31, 12, 0, 19] {
        let mut output = [Some(ParameterValue::Bool(true)); 3];
        let before = clock.0.get();
        block.values_at(offset, &mut output).unwrap();
        assert_eq!(clock.0.get(), before + 1);
        for (value, &id) in output.iter().zip(program.parameter_ids()) {
            assert_eq!(*value, block.value_at(id, offset));
        }
    }
    let mut missing = [Some(ParameterValue::Float(9.0)); 3];
    program.block(None, 32).values_at(0, &mut missing).unwrap();
    assert_eq!(missing, [None; 3]);
    assert!(block.values_at(32, &mut missing).is_err());
    assert!(block.values_at(0, &mut missing[..2]).is_err());
    assert_eq!(
        program.storage_bytes(),
        PreparedParameterAutomation::quoted_storage_bytes(3, 6)
    );
    assert_eq!(
        PreparedParameterAutomation::quoted_construction_overlap_bytes(3, 6),
        Some(
            3 * std::mem::size_of::<ParameterAutomationLane>()
                + 6 * std::mem::size_of::<ParameterAutomationPoint>()
        )
    );
}
