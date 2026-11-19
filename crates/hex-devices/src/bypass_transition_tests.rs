//! Live host transitions use the actual provider state, including bypass history.
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, NativeDeviceRegistry,
    NativeRuntimeChain, PreparedRuntimeInstance, ProcessorChainOwner,
};
use std::sync::Arc;

mod short_latency;

fn chain(
    definition: &'static dyn NativeDeviceDefinition,
    bypassed: bool,
    rate: u32,
) -> NativeRuntimeChain {
    let state = definition
        .default_state()
        .unwrap()
        .with_output_gain_db(6.0)
        .unwrap();
    chain_state(definition, state, bypassed, rate)
}

fn chain_state(
    definition: &'static dyn NativeDeviceDefinition,
    state: hex_lattice_contract::NativeDeviceState,
    bypassed: bool,
    rate: u32,
) -> NativeRuntimeChain {
    let definitions = [definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let prepared = registry
        .prepare_runtime_device(
            &state,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
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
        configuration: Some(Arc::new(state)),
        definition: prepared.processor,
    }])
    .unwrap()
}

fn render(chain: &mut NativeRuntimeChain, count: usize, block: usize) -> Vec<[f32; 2]> {
    let mut frames = vec![[0.25, -0.25]; count];
    for part in frames.chunks_mut(block) {
        chain.begin_block();
        chain.process_block(part).unwrap();
        chain.commit_block();
    }
    frames
}

fn check_power_transition(definition: &'static dyn NativeDeviceDefinition, rate: u32) {
    let ramp_frames = rate.div_ceil(200) as usize;
    for old_bypassed in [false, true] {
        let mut wet_reference = chain(definition, false, rate);
        if !old_bypassed {
            render(&mut wet_reference, 1024, 64);
        }
        let wet = render(&mut wet_reference, ramp_frames + 60, 1);
        for block in [1, 7, 64, 256] {
            let mut old = chain(definition, old_bypassed, rate);
            render(&mut old, 1024, 64);
            let mut next = chain(definition, !old_bypassed, rate);
            assert!(next.configuration_matches_range(&old, 0..1));
            assert!(next.can_inherit_from(&old));
            next.transfer_history_from(&mut old);
            let actual = render(&mut next, ramp_frames + 60, block);
            for (index, (frame, wet)) in actual.iter().zip(&wet).enumerate() {
                let progress = (index + 1).min(ramp_frames) as f32 / ramp_frames as f32;
                let mix = if old_bypassed {
                    progress
                } else {
                    1.0 - progress
                };
                for channel in 0..2 {
                    let dry = if channel == 0 { 0.25 } else { -0.25 };
                    let expected = dry + (wet[channel] - dry) * mix;
                    assert!(
                        (frame[channel] - expected).abs() < 1e-5,
                        "block {block} frame {index}: {frame:?} expected {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn auto_pan_power_transition_uses_its_history_until_off_and_restarts_fresh() {
    check_power_transition(&crate::auto_pan::AUTO_PAN_DEFINITION, 48_000);
}
#[test]
fn gate_power_transition_uses_its_detector_until_off_and_reopens_fresh() {
    check_power_transition(&crate::gate::GATE_DEFINITION, 48_000);
}

#[test]
fn gate_power_reversal_preserves_the_open_detector() {
    const QUIET: [f32; 2] = [0.001, -0.001];
    let definition = &crate::gate::GATE_DEFINITION;
    let mut on = chain(definition, false, 48_000);
    let mut off = chain(definition, true, 48_000);
    let mut back_on = chain(definition, false, 48_000);
    render(&mut on, 1024, 64);
    off.transfer_history_from(&mut on);
    let mut partial_fade = [QUIET; 64];
    off.begin_block();
    off.process_block(&mut partial_fade).unwrap();
    off.commit_block();
    back_on.transfer_history_from(&mut off);
    let mut reversed = [QUIET; 241];
    back_on.begin_block();
    back_on.process_block(&mut reversed).unwrap();
    back_on.commit_block();
    let open_gain = 10.0_f32.powf(6.0 / 20.0);
    assert!((reversed[240][0] - QUIET[0] * open_gain).abs() < 1.0e-8);
}

#[test]
fn compressor_power_transition_uses_its_detector_until_off_and_restarts_fresh() {
    check_power_transition(&crate::compressor::COMPRESSOR_DEFINITION, 48_000);
}

#[test]
fn compressor_power_reversals_and_settled_off_follow_the_external_key() {
    use hex_lattice::StereoAuxiliaryBlock;
    use hex_lattice_contract::NativeDeviceMetadata;

    const DRY: [f32; 2] = [0.0625, -0.03125];
    const KEY: [f32; 2] = [0.0, 0.75];
    const QUIET_KEY: [f32; 2] = [0.0; 2];
    const BLOCK: usize = 256;
    const WARMUP: usize = 16 * BLOCK;
    const FADE: usize = 240;

    let definition = &crate::compressor::COMPRESSOR_DEFINITION;
    let make_chain = |bypassed| {
        chain_state(
            definition,
            definition.default_state().unwrap(),
            bypassed,
            48_000,
        )
    };
    let render_keyed = |chain: &mut NativeRuntimeChain, key: [f32; 2], frames| {
        let key = [key; BLOCK];
        let mut output = vec![DRY; frames];
        for chunk in output.chunks_mut(BLOCK) {
            let length = chunk.len();
            let block =
                StereoAuxiliaryBlock::new(chunk, Some(&key[..length]), &mut [], BLOCK).unwrap();
            chain.begin_block();
            chain
                .process_auxiliary_stage(
                    0,
                    block,
                    0,
                    None,
                    None,
                    None,
                    None,
                    &mut |_, _, _, _, _, _| {},
                )
                .unwrap();
            chain.commit_block();
        }
        output
    };

    for off_frames in [64, 2 * BLOCK] {
        let mut on = make_chain(false);
        let warmed = render_keyed(&mut on, KEY, WARMUP);
        assert!(warmed[WARMUP - 1][0] < DRY[0] * 0.75);

        let mut off = make_chain(true);
        off.transfer_history_from(&mut on);
        let off_output = render_keyed(&mut off, KEY, off_frames);
        assert!(off_output[0][0] < DRY[0]);
        if off_frames > FADE {
            assert!(off_output[FADE..].iter().all(|frame| *frame == DRY));
        }

        let mut reference = make_chain(false);
        if off_frames < FADE {
            render_keyed(&mut reference, KEY, WARMUP + off_frames);
        }
        let expected = render_keyed(&mut reference, QUIET_KEY, BLOCK);
        let mut resumed = make_chain(false);
        resumed.transfer_history_from(&mut off);
        let actual = render_keyed(&mut resumed, QUIET_KEY, BLOCK);
        assert_eq!(
            actual[FADE..],
            expected[FADE..],
            "incorrect external-key detector after {off_frames} off frames",
        );
    }
}

#[test]
fn reverb_power_transition_restarts_a_fresh_room_at_supported_rates() {
    for rate in [8_000, 44_100, 48_000, 192_000] {
        check_power_transition(&crate::reverb::REVERB_DEFINITION, rate);
    }
}

#[test]
fn frozen_reverb_reversal_rollback_and_unchanged_handoff_keep_one_audible_mix() {
    use crate::reverb::{REVERB_DEFINITION as D, parameter as p};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    let mut state = D.default_state().unwrap();
    for (id, value) in [
        (p::DRY_WET, ParameterValue::Float(100.0)),
        (p::SPIN, ParameterValue::Bool(false)),
        (p::CHORUS, ParameterValue::Bool(false)),
    ] {
        state = D.patch_parameter(&state, id, value).unwrap();
    }
    let mut seed = chain_state(&D, state.clone(), false, 48_000);
    render(&mut seed, 4800, 64);
    state = D
        .patch_parameter(&state, p::FREEZE, ParameterValue::Bool(true))
        .unwrap();
    let mut frozen = chain_state(&D, state.clone(), false, 48_000);
    frozen.inherit_from(&seed);
    render(&mut frozen, 4800, 64);
    let mut reference = chain_state(&D, state.clone(), false, 48_000);
    reference.inherit_from(&frozen);
    let wet = render(&mut reference, 318, 1);
    assert!(wet.iter().flatten().any(|sample| sample.abs() > 1e-5));
    for block in [1, 17, 64, 256] {
        let mut old = chain_state(&D, state.clone(), false, 48_000);
        old.inherit_from(&frozen);
        let mut off = chain_state(&D, state.clone(), true, 48_000);
        assert!(off.configuration_matches_range(&old, 0..1));
        off.transfer_history_from(&mut old);
        let mut actual = render(&mut off, 13, block);
        let mut unchanged = chain_state(&D, state.clone(), true, 48_000);
        unchanged.transfer_history_from(&mut off);
        actual.extend(render(&mut unchanged, 5, block));
        let mut on = chain_state(&D, state.clone(), false, 48_000);
        on.transfer_history_from(&mut unchanged);
        on.begin_block();
        on.process_block(&mut [[0.25, -0.25]; 7]).unwrap();
        on.rollback_block();
        actual.extend(render(&mut on, 300, block));
        for (i, (actual, wet)) in actual.iter().zip(&wet).enumerate() {
            let mix = if i < 18 {
                1.0 - (i + 1) as f32 / 240.0
            } else {
                0.925 + 0.075 * ((i - 17).min(240) as f32 / 240.0)
            };
            for lane in 0..2 {
                let dry = if lane == 0 { 0.25 } else { -0.25 };
                let expected = dry + (wet[lane] - dry) * mix;
                assert!(
                    (actual[lane] - expected).abs() < 1e-6,
                    "block={block} frame={i} actual={actual:?} expected={expected}"
                );
            }
        }
    }
}

#[test]
#[ignore = "manual release hot-path profile; no physical deadline acceptance"]
fn reverb_bypass_hot_path_profile() {
    use std::{hint::black_box, time::Instant};
    println!("optimized={}", !cfg!(debug_assertions));
    let definition = &crate::reverb::REVERB_DEFINITION;
    let mut on = chain(definition, false, 48_000);
    let mut off = chain(definition, true, 48_000);
    let mut times = Vec::with_capacity(4096);
    let mut enabled = true;
    for iteration in 0_u32..4608 {
        let mut input = [[0.25, -0.25]; 128];
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
        "reverb 48k/128, 512 warmup +4096 measured blocks, bypass every16: mean={}ns p50={}ns p99={}ns max={}ns",
        times.iter().sum::<u128>() / times.len() as u128,
        times[times.len() / 2],
        times[times.len() * 99 / 100],
        times[times.len() - 1]
    );
}

#[test]
#[ignore = "manual release hot-path profile; no physical deadline acceptance"]
fn reverb_settled_power_profile() {
    use std::{hint::black_box, time::Instant};

    assert!(!black_box(cfg!(debug_assertions)), "run with --release");
    let definition = &crate::reverb::REVERB_DEFINITION;
    for bypassed in [false, true] {
        let mut runtime = chain(definition, bypassed, 48_000);
        let mut times = Vec::with_capacity(4096);
        for block in 0..4608 {
            let mut input = [[0.25, -0.25]; 128];
            let start = Instant::now();
            runtime.begin_block();
            runtime.process_block(black_box(&mut input)).unwrap();
            runtime.commit_block();
            let elapsed = start.elapsed().as_nanos();
            black_box(input);
            if block >= 512 {
                times.push(elapsed);
            }
        }
        times.sort_unstable();
        println!(
            "reverb 48k/128, 512 warmup +4096 measured blocks, off={bypassed}: p50={}ns p99={}ns",
            times[times.len() / 2],
            times[times.len() * 99 / 100],
        );
    }
}
