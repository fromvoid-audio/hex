//! The real Utility filter owner keeps one history through reversible host bypass.
use super::*;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeRuntimeChain, PreparedRuntimeInstance,
    ProcessorChainOwner,
};
use std::sync::Arc;

fn state(dc: bool, bass: bool, muted: bool) -> NativeDeviceState {
    let mut state = UTILITY_DEFINITION
        .default_state()
        .unwrap()
        .with_output_gain_db(6.0)
        .unwrap();
    let registry = crate::native_registry().unwrap();
    for (id, value) in [
        (parameter::DC_REMOVE, dc),
        (parameter::BASS_MONO, bass),
        (parameter::MUTE, muted),
    ] {
        state = registry
            .patch_parameter(&state, id, ParameterValue::Bool(value))
            .unwrap();
    }
    assert_eq!(state.output_gain_db(), 6.0);
    state
}
fn chain(state: &NativeDeviceState, bypassed: bool, rate: u32) -> NativeRuntimeChain {
    let registry = crate::native_registry().unwrap();
    let prepared = registry
        .prepare_runtime_device(
            state,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    assert!(
        prepared.processor.is_some(),
        "exercise the stateful provider"
    );
    NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        modulation: None,
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: state.type_id(),
        matrix: prepared.matrix,
        bypassed,
        sample_rate: rate,
        configuration: Some(Arc::new(state.clone())),
        definition: prepared.processor,
    }])
    .unwrap()
}
fn render(chain: &mut NativeRuntimeChain, count: usize, block: usize) -> Vec<StereoFrame> {
    let mut output = vec![[0.25, -0.125]; count];
    for part in output.chunks_mut(block) {
        chain.begin_block();
        chain.process_block(part).unwrap();
        chain.commit_block();
    }
    output
}

#[test]
fn utility_filter_bypass_pcm_history_reversal_and_rollback_match_uninterrupted_reference() {
    for (dc, bass, muted) in [
        (true, false, false),
        (false, true, false),
        (true, true, false),
        (true, true, true),
    ] {
        let state = state(dc, bass, muted);
        for rate in [8_000_u32, 44_100, 48_000, 192_000] {
            let ramp = rate.div_ceil(200) as usize;
            for block in [1, 7, 64, 256] {
                let mut previous = chain(&state, false, rate);
                render(&mut previous, 1024, 64);
                let mut wet_reference = chain(&state, false, rate);
                wet_reference.inherit_from(&previous);
                let mut off_frames = 0;
                let mut reverse_from = 0.0;
                for (phase, (bypassed, count)) in [
                    (true, 13),
                    (true, 5),
                    (false, ramp + 60),
                    (true, ramp + 60),
                    (false, ramp + 60),
                ]
                .into_iter()
                .enumerate()
                {
                    let mut next = chain(&state, bypassed, rate);
                    assert!(next.configuration_matches_range(&previous, 0..1));
                    assert!(next.can_inherit_from(&previous));
                    next.transfer_history_from(&mut previous);
                    if phase == 2 {
                        // Speculative DSP and ramp advancement must both roll back.
                        next.begin_block();
                        next.process_block(&mut [[0.25, -0.125]; 7]).unwrap();
                        next.rollback_block();
                    }
                    let actual = render(&mut next, count, block);
                    let wet = render(&mut wet_reference, count, 127);
                    for (i, (actual, wet)) in actual.iter().zip(wet).enumerate() {
                        let progress = (i + 1).min(ramp) as f32 / ramp as f32;
                        let mix = match phase {
                            0 | 1 => 1.0 - (off_frames + i + 1) as f32 / ramp as f32,
                            2 => reverse_from + (1.0 - reverse_from) * progress,
                            3 => 1.0 - progress,
                            _ => progress,
                        };
                        for (lane, dry) in [0.25, -0.125].into_iter().enumerate() {
                            let expected = dry + (wet[lane] - dry) * mix;
                            assert!(
                                (actual[lane] - expected).abs() < 1e-6,
                                "dc={dc} bass={bass} mute={muted} rate={rate} block={block} phase={phase} frame={i}: {actual:?} expected={expected}"
                            );
                        }
                    }
                    if phase < 2 {
                        off_frames += count;
                        reverse_from = 1.0 - off_frames as f32 / ramp as f32;
                    }
                    previous = next;
                }
            }
        }
    }
}

#[test]
#[ignore = "manual optimized Utility filter host profile; no physical deadline acceptance"]
fn utility_filter_bypass_hot_path_profile() {
    use std::{hint::black_box, time::Instant};
    println!("optimized={}", !cfg!(debug_assertions));
    let state = state(true, true, false);
    let mut on = chain(&state, false, 48_000);
    let mut off = chain(&state, true, 48_000);
    println!("prepared pair bytes={}", on.declared_pair_bytes());
    let mut times = Vec::with_capacity(4096);
    let mut enabled = true;
    for iteration in 0_u32..4608 {
        let mut input = [[0.25, -0.125]; 128];
        let start = Instant::now();
        if iteration.is_multiple_of(16) {
            if enabled {
                off.transfer_history_from(&mut on);
            } else {
                on.transfer_history_from(&mut off);
            }
            enabled = !enabled;
        }
        let chain = if enabled { &mut on } else { &mut off };
        chain.begin_block();
        chain.process_block(black_box(&mut input)).unwrap();
        chain.commit_block();
        let elapsed = start.elapsed().as_nanos();
        black_box(input);
        if iteration >= 512 {
            times.push(elapsed);
        }
    }
    times.sort_unstable();
    println!(
        "utility DC+bass mono,48k/128,+6dB host gain,512 warmup+4096 measured,bypass every16: mean={}ns p50={}ns p99={}ns max={}ns",
        times.iter().sum::<u128>() / times.len() as u128,
        times[times.len() / 2],
        times[times.len() * 99 / 100],
        times[times.len() - 1]
    );
}
