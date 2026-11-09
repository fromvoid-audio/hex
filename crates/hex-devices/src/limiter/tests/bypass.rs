//! Host admission, accounting and real detector/lookahead histories agree.
use super::*;
use hex_lattice::{NativeRuntimeChain, PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix};

fn chain(
    rate: u32,
    changes: &[(ParameterId, ParameterValue)],
    bypassed: bool,
) -> NativeRuntimeChain {
    let state = state(changes).with_output_gain_db(6.0).unwrap();
    let provider = factory(rate, changes);
    let latency = provider.algorithmic_latency_frames().unwrap();
    assert_eq!(provider.reserved_bypass_latency_frames(), Some(latency));
    assert!(!provider.supports_parameter_automation(GAIN));
    let definition = PreparedProcessorFactory::Owned(provider.clone());
    assert_eq!(
        definition.declared_pair_bytes(),
        Some(
            provider.declared_pair_bytes()
                + 2 * latency as usize * std::mem::size_of::<StereoFrame>()
        )
    );
    let instances = vec![PreparedRuntimeInstance {
        modulation: None,
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: state.type_id(),
        matrix: StereoMatrix::identity(),
        bypassed,
        sample_rate: rate,
        configuration: Some(Arc::new(state)),
        definition: Some(definition),
    }];
    let quote =
        NativeRuntimeChain::declared_storage_preflight_bytes_for_prepared(&instances).unwrap();
    let chain = NativeRuntimeChain::new_prepared(instances).unwrap();
    assert_eq!(chain.accounted_storage_bytes(), Some(quote));
    assert_eq!(
        chain.algorithmic_latency_frames_for_entry(0).unwrap(),
        Some(latency)
    );
    chain
}

fn process(
    chain: &mut NativeRuntimeChain,
    input: &[StereoFrame],
    block: usize,
) -> Vec<StereoFrame> {
    let mut output = input.to_vec();
    for part in output.chunks_mut(block) {
        chain.begin_block();
        chain.process_block(part).unwrap();
        chain.commit_block();
    }
    output
}

#[test]
fn every_configuration_has_exact_reserved_storage_and_bypass_latency() {
    for rate in [8000, 44100, 48000, 96000, 192000] {
        for lookahead in 0..3 {
            for routing in 0..2 {
                for mode in 0..3 {
                    let changes = [
                        (LOOKAHEAD, ParameterValue::Choice(lookahead)),
                        (ROUTING, ParameterValue::Choice(routing)),
                        (MODE, ParameterValue::Choice(mode)),
                    ];
                    let wet = chain(rate, &changes, false);
                    let mut dry = chain(rate, &changes, true);
                    let latency = wet
                        .algorithmic_latency_frames_for_entry(0)
                        .unwrap()
                        .unwrap() as usize;
                    let input: Vec<_> = (0..latency + 31)
                        .map(|i| [i as f32 * 0.001, -(i as f32) * 0.002])
                        .collect();
                    let output = process(&mut dry, &input, 7);
                    assert!(output[..latency].iter().all(|f| *f == [0.0; 2]));
                    assert_eq!(&output[latency..], &input[..31]);
                }
            }
        }
    }
}

#[test]
fn live_bypass_reversal_and_rollback_preserve_the_uninterrupted_detector_history() {
    for lookahead in 0..3 {
        for routing in 0..2 {
            for mode in 0..3 {
                check_mix(48000, lookahead, routing, mode);
            }
        }
    }
    for rate in [8000, 44100, 192000] {
        check_mix(rate, 2, 1, 2);
    }
}

fn check_mix(rate: u32, lookahead: u32, routing: u32, mode: u32) {
    let changes = [
        (LOOKAHEAD, ParameterValue::Choice(lookahead)),
        (ROUTING, ParameterValue::Choice(routing)),
        (MODE, ParameterValue::Choice(mode)),
        (GAIN, ParameterValue::Float(12.0)),
    ];
    let latency = factory(rate, &changes)
        .algorithmic_latency_frames()
        .unwrap() as usize;
    let warm = latency * 2 + 64;
    let ramp = rate.div_ceil(200) as usize;
    let settled = ramp + 60;
    let count = 18 + 3 * settled;
    let input: Vec<_> = (0..warm + count)
        .map(|i| {
            [
                (i as f32 * 0.031).sin() * 0.6,
                (i as f32 * 0.023).cos() * -0.4,
            ]
        })
        .collect();
    let mut reference = chain(rate, &changes, false);
    let wet = process(&mut reference, &input, 64);
    for block in [1, 7, 64, 512] {
        let mut old = chain(rate, &changes, false);
        process(&mut old, &input[..warm], 64);
        let mut offset = 0;
        for (bypassed, length) in [
            (true, 13),
            (true, 5),
            (false, settled),
            (true, settled),
            (false, settled),
        ] {
            let mut next = chain(rate, &changes, bypassed);
            assert!(next.configuration_matches_range(&old, 0..1));
            assert!(next.can_inherit_from(&old));
            next.transfer_history_from(&mut old);
            let start = warm + offset;
            let mut discarded = input[start..start + 7].to_vec();
            next.begin_block();
            next.process_block(&mut discarded).unwrap();
            next.rollback_block();
            let actual = process(&mut next, &input[start..start + length], block);
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
                let dry = input[warm + i - latency];
                for lane in 0..2 {
                    let expected = dry[lane] + (wet[warm + i][lane] - dry[lane]) * mix;
                    assert!(
                        (frame[lane] - expected).abs() < 3e-6,
                        "{rate}/{lookahead}/{routing}/{mode} block={block} frame={i}: {frame:?} != {expected}"
                    );
                }
            }
            offset += length;
            old = next;
        }
    }
}
