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

fn check(definition: &'static dyn NativeDeviceDefinition) {
    check_rate(definition, 48_000);
}

fn check_rate(definition: &'static dyn NativeDeviceDefinition, rate: u32) {
    let ramp_frames = rate.div_ceil(200) as usize;
    for old_bypassed in [false, true] {
        let mut wet_reference = chain(definition, false, rate);
        render(&mut wet_reference, 1024, 64);
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
fn auto_pan_bypass_transition_preserves_real_modulator_history() {
    check(&crate::auto_pan::AUTO_PAN_DEFINITION);
}
#[test]
fn gate_bypass_transition_preserves_real_detector_history() {
    check(&crate::gate::GATE_DEFINITION);
}
#[test]
fn compressor_bypass_transition_preserves_real_detector_history() {
    check(&crate::compressor::COMPRESSOR_DEFINITION);
}

#[test]
fn reverb_bypass_transition_preserves_real_room_history_at_supported_rates() {
    for rate in [8_000, 44_100, 48_000, 192_000] {
        check_rate(&crate::reverb::REVERB_DEFINITION, rate);
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
