//! Compare the host mix against an uninterrupted real provider and a separately
//! indexed dry delay. No provider runtime or host ramp is used as the mix oracle.
use super::*;
use hex_lattice::{
    NativeInstrumentClock, ParameterAutomationLane, ParameterAutomationPoint,
    PreparedParameterAutomation, ProcessorInstanceIdentity,
};
use hex_lattice_contract::ParameterId;

fn provider_chain(
    definition: &'static dyn NativeDeviceDefinition,
    bypassed: bool,
    rate: u32,
    automated: bool,
) -> NativeRuntimeChain {
    let state = definition
        .default_state()
        .unwrap()
        .with_output_gain_db(6.0)
        .unwrap();
    let definitions = [definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let parameters = if automated {
        vec![ParameterId::from_static(1)]
    } else {
        vec![]
    };
    let prepared = registry
        .prepare_runtime_instance_with_automation(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 1).unwrap(),
            &state,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
            &parameters,
        )
        .unwrap();
    let automation = if automated {
        let descriptor = definition
            .descriptors()
            .iter()
            .find(|d| d.id == parameters[0])
            .copied()
            .unwrap();
        PreparedParameterAutomation::new(
            vec![ParameterAutomationLane {
                descriptor,
                semantics: hex_lattice_contract::validated_parameter_semantics(
                    definition,
                    &state,
                    parameters[0],
                )
                .unwrap()
                .unwrap(),
                points: 0..3,
            }],
            vec![
                ParameterAutomationPoint {
                    beat_raw: 0,
                    normalized: 0.1,
                    step: false,
                },
                ParameterAutomationPoint {
                    beat_raw: 4109,
                    normalized: 0.9,
                    step: false,
                },
                ParameterAutomationPoint {
                    beat_raw: 4318,
                    normalized: 0.25,
                    step: false,
                },
            ],
        )
        .unwrap()
    } else {
        PreparedParameterAutomation::default()
    };
    NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        modulation: None,
        automation,
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: state.type_id(),
        matrix: prepared.matrix,
        bypassed,
        sample_rate: rate,
        configuration: Some(Arc::new(state)),
        definition: prepared.processor,
    }])
    .unwrap()
}

struct Clock(usize);
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        Some((self.0 + offset) as u64)
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.beat_at_offset(offset)
    }
}

fn signal(frame: usize) -> [f32; 2] {
    [
        (frame as f32 * 0.031).sin() * 0.2,
        (frame as f32 * 0.023).cos() * -0.15,
    ]
}

fn process(
    chain: &mut NativeRuntimeChain,
    input: &[[f32; 2]],
    start: usize,
    block: usize,
) -> Vec<[f32; 2]> {
    let mut output = input.to_vec();
    for (index, part) in output.chunks_mut(block).enumerate() {
        let clock = Clock(start + index * block);
        chain.begin_block();
        chain
            .process_range_with_clock(0..1, part, Some(&clock), Some(&clock))
            .unwrap();
        chain.commit_block();
    }
    output
}

#[test]
fn real_provider_reversals_rollback_and_unchanged_handoffs_match_wet_and_delayed_dry() {
    for (definition, automated) in [
        (
            &crate::rattle::RATTLE_DEFINITION as &'static dyn NativeDeviceDefinition,
            false,
        ),
        (&crate::rattle::RATTLE_DEFINITION, true),
        (&crate::overdrive::OVERDRIVE_DEFINITION, false),
    ] {
        for rate in [8_000_u32, 44_100, 48_000, 192_000] {
            let ramp = rate.div_ceil(200) as usize;
            let settled = ramp + 60;
            let count = 18 + 3 * settled;
            let input: Vec<_> = (0..4096 + count).map(signal).collect();
            let mut reference = provider_chain(definition, false, rate, automated);
            let wet = process(&mut reference, &input, 0, 64);
            for block in [1, 7, 64, 513] {
                let mut old = provider_chain(definition, false, rate, automated);
                process(&mut old, &input[..4096], 0, 64);
                let mut offset = 0;
                for (bypassed, length) in [
                    (true, 13),
                    (true, 5),
                    (false, settled),
                    (true, settled),
                    (false, settled),
                ] {
                    let mut next = provider_chain(definition, bypassed, rate, automated);
                    assert!(next.configuration_matches_range(&old, 0..1));
                    assert!(next.can_inherit_from(&old));
                    next.transfer_history_from(&mut old);
                    let start = 4096 + offset;
                    let mut discarded = input[start..start + 7].to_vec();
                    next.begin_block();
                    next.process_range_with_clock(
                        0..1,
                        &mut discarded,
                        Some(&Clock(start)),
                        Some(&Clock(start)),
                    )
                    .unwrap();
                    next.rollback_block();
                    let actual = process(&mut next, &input[start..start + length], start, block);
                    for (local, frame) in actual.iter().enumerate() {
                        let i = offset + local;
                        let mix = if i < 18 {
                            1.0 - (i + 1) as f32 / ramp as f32
                        } else if i < 18 + settled {
                            let initial = 1.0 - 18.0 / ramp as f32;
                            initial + (1.0 - initial) * (i - 17).min(ramp) as f32 / ramp as f32
                        } else if i < 18 + 2 * settled {
                            1.0 - (i - 17 - settled).min(ramp) as f32 / ramp as f32
                        } else {
                            (i - 17 - 2 * settled).min(ramp) as f32 / ramp as f32
                        };
                        let dry = input[4096 + i - 11];
                        for lane in 0..2 {
                            let expected = dry[lane] + (wet[4096 + i][lane] - dry[lane]) * mix;
                            assert!(
                                (frame[lane] - expected).abs() < 2e-6,
                                "{:?} automated={automated} rate={rate} block={block} frame={i} actual={frame:?} expected={expected}",
                                definition.type_id().as_str()
                            );
                        }
                    }
                    offset += length;
                    old = next;
                }
            }
        }
    }
}

#[test]
fn warm_bypass_keeps_real_release_history_and_silence_stays_exactly_silent() {
    for definition in [
        &crate::rattle::RATTLE_DEFINITION as &'static dyn NativeDeviceDefinition,
        &crate::overdrive::OVERDRIVE_DEFINITION,
    ] {
        let mut silent = provider_chain(definition, true, 48_000, false);
        assert!(
            process(&mut silent, &[[0.0; 2]; 1024], 0, 64)
                .iter()
                .all(|f| *f == [0.0; 2])
        );
        let mut wet = provider_chain(definition, false, 48_000, false);
        let mut audible = provider_chain(definition, false, 48_000, false);
        let warm: Vec<_> = (0..1024).map(signal).collect();
        process(&mut wet, &warm, 0, 64);
        process(&mut audible, &warm, 0, 64);
        let mut off = provider_chain(definition, true, 48_000, false);
        off.transfer_history_from(&mut audible);
        let zeros = vec![[0.0; 2]; 192_512];
        let reference = process(&mut wet, &zeros, 1024, 256);
        assert!(reference[..240].iter().flatten().any(|v| v.abs() > 1e-5));
        let head = process(&mut off, &zeros[..300], 1024, 64);
        assert!(head[240..].iter().all(|f| *f == [0.0; 2]));
        let mut on = provider_chain(definition, false, 48_000, false);
        on.transfer_history_from(&mut off);
        let actual = process(&mut on, &zeros[300..], 1324, 256);
        for (i, (frame, expected)) in actual.iter().zip(&reference[300..]).enumerate() {
            let mix = (i + 1).min(240) as f32 / 240.0;
            for lane in 0..2 {
                assert!((frame[lane] - expected[lane] * mix).abs() < 2e-6);
            }
        }
        assert!(actual[192_000..].iter().all(|f| *f == [0.0; 2]));
        let wake: Vec<_> = (0..512).map(signal).collect();
        assert_eq!(
            process(&mut on, &wake, 193_536, 7),
            process(&mut wet, &wake, 193_536, 64)
        );
        let different_rate = provider_chain(definition, false, 44_100, false);
        assert!(!different_rate.configuration_matches_range(&on, 0..1));
    }
}
