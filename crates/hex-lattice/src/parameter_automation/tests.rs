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

fn host_program() -> PreparedParameterAutomation {
    PreparedParameterAutomation::new_with_host(
        Vec::new(),
        vec![
            HostAutomationLane {
                target: DeviceAutomationTarget::HostInputGain,
                points: 0..2,
            },
            HostAutomationLane {
                target: DeviceAutomationTarget::HostOutputGain,
                points: 2..3,
            },
        ],
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
            ParameterAutomationPoint {
                beat_raw: 0,
                normalized: 0.5,
                step: true,
            },
        ],
    )
    .unwrap()
}
#[test]
fn host_lanes_are_not_provider_ids_and_use_content_clock_and_exact_physical_storage() {
    let program = host_program();
    assert!(!program.is_empty());
    assert!(program.parameter_ids().is_empty());
    let block = program.block(Some(&Clock), 32);
    for (offset, db) in [(0, -60.0), (10, -60.0), (15, -18.0), (20, 24.0), (31, 24.0)] {
        assert_eq!(
            block.host_gain_at(DeviceAutomationTarget::HostInputGain, offset),
            Some(10.0_f32.powf(db / 20.0))
        );
    }
    assert_eq!(
        block.host_gain_at(DeviceAutomationTarget::HostOutputGain, 15),
        Some(10.0_f32.powf(-18.0 / 20.0))
    );
    assert_eq!(
        block.host_gain_at(DeviceAutomationTarget::HostInputGain, 32),
        None
    );
    assert_eq!(
        program
            .block(None, 32)
            .host_gain_at(DeviceAutomationTarget::HostInputGain, 15),
        None
    );
    assert_eq!(
        program.storage_bytes(),
        PreparedParameterAutomation::quoted_storage_bytes_with_host(0, 2, 3)
    );
    assert!(
        PreparedParameterAutomation::new_with_host(
            Vec::new(),
            vec![HostAutomationLane {
                target: DeviceAutomationTarget::Parameter(ParameterId::from_static(1)),
                points: 0..1
            }],
            vec![ParameterAutomationPoint {
                beat_raw: 0,
                normalized: 0.5,
                step: false
            }]
        )
        .is_err()
    );
}
fn host_chain(bypassed: bool) -> crate::NativeRuntimeChain {
    use crate::*;
    NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        automation: host_program(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: DeviceTypeId::from_static("test.host"),
        matrix: StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap(),
        bypassed,
        sample_rate: 48_000,
        configuration: Some(std::sync::Arc::new(
            NativeDeviceState::new(DeviceTypeId::from_static("test.host"), 1, vec![])
                .unwrap()
                .with_input_gain_db(-6.0)
                .unwrap()
                .with_output_gain_db(3.0)
                .unwrap(),
        )),
        definition: None,
    }])
    .unwrap()
}
#[test]
fn all_chain_clock_entrypoints_keep_unavailable_host_stage_origins_independent() {
    use crate::{ForeignProcessors, NativeInstrumentClock, RefuseForeign};
    let clock: &dyn NativeInstrumentClock = &Clock;
    let sampled_input = HostGain::from_db(-60.0).unwrap().linear();
    let sampled_output = HostGain::from_normalized(0.5).unwrap().linear();
    let base_input = HostGain::from_db(-6.0).unwrap().linear();
    let base_output = HostGain::from_db(3.0).unwrap().linear();
    for (input_clock, output_clock, gain) in [
        (Some(clock), None, sampled_input * base_output),
        (None, Some(clock), base_input * sampled_output),
    ] {
        for api in 0..5 {
            let mut chain = host_chain(false);
            chain.begin_block();
            let mut frame = [[1.0, 0.5]];
            let mut foreign = RefuseForeign;
            let foreign: &mut dyn ForeignProcessors = &mut foreign;
            match api {
                0 => chain.process_range_with_clock(0..1, &mut frame, input_clock, output_clock),
                1 => chain.process_range_observed_with_clock(
                    0..1,
                    &mut frame,
                    0,
                    input_clock,
                    output_clock,
                    &mut |_, _, _, _, _| {},
                ),
                2 => chain.process_range_tapped_with_clock(
                    0..1,
                    &mut frame,
                    0,
                    input_clock,
                    output_clock,
                    &mut |_, _, _, _, _, _| {},
                ),
                3 => chain.process_range_with_foreign(
                    0..1,
                    &mut frame,
                    0,
                    input_clock,
                    output_clock,
                    foreign,
                ),
                _ => chain.process_range_tapped_with_foreign(
                    0..1,
                    &mut frame,
                    0,
                    input_clock,
                    output_clock,
                    foreign,
                    &mut |_, _, _, _, _, _| {},
                ),
            }
            .unwrap();
            for (actual, expected) in frame[0].into_iter().zip([2.0 * gain, gain]) {
                assert!(
                    (actual - expected).abs() < 1e-7,
                    "stage origins crossed in API {api}"
                );
            }
        }
    }
}

#[test]
fn memoryless_host_chain_runs_without_provider_optin_and_freezes_only_committed_trim() {
    let mut chain = host_chain(false);
    assert!(chain.has_stateful_processors());
    chain.begin_block();
    let mut frame = [[1.0, 0.5]; 1];
    chain
        .process_range_with_clock(0..1, &mut frame, Some(&Clock), Some(&Clock))
        .unwrap();
    chain.commit_block();
    let committed = frame[0];
    chain.begin_block();
    let mut speculative = [[1.0, 0.5]; 32];
    chain
        .process_range_with_clock(0..1, &mut speculative, Some(&Clock), Some(&Clock))
        .unwrap();
    chain.rollback_block();
    chain.begin_output_tail();
    chain.begin_block();
    let mut tail = [[1.0, 0.5]; 32];
    chain
        .process_range_with_clock(0..1, &mut tail, Some(&Clock), Some(&Clock))
        .unwrap();
    assert!(tail.iter().all(|f| *f == committed));
    let mut absent = host_chain(false);
    absent.begin_block();
    let mut dry = [[1.0, 0.5]];
    absent.process_block(&mut dry).unwrap();
    assert_eq!(
        dry[0][0],
        2.0 * 10.0_f32.powf(-6.0 / 20.0) * 10.0_f32.powf(3.0 / 20.0)
    );
    let mut bypass = host_chain(true);
    bypass.begin_block();
    let mut dry = [[1.0, 0.5]; 32];
    bypass
        .process_range_with_clock(0..1, &mut dry, Some(&Clock), Some(&Clock))
        .unwrap();
    assert_eq!(dry, [[1.0, 0.5]; 32]);
}

#[test]
fn bypass_transition_preserves_automation_clock_across_internal_chunks() {
    let mut active = host_chain(false);
    let mut wet = [[1.0, 0.5]; 300];
    active.begin_block();
    active
        .process_range_with_clock(0..1, &mut wet, Some(&Clock), Some(&Clock))
        .unwrap();
    active.commit_block();

    let mut old = host_chain(false);
    let mut bypass = host_chain(true);
    assert!(bypass.configuration_matches_range(&old, 0..1));
    bypass.transfer_history_from(&mut old);
    let mut actual = [[1.0, 0.5]; 300];
    bypass.begin_block();
    bypass
        .process_range_with_clock(0..1, &mut actual, Some(&Clock), Some(&Clock))
        .unwrap();
    bypass.commit_block();
    for (index, (actual, wet)) in actual.iter().zip(wet).enumerate() {
        let mix = 1.0 - ((index + 1).min(240) as f32 / 240.0);
        for channel in 0..2 {
            let dry = if channel == 0 { 1.0 } else { 0.5 };
            let expected = dry + (wet[channel] - dry) * mix;
            assert!(
                (actual[channel] - expected).abs() < 1e-5,
                "frame {index} channel {channel}: {actual:?} expected {expected}"
            );
        }
    }
}

#[test]
fn host_gain_uses_exact_integer_law_and_rejects_malformed_prepared_points() {
    let target = DeviceAutomationTarget::HostInputGain;
    let make = |normalized| {
        PreparedParameterAutomation::new_with_host(
            Vec::new(),
            vec![HostAutomationLane {
                target,
                points: 0..1,
            }],
            vec![ParameterAutomationPoint {
                beat_raw: 0,
                normalized,
                step: false,
            }],
        )
    };
    for millidb in [-60_000, -6_000, 0, 3_000, 24_000] {
        let gain = HostGain::from_millidb(millidb).unwrap();
        let program = make(gain.normalized()).unwrap();
        assert_eq!(
            program.block(Some(&Clock), 1).host_gain_at(target, 0),
            Some(gain.linear())
        );
    }
    assert_eq!(
        make(HostGain::ZERO.normalized())
            .unwrap()
            .block(Some(&Clock), 1)
            .host_gain_at(target, 0),
        Some(1.0)
    );
    for value in [f32::NAN, f32::INFINITY, -0.001, 1.001] {
        assert!(make(value).is_err());
    }
}

#[test]
fn mixed_cached_host_and_provider_rows_keep_bulk_provider_only_and_quote_all_staging() {
    let program = PreparedParameterAutomation::new_with_host(
        vec![ParameterAutomationLane {
            descriptor: descriptor(),
            semantics: semantics(),
            points: 0..2,
        }],
        vec![
            HostAutomationLane {
                target: DeviceAutomationTarget::HostInputGain,
                points: 2..3,
            },
            HostAutomationLane {
                target: DeviceAutomationTarget::HostOutputGain,
                points: 3..5,
            },
        ],
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
            ParameterAutomationPoint {
                beat_raw: 0,
                normalized: HostGain::ZERO.normalized(),
                step: true,
            },
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
    .unwrap();
    assert_eq!(program.parameter_ids(), &[descriptor().id]);
    let block = program.block(Some(&Clock), 32);
    let mut provider = [None];
    block.values_at(15, &mut provider).unwrap();
    assert_eq!(provider, [Some(ParameterValue::Float(5.0))]);
    assert_eq!(
        block.host_gain_at(DeviceAutomationTarget::HostInputGain, 15),
        Some(1.0)
    );
    assert_eq!(
        block.host_gain_at(DeviceAutomationTarget::HostOutputGain, 15),
        Some(HostGain::from_normalized(0.5).unwrap().linear())
    );
    assert!(block.values_at(15, &mut [None; 3]).is_err());
    program
        .block(None, 32)
        .values_at(15, &mut provider)
        .unwrap();
    assert_eq!(provider, [None]);
    assert_eq!(
        program
            .block(None, 32)
            .host_gain_at(DeviceAutomationTarget::HostInputGain, 15),
        None
    );
    assert_eq!(
        program.storage_bytes(),
        PreparedParameterAutomation::quoted_storage_bytes_with_host(1, 2, 5)
    );
    assert_eq!(
        PreparedParameterAutomation::quoted_construction_overlap_bytes_with_host(1, 2, 5),
        Some(
            std::mem::size_of::<ParameterAutomationLane>()
                + 2 * std::mem::size_of::<HostAutomationLane>()
                + 5 * std::mem::size_of::<ParameterAutomationPoint>()
        )
    );
}

#[test]
fn transient_provider_union_preserves_curves_bulk_clear_and_physical_backing() {
    let mut prepared = host_program();
    let desc = descriptor();
    assert_eq!(
        prepared.provider_input_growth_overlap_bytes(desc.id),
        Some(2 * std::mem::size_of::<PreparedParameterAutomationLane>())
    );
    prepared.arm_provider_input(desc, semantics()).unwrap();
    assert_eq!(
        prepared.input_target(),
        Some(DeviceAutomationTarget::Parameter(desc.id))
    );
    assert_eq!(prepared.parameter_ids(), &[desc.id]);
    assert_eq!(
        prepared.provider_input_growth_overlap_bytes(desc.id),
        Some(0)
    );
    let (_, lanes, points) = prepared.storage_vectors();
    assert_eq!(lanes.len(), 3);
    assert_eq!(points.len(), 3, "arming adds no authored points");
    assert_eq!(
        prepared.storage_bytes(),
        PreparedParameterAutomation::quoted_storage_bytes_with_host(1, 2, 3)
    );
    let input = NativeTransientValue::Parameter {
        id: desc.id,
        value: ParameterValue::Float(7.0),
    };
    let block = prepared.block_with_input(None, 32, Some(input)).unwrap();
    let mut bulk = [None];
    block.values_at(15, &mut bulk).unwrap();
    assert_eq!(bulk, [Some(ParameterValue::Float(7.0))]);
    assert_eq!(block.value_at(desc.id, 15), bulk[0]);
    assert!(block.values_at(32, &mut bulk).is_err());
    assert!(block.values_at(0, &mut []).is_err());
    let cleared = prepared.block_with_input(Some(&Clock), 32, None).unwrap();
    cleared.values_at(15, &mut bulk).unwrap();
    assert_eq!(
        bulk,
        [None],
        "clear keeps the armed ID and requests immutable base"
    );
    assert_eq!(
        cleared.host_gain_at(DeviceAutomationTarget::HostInputGain, 15),
        Some(HostGain::from_db(-18.0).unwrap().linear()),
        "inserting a provider fallback preserves host indices"
    );
    assert_eq!(
        cleared.host_gain_at(DeviceAutomationTarget::HostOutputGain, 15),
        Some(HostGain::from_db(-18.0).unwrap().linear())
    );
    let previous = prepared.clone();
    assert!(
        prepared
            .arm_host_gain_input(DeviceAutomationTarget::HostOutputGain)
            .is_err()
    );
    assert_eq!(prepared, previous);
    for value in [
        ParameterValue::Float(f32::NAN),
        ParameterValue::Float(99.0),
        ParameterValue::Bool(true),
    ] {
        assert!(
            prepared
                .block_with_input(
                    None,
                    1,
                    Some(NativeTransientValue::Parameter { id: desc.id, value })
                )
                .is_err()
        );
    }
    let mut readonly = PreparedParameterAutomation::default();
    assert!(
        readonly
            .arm_provider_input(
                desc,
                ParameterSemantics {
                    access: ParameterAccess::ReadOnly,
                    ..semantics()
                }
            )
            .is_err()
    );
    assert_eq!(readonly, PreparedParameterAutomation::default());
}

#[test]
fn transient_boolean_choice_and_other_provider_lanes_restore_the_current_curve() {
    for (kind, default, value, stepping) in [
        (
            ParameterKind::Bool,
            ParameterValue::Bool(false),
            ParameterValue::Bool(true),
            ParameterStepping::Boolean,
        ),
        (
            ParameterKind::Choice {
                labels: &["one", "two", "three"],
            },
            ParameterValue::Choice(0),
            ParameterValue::Choice(1),
            ParameterStepping::Choice,
        ),
    ] {
        let desc = ParameterDescriptor {
            id: ParameterId::from_static(2),
            kind,
            default,
            ..descriptor()
        };
        let discrete = ParameterSemantics {
            normalization: ParameterNormalization::Discrete,
            stepping,
            ..semantics()
        };
        let mut prepared = PreparedParameterAutomation::new(
            vec![
                ParameterAutomationLane {
                    descriptor: descriptor(),
                    semantics: semantics(),
                    points: 0..2,
                },
                ParameterAutomationLane {
                    descriptor: desc,
                    semantics: discrete,
                    points: 2..4,
                },
            ],
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
        .unwrap();
        prepared.arm_provider_input(desc, discrete).unwrap();
        assert_eq!(prepared.storage_vectors().2.len(), 4);
        let block = prepared
            .block_with_input(
                Some(&Clock),
                32,
                Some(NativeTransientValue::Parameter { id: desc.id, value }),
            )
            .unwrap();
        let shifted = crate::OffsetAutomation {
            values: &block,
            offset: 7,
        };
        let mut values = [None; 2];
        shifted.values_at(8, &mut values).unwrap();
        assert_eq!(values, [Some(ParameterValue::Float(5.0)), Some(value)]);
        for (index, &id) in shifted.parameter_ids().iter().enumerate() {
            assert_eq!(shifted.value_at(id, 8), values[index]);
        }
        let cleared = prepared.block_with_input(Some(&Clock), 32, None).unwrap();
        cleared.values_at(15, &mut values).unwrap();
        assert_eq!(values, [Some(ParameterValue::Float(5.0)), Some(default)]);
        assert_eq!(
            prepared
                .block_with_input(Some(&Clock), 32, None)
                .unwrap()
                .value_at(desc.id, 20),
            Some(match kind {
                ParameterKind::Bool => ParameterValue::Bool(true),
                _ => ParameterValue::Choice(2),
            })
        );
    }
}

fn process_transient_host(
    chain: &mut crate::NativeRuntimeChain,
    frames: &mut [crate::StereoFrame],
    transient: Option<&mut crate::NativeTransientProcessing>,
    offset: usize,
) -> Result<(), Error> {
    chain.process_range_tapped_with_foreign_and_input(
        0..1,
        frames,
        offset,
        None,
        None,
        &mut crate::RefuseForeign,
        transient,
        &mut |_, _, _, _, _, _| {},
    )
}
#[test]
fn transient_host_stages_only_successful_current_processing_and_clear_restores_base() {
    use crate::{
        NativeTransientInput, NativeTransientProcessing, ProcessorChainOwner,
        ProcessorInstanceIdentity,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 1).unwrap();
    let value = NativeTransientValue::HostOutputGain(HostGain::ZERO);
    let mut context = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation: NativeTransientOperation::Set(value),
    });
    let mut chain = host_chain(false);
    chain.entries[0]
        .automation
        .arm_host_gain_input(value.target())
        .unwrap();
    chain.begin_block();
    let mut frame = [[1.0, 0.5]];
    process_transient_host(&mut chain, &mut frame, Some(&mut context), 9).unwrap();
    let input = HostGain::from_db(-6.0).unwrap().linear();
    assert_eq!(frame, [[2.0 * input, input]]);
    assert_eq!(
        context.applied,
        Some(crate::NativeTransientApplication {
            identity,
            operation: NativeTransientOperation::Set(value),
            offset: 9
        })
    );
    chain.commit_block();
    chain.begin_block();
    let mut cleared = [[1.0, 0.5]];
    context = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation: NativeTransientOperation::Clear(value.target()),
    });
    process_transient_host(&mut chain, &mut cleared, Some(&mut context), 0).unwrap();
    assert_eq!(
        context.applied,
        Some(crate::NativeTransientApplication {
            identity,
            operation: NativeTransientOperation::Clear(value.target()),
            offset: 0
        })
    );
    let base = input * HostGain::from_db(3.0).unwrap().linear();
    assert_eq!(cleared, [[2.0 * base, base]]);
    chain.rollback_block();
    chain.begin_output_tail();
    context.applied = None;
    let mut tail = [[1.0, 0.5]];
    process_transient_host(&mut chain, &mut tail, Some(&mut context), 0).unwrap();
    assert_eq!(tail, frame, "retained tail uses committed effective trims");
    assert_eq!(context.applied, None);
    let mut bypass = host_chain(true);
    bypass.entries[0]
        .automation
        .arm_host_gain_input(value.target())
        .unwrap();
    bypass.begin_block();
    let mut dry = [[1.0, 0.5]];
    process_transient_host(&mut bypass, &mut dry, Some(&mut context), 0).unwrap();
    assert_eq!(dry, [[1.0, 0.5]]);
    assert_eq!(context.applied, None);
    let mut empty = [];
    process_transient_host(&mut bypass, &mut empty, Some(&mut context), 0).unwrap();
    assert_eq!(context.applied, None);
    let bad = NativeTransientValue::HostInputGain(HostGain::ZERO);
    context.input.operation = NativeTransientOperation::Set(bad);
    let before = dry;
    assert!(process_transient_host(&mut bypass, &mut dry, Some(&mut context), 0).is_err());
    assert_eq!(dry, before);
    assert_eq!(context.applied, None);
}

#[test]
fn armed_provider_clear_is_processed_and_late_refusal_discards_observation() {
    use crate::*;
    use std::sync::Arc;
    struct Factory(bool);
    #[derive(Clone, Copy)]
    struct Runtime {
        effective: f32,
        fail_loud: bool,
    }
    impl PreparedNativeProcessor for Factory {
        fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
            (id == descriptor().id).then_some(descriptor())
        }
        fn supports_parameter_automation(&self, id: ParameterId) -> bool {
            id == descriptor().id
        }
        fn runtime_version(&self) -> u32 {
            1
        }
        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<Runtime>()
        }
        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            Box::new(Runtime {
                effective: 2.0,
                fail_loud: self.0,
            })
        }
    }
    impl NativeProcessorRuntime for Runtime {
        fn supports_parameter_automation(&self, id: ParameterId) -> bool {
            id == descriptor().id
        }
        fn process_block_with_automation(
            &mut self,
            frames: &mut [StereoFrame],
            _: Option<&dyn NativeInstrumentClock>,
            values: &dyn NativeParameterAutomation,
            resources: &dyn DeviceResources,
        ) -> Result<(), DeviceContractError> {
            for (offset, frame) in frames.iter_mut().enumerate() {
                let mut sampled = [None];
                values.values_at(offset, &mut sampled)?;
                self.effective = match sampled[0] {
                    Some(ParameterValue::Float(value)) => value,
                    None => 2.0,
                    _ => return Err(DeviceContractError::InvalidParameterValue),
                };
                if self.fail_loud && frame[0] > 10.0 {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                self.process_block(std::slice::from_mut(frame), resources);
            }
            Ok(())
        }
        fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn DeviceResources) {
            for frame in frames {
                for sample in frame {
                    *sample *= self.effective;
                }
            }
        }
        fn reset(&mut self) {
            self.effective = 2.0;
        }
        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }
        fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
            *self = *previous.as_any().downcast_ref::<Self>().unwrap();
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    let mut armed = PreparedParameterAutomation::default();
    armed.arm_provider_input(descriptor(), semantics()).unwrap();
    let make = |device_id, automation, fail_loud| PreparedRuntimeInstance {
        automation,
        meter: Default::default(),
        owner: ProcessorChainOwner::Main,
        device_id,
        device_type: DeviceTypeId::from_static("test.transient"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(PreparedProcessorFactory::Legacy(Arc::new(Factory(
            fail_loud,
        )))),
    };
    let mut chain = NativeRuntimeChain::new_prepared(vec![
        make(1, armed, false),
        make(2, program(descriptor(), semantics()), true),
    ])
    .unwrap();
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 1).unwrap();
    let mut context = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation: NativeTransientOperation::Set(NativeTransientValue::Parameter {
            id: descriptor().id,
            value: ParameterValue::Float(5.0),
        }),
    });
    let process = |chain: &mut NativeRuntimeChain,
                   frames: &mut [StereoFrame],
                   context: Option<&mut NativeTransientProcessing>| {
        chain.process_range_tapped_with_foreign_and_input(
            0..2,
            frames,
            4,
            None,
            None,
            &mut RefuseForeign,
            context,
            &mut |_, _, _, _, _, _| {},
        )
    };
    chain.begin_block();
    let mut loud = [[5.0; 2]];
    assert!(process(&mut chain, &mut loud, Some(&mut context)).is_err());
    assert_eq!(
        context.applied, None,
        "later entry refusal discards earlier speculative application"
    );
    chain.rollback_block();
    chain.begin_block();
    let mut frame = [[1.0; 2]];
    process(&mut chain, &mut frame, Some(&mut context)).unwrap();
    assert_eq!(frame, [[10.0; 2]]);
    assert_eq!(context.applied.unwrap().offset, 4);
    chain.commit_block();
    chain.begin_block();
    let clear = NativeTransientOperation::Clear(DeviceAutomationTarget::Parameter(descriptor().id));
    context = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation: clear,
    });
    let mut restored = [[1.0; 2]];
    process(&mut chain, &mut restored, Some(&mut context)).unwrap();
    assert_eq!(restored, [[4.0; 2]]);
    assert_eq!(
        context.applied,
        Some(NativeTransientApplication {
            identity,
            operation: clear,
            offset: 4
        })
    );
    chain.commit_block();
    chain.begin_block();
    let mut unmonitored = [[1.0; 2]];
    process(&mut chain, &mut unmonitored, None).unwrap();
    assert_eq!(
        unmonitored, restored,
        "fallback remains delivered after the terminal monitor leaves"
    );
}

struct External<'a> {
    targets: &'a [DeviceAutomationTarget],
    values: &'a [Option<NormalizedAutomationValue>],
}
impl ExternalParameterAutomation for External<'_> {
    fn frames(&self) -> usize {
        self.values.len()
    }
    fn targets(&self) -> &[DeviceAutomationTarget] {
        self.targets
    }
    fn value_at(
        &self,
        target: DeviceAutomationTarget,
        offset: usize,
    ) -> Option<NormalizedAutomationValue> {
        if self.targets.contains(&target) {
            self.values.get(offset).copied().flatten()
        } else {
            None
        }
    }
}
#[test]
fn external_samples_share_mapping_and_live_authored_base_precedence() {
    let id = descriptor().id;
    let target = DeviceAutomationTarget::Parameter(id);
    let values = [Some(NormalizedAutomationValue::new(0.5).unwrap()), None];
    let external = External {
        targets: &[target],
        values: &values,
    };
    let mut prepared = program(
        descriptor(),
        ParameterSemantics {
            normalization: ParameterNormalization::PositiveLogarithmic,
            ..semantics()
        },
    );
    let block = prepared
        .block_with_external(Some(&Clock), 2, None, &external)
        .unwrap();
    assert_eq!(block.value_at(id, 0), Some(ParameterValue::Float(3.0)));
    assert_eq!(block.value_at(id, 1), Some(ParameterValue::Float(1.0)));
    let mut bulk = [None];
    block.values_at(0, &mut bulk).unwrap();
    assert_eq!(bulk[0], block.value_at(id, 0));
    prepared
        .arm_provider_input(
            descriptor(),
            ParameterSemantics {
                normalization: ParameterNormalization::PositiveLogarithmic,
                ..semantics()
            },
        )
        .unwrap();
    let live = NativeTransientValue::Parameter {
        id,
        value: ParameterValue::Float(7.0),
    };
    assert_eq!(
        prepared
            .block_with_external(None, 2, Some(live), &external)
            .unwrap()
            .value_at(id, 0),
        Some(ParameterValue::Float(7.0))
    );
    let clear = prepared
        .block_with_external(None, 2, None, &external)
        .unwrap();
    assert_eq!(clear.value_at(id, 0), Some(ParameterValue::Float(3.0)));
    assert_eq!(clear.value_at(id, 1), None);
    assert!(NormalizedAutomationValue::new(f32::NAN).is_err());
    assert!(NormalizedAutomationValue::new(f32::INFINITY).is_err());
    assert!(NormalizedAutomationValue::new(-0.01).is_err());
    assert!(NormalizedAutomationValue::new(1.01).is_err());
    assert!(
        prepared
            .block_with_external(None, 3, None, &external)
            .is_err()
    );
}
#[test]
fn external_only_provider_and_host_slots_do_not_arm_live_input() {
    let mut prepared = PreparedParameterAutomation::new(Vec::new(), Vec::new()).unwrap();
    prepared
        .prepare_external_host(DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    prepared
        .prepare_external_provider(descriptor(), semantics())
        .unwrap();
    prepared
        .prepare_external_host(DeviceAutomationTarget::HostInputGain)
        .unwrap();
    assert_eq!(prepared.input_target(), None);
    let targets = [
        DeviceAutomationTarget::Parameter(descriptor().id),
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ];
    // Target declaration follows the actual enum's stable order.
    let mut targets = targets;
    targets.sort_unstable();
    let values = [Some(NormalizedAutomationValue::new(0.0).unwrap()), None];
    let external = External {
        targets: &targets,
        values: &values,
    };
    let block = prepared
        .block_with_external(None, 2, None, &external)
        .unwrap();
    assert_eq!(
        block.value_at(descriptor().id, 0),
        Some(ParameterValue::Float(1.0))
    );
    assert_eq!(
        block.host_gain_at(DeviceAutomationTarget::HostInputGain, 0),
        Some(HostGain::from_db(-60.0).unwrap().linear())
    );
    assert_eq!(
        block.host_gain_at(DeviceAutomationTarget::HostOutputGain, 0),
        Some(HostGain::from_db(-60.0).unwrap().linear())
    );
    assert_eq!(
        block.host_gain_at(DeviceAutomationTarget::HostInputGain, 1),
        None
    );
    let bad = External {
        targets: &[targets[0], targets[0]],
        values: &values,
    };
    assert!(prepared.block_with_external(None, 2, None, &bad).is_err());
}
