use super::*;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, OwnedProcessorRuntime,
    PreparedOwnedProcessorRuntime, PreparedProcessorFactory, TuningTable,
};
use std::sync::Arc;
fn patch(state: &NativeDeviceState, id: ParameterId, value: ParameterValue) -> NativeDeviceState {
    REVERB_DEFINITION.patch_parameter(state, id, value).unwrap()
}
fn state(decay: f32, wet: f32) -> NativeDeviceState {
    let s = REVERB_DEFINITION.default_state().unwrap();
    let s = patch(&s, parameter::DECAY, ParameterValue::Float(decay));
    patch(&s, parameter::DRY_WET, ParameterValue::Float(wet))
}
fn prepare(rate: u32, state: &NativeDeviceState) -> Arc<dyn PreparedOwnedProcessorRuntime> {
    let Some(PreparedProcessorFactory::Owned(factory)) = REVERB_DEFINITION
        .prepare_runtime_factory(
            state,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
    else {
        panic!("reverb must prepare an owned factory");
    };
    factory
}
fn run(runtime: &mut dyn OwnedProcessorRuntime, frames: &mut [[f32; 2]], partitions: &[usize]) {
    let mut start = 0;
    let mut i = 0;
    while start < frames.len() {
        let end = start
            .saturating_add(partitions[i % partitions.len()])
            .min(frames.len());
        runtime.begin_block();
        runtime.process_block(&mut frames[start..end], &EMPTY_DEVICE_RESOURCES);
        runtime.commit_block();
        start = end;
        i += 1;
    }
}
fn process_one(runtime: &mut dyn OwnedProcessorRuntime, frames: &mut [[f32; 2]]) {
    runtime.begin_block();
    runtime.process_block(frames, &EMPTY_DEVICE_RESOURCES);
    runtime.commit_block();
}
#[test]
fn state_v3_migrates_v1_and_v2_strictly_and_every_control_is_typed_bounded_and_roundtrips() {
    let d = &REVERB_DEFINITION;
    let original = d.default_state().unwrap();
    assert_eq!(d.descriptors().len(), 33);
    assert_eq!(d.migrate_state(&original).unwrap(), original);
    let payload = [2.1_f32, 73.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let legacy = NativeDeviceState::new(REVERB_TYPE_ID, 1, payload).unwrap();
    let migrated = d.migrate_state(&legacy).unwrap();
    assert_eq!(
        d.read_parameter(&migrated, parameter::DECAY).unwrap(),
        ParameterValue::Float(2.1)
    );
    assert_eq!(
        d.read_parameter(&migrated, parameter::DRY_WET).unwrap(),
        ParameterValue::Float(73.0)
    );
    assert_eq!(migrated.state_version(), 3);
    assert_eq!(
        preview(&legacy, 48000).unwrap(),
        preview(&migrated, 48000).unwrap()
    );
    let old =
        NativeDeviceState::new(REVERB_TYPE_ID, 2, original.payload()[..31 * 4].to_vec()).unwrap();
    let restored = d.migrate_state(&old).unwrap();
    assert_eq!(restored, original);
    assert!(
        preview(&original, 48000)
            .unwrap()
            .input
            .iter()
            .any(|point| point[1] > 2.0 / 3.0),
        "resonance is visible above unity"
    );
    for descriptor in d.descriptors() {
        let values = match descriptor.kind {
            ParameterKind::Float { min, max, .. } => {
                vec![ParameterValue::Float(min), ParameterValue::Float(max)]
            }
            ParameterKind::Bool => vec![ParameterValue::Bool(false), ParameterValue::Bool(true)],
            ParameterKind::Choice { labels } => vec![
                ParameterValue::Choice(0),
                ParameterValue::Choice(labels.len() as u32 - 1),
            ],
        };
        for value in values {
            let changed = patch(&original, descriptor.id, value);
            assert_eq!(d.read_parameter(&changed, descriptor.id).unwrap(), value);
            assert_eq!(d.migrate_state(&changed).unwrap(), changed);
        }
        if let ParameterKind::Float { min, max, .. } = descriptor.kind {
            for value in [f32::NAN, f32::INFINITY, min - 1.0, max + 1.0] {
                assert!(
                    d.patch_parameter(&original, descriptor.id, ParameterValue::Float(value))
                        .is_err()
                );
            }
        }
    }
    for version in [1, 2, 3] {
        for len in [0, 4, 7, 9, 120, 128] {
            let s = NativeDeviceState::new(REVERB_TYPE_ID, version, vec![0; len]).unwrap();
            assert!(d.migrate_state(&s).is_err());
        }
    }
    let mut bytes = original.payload().to_vec();
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
    assert!(
        d.migrate_state(&NativeDeviceState::new(REVERB_TYPE_ID, 2, bytes).unwrap())
            .is_err()
    );
}
#[test]
fn real_storage_fits_shared_admission_at_all_rates_and_invalid_rates_refuse() {
    let state = state(60.0, 100.0);
    for rate in [8000, 44100, 48000, 96000, 192000] {
        let p = prepare(rate, &state);
        assert!(p.declared_pair_bytes() <= hex_lattice::MAX_OWNED_PROCESSOR_PAIR_BYTES);
        assert!(p.declared_pair_bytes() > 0);
    }
    for rate in [0, 7999, 192001, u32::MAX] {
        assert!(
            REVERB_DEFINITION
                .prepare_processor(
                    &state,
                    &DevicePreparationContext {
                        sample_rate: rate,
                        tuning: &TuningTable::default(),
                        resources: &EMPTY_DEVICE_RESOURCES
                    }
                )
                .is_err()
        );
    }
}
#[test]
fn early_impulse_fractional_onset_and_predelay_have_hand_computed_oracles() {
    let mut s = state(1.0, 100.0);
    for id in [
        parameter::LO_CUT,
        parameter::HI_CUT,
        parameter::SPIN,
        parameter::CHORUS,
    ] {
        s = patch(&s, id, ParameterValue::Bool(false));
    }
    s = patch(&s, parameter::SHAPE, ParameterValue::Float(0.0));
    s = patch(&s, parameter::PREDELAY, ParameterValue::Float(0.0));
    let mut input = vec![[0.0; 2]; 100];
    input[0] = [1.0, 0.0];
    let mut delayed = input.clone();
    run(
        prepare(8000, &s).create_runtime().unwrap().as_mut(),
        &mut input,
        &[1, 7, 31],
    );
    // First original tap is 4.3ms * .75 at shape=0: 25.8 frames.
    // Linear interpolation splits its 1/8 weight between frames 25 and 26.
    assert!(input[..25].iter().all(|v| *v == [0.0; 2]));
    assert!((input[25][0] - 0.025).abs() < 2e-6);
    assert!((input[26][0] - 0.1).abs() < 2e-6);
    assert!(input.iter().all(|v| v[1] == 0.0));
    s = patch(&s, parameter::PREDELAY, ParameterValue::Float(5.0));
    run(
        prepare(8000, &s).create_runtime().unwrap().as_mut(),
        &mut delayed,
        &[17],
    );
    assert!(
        delayed[40..]
            .iter()
            .zip(&input[..60])
            .all(|(a, b)| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6))
    );
}
fn energy(frames: &[[f32; 2]]) -> f64 {
    frames.iter().flatten().map(|v| f64::from(*v).powi(2)).sum()
}
#[test]
fn mono_stereo_uneven_partitions_decay_and_short_tail_deadline() {
    for rate in [8000, 44100, 48000, 96000, 192000] {
        let s = state(2.5, 100.0);
        let p = prepare(rate, &s);
        let mut a = vec![[0.0; 2]; rate as usize];
        a[0] = [1.0, 0.3];
        let mut b = a.clone();
        let mut whole = p.create_runtime().unwrap();
        let mut chunks = p.create_runtime().unwrap();
        run(whole.as_mut(), &mut a, &[usize::MAX]);
        run(chunks.as_mut(), &mut b, &[1, 7, 131, 3, 257]);
        assert_eq!(a, b);
        assert!(a.iter().flatten().all(|v| v.is_finite()));
        assert!(a.iter().any(|v| v[0] != v[1]));
        assert!(energy(&a) > 0.0);
        let mut tail = vec![[0.0; 2]; rate as usize * 9];
        run(chunks.as_mut(), &mut tail, &[127, 1, 513]);
        assert!(chunks.committed_is_quiescent());
        assert!(tail[tail.len() - 512..].iter().flatten().all(|v| *v == 0.0));
    }
    let mut short = vec![[0.0; 2]; 48000];
    short[0] = [1.0; 2];
    let mut long = short.clone();
    run(
        prepare(48000, &state(0.1, 100.0))
            .create_runtime()
            .unwrap()
            .as_mut(),
        &mut short,
        &[127],
    );
    run(
        prepare(48000, &state(2.5, 100.0))
            .create_runtime()
            .unwrap()
            .as_mut(),
        &mut long,
        &[127],
    );
    assert!(energy(&long[24000..]) > energy(&short[24000..]) * 100.0);
}
#[test]
fn freeze_flat_cut_holds_energy_and_stop_reset_clears_it_without_unbounded_state() {
    let mut s = state(1.2, 100.0);
    s = patch(&s, parameter::CHORUS, ParameterValue::Bool(false));
    let mut old = prepare(48000, &s).create_runtime().unwrap();
    let mut excitation = vec![[0.0; 2]; 9600];
    excitation[0] = [1.0; 2];
    process_one(old.as_mut(), &mut excitation);
    s = patch(&s, parameter::FREEZE, ParameterValue::Bool(true));
    let p = prepare(48000, &s);
    assert_eq!(p.output_tail(), hex_lattice::OutputTail::Unbounded);
    let mut frozen = p.create_runtime().unwrap();
    frozen.inherit_from(old.as_ref());
    let mut settle = vec![[0.0; 2]; 4800];
    process_one(frozen.as_mut(), &mut settle);
    let mut silence = vec![[0.0; 2]; 48000];
    process_one(frozen.as_mut(), &mut silence);
    let first = energy(&silence);
    process_one(frozen.as_mut(), &mut silence);
    let second = energy(&silence);
    assert!(first > 1e-8);
    assert!(
        second > first * 0.5 && second < first * 2.0,
        "frozen energy {first} then {second}"
    );
    let mut twin = p.create_runtime().unwrap();
    twin.inherit_from(frozen.as_ref());
    let mut noise = vec![[16.0, -16.0]; 4096];
    let mut zero = vec![[0.0; 2]; 4096];
    process_one(frozen.as_mut(), &mut noise);
    process_one(twin.as_mut(), &mut zero);
    assert_eq!(noise, zero, "cut blocks new excitation at 100% wet");
    frozen.reset();
    process_one(frozen.as_mut(), &mut silence);
    assert!(frozen.committed_is_quiescent());
    assert_eq!(energy(&silence), 0.0);
}
#[test]
fn bypass_rate_change_and_mid_transition_copy_preserve_existing_host_contracts() {
    let p = prepare(48000, &state(1.2, 25.0));
    let mut a = p.create_runtime().unwrap();
    let mut b = p.create_runtime().unwrap();
    let mut input = vec![[0.2, -0.3]; 4097];
    let original = input.clone();
    let mut wet = input.clone();
    a.begin_block();
    a.process_bypassed_block_with_clock(&mut input, None, &EMPTY_DEVICE_RESOURCES);
    a.commit_block();
    process_one(b.as_mut(), &mut wet);
    assert_eq!(input, original);
    let mut x = vec![[0.0; 2]; 997];
    let mut y = x.clone();
    process_one(a.as_mut(), &mut x);
    process_one(b.as_mut(), &mut y);
    assert_eq!(x, y);
    let changed = state(2.5, 70.0);
    let p = prepare(48000, &changed);
    let mut next = p.create_runtime().unwrap();
    next.inherit_from(a.as_ref());
    process_one(next.as_mut(), &mut x[..71]);
    let mut twin = p.create_runtime().unwrap();
    twin.inherit_from(next.as_ref());
    x.fill([0.0; 2]);
    y.fill([0.0; 2]);
    run(next.as_mut(), &mut x, &[1, 7, 131]);
    run(twin.as_mut(), &mut y, &[997]);
    assert_eq!(x, y);
    assert!(
        !prepare(44100, &changed)
            .create_runtime()
            .unwrap()
            .can_inherit_from(next.as_ref())
    );
}
#[test]
fn every_room_control_changes_processed_audio_in_its_enabled_domain() {
    let mut base = state(1.2, 100.0);
    for id in [parameter::LO_CUT, parameter::LOW] {
        base = patch(&base, id, ParameterValue::Bool(true));
    }
    let render = |s: &NativeDeviceState| {
        let mut input = vec![[0.0; 2]; 12000];
        for i in (0..2400).step_by(97) {
            input[i] = [0.3, -0.2];
        }
        run(
            prepare(48000, s).create_runtime().unwrap().as_mut(),
            &mut input,
            &[127],
        );
        input
    };
    let reference = render(&base);
    for descriptor in REVERB_DEFINITION.descriptors() {
        if matches!(
            descriptor.id,
            parameter::DRY_WET | parameter::SMOOTH | parameter::FLAT | parameter::CUT
        ) {
            continue;
        }
        let v = match descriptor.kind {
            ParameterKind::Float { min, max, .. } => {
                ParameterValue::Float(if descriptor.default == ParameterValue::Float(max) {
                    min
                } else {
                    max
                })
            }
            ParameterKind::Bool => ParameterValue::Bool(
                REVERB_DEFINITION
                    .read_parameter(&base, descriptor.id)
                    .unwrap()
                    != ParameterValue::Bool(true),
            ),
            ParameterKind::Choice { labels } => ParameterValue::Choice(labels.len() as u32 - 1),
        };
        let changed = patch(&base, descriptor.id, v);
        assert!(
            render(&changed) != reference,
            "{} must affect audio",
            descriptor.label
        );
    }
}
#[test]
fn nonfinite_audio_and_extreme_finite_excitation_remain_bounded() {
    let mut r = prepare(48000, &state(60.0, 100.0))
        .create_runtime()
        .unwrap();
    let mut input = vec![[f32::MAX, f32::MIN]; 12000];
    input[0] = [f32::NAN, f32::INFINITY];
    process_one(r.as_mut(), &mut input);
    assert!(
        input
            .iter()
            .flatten()
            .all(|v| v.is_finite() && v.abs() <= 16.0)
    );
}
#[test]
fn smooth_modes_have_original_sample_accurate_mix_transition_oracles() {
    for (mode, frames) in [(0, 1), (1, 960), (2, 4800)] {
        let s = patch(
            &state(1.2, 0.0),
            parameter::PREDELAY,
            ParameterValue::Float(250.0),
        );
        let s = patch(&s, parameter::SMOOTH, ParameterValue::Choice(mode));
        let old = prepare(48000, &s).create_runtime().unwrap();
        let changed = patch(&s, parameter::DRY_WET, ParameterValue::Float(100.0));
        let mut next = prepare(48000, &changed).create_runtime().unwrap();
        next.inherit_from(old.as_ref());
        let mut input = vec![[1.0; 2]; frames];
        run(next.as_mut(), &mut input, &[1, 7, 131]);
        for (i, frame) in input.iter().enumerate() {
            assert!((frame[0] - (1.0 - (i + 1) as f32 / frames as f32)).abs() < 1e-4);
        }
        assert_eq!(input[frames - 1], [0.0; 2]);
    }
}
#[test]
fn freeze_flat_and_cut_each_have_a_functional_signal_law() {
    let s = patch(
        &state(1.2, 100.0),
        parameter::FREEZE,
        ParameterValue::Bool(true),
    );
    let mut cut = prepare(48000, &s).create_runtime().unwrap();
    let mut no_cut = prepare(
        48000,
        &patch(&s, parameter::CUT, ParameterValue::Bool(false)),
    )
    .create_runtime()
    .unwrap();
    let mut a = vec![[0.0; 2]; 12000];
    a[0] = [1.0; 2];
    let mut b = a.clone();
    process_one(cut.as_mut(), &mut a);
    process_one(no_cut.as_mut(), &mut b);
    assert_eq!(energy(&a), 0.0);
    assert!(energy(&b) > 0.0);
    let colored = patch(
        &patch(&s, parameter::FLAT, ParameterValue::Bool(false)),
        parameter::CUT,
        ParameterValue::Bool(false),
    );
    let mut colored = prepare(48000, &colored).create_runtime().unwrap();
    let mut input = vec![[0.0; 2]; 12000];
    input[0] = [1.0; 2];
    process_one(colored.as_mut(), &mut input);
    assert!(
        input != b,
        "flat and frequency-shaped frozen feedback differ"
    );
}
#[test]
fn provider_preview_uses_authored_filter_and_decay_controls() {
    let s = state(1.2, 55.0);
    let curves = preview(&s, 48000).unwrap();
    assert!(curves.input.iter().flatten().all(|v| v.is_finite()));
    let changed = patch(&s, parameter::HI_CUT, ParameterValue::Bool(false));
    assert!(
        preview(&changed, 48000)
            .unwrap()
            .input
            .iter()
            .all(|point| point[1] == 2.0 / 3.0)
    );
    let changed = patch(&s, parameter::HIGH_RATIO, ParameterValue::Float(2.0));
    assert!(curves.decay != super::preview(&changed, 48000).unwrap().decay);
}
#[test]
fn freeze_off_tail_declaration_includes_an_inherited_slow_transition() {
    let s = patch(
        &state(0.1, 100.0),
        parameter::SMOOTH,
        ParameterValue::Choice(2),
    );
    let mut ordinary = prepare(8000, &s).create_runtime().unwrap();
    let mut excitation = vec![[0.0; 2]; 2000];
    excitation[0] = [1.0; 2];
    process_one(ordinary.as_mut(), &mut excitation);
    let frozen_state = patch(&s, parameter::FREEZE, ParameterValue::Bool(true));
    let mut frozen = prepare(8000, &frozen_state).create_runtime().unwrap();
    frozen.inherit_from(ordinary.as_ref());
    let mut settle = vec![[0.0; 2]; 1000];
    process_one(frozen.as_mut(), &mut settle);
    let next = prepare(8000, &s);
    let hex_lattice::OutputTail::Finite { max_tail_frames } = next.output_tail() else {
        panic!("ordinary tail");
    };
    let mut next = next.create_runtime().unwrap();
    next.inherit_from(frozen.as_ref());
    let mut silence = vec![[0.0; 2]; max_tail_frames as usize];
    run(next.as_mut(), &mut silence, &[1, 7, 131]);
    assert!(next.committed_is_quiescent());
    assert_eq!(silence.last(), Some(&[0.0; 2]));
}

#[test]
fn reverb_power_off_discards_the_room_and_on_restarts_fresh() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner,
        StereoMatrix,
    };

    const RATE: u32 = 8_000;
    const SILENCE: [f32; 2] = [0.0; 2];

    let configured = Arc::new(state(15.0, 100.0));
    let prepare_chain = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            owner: ProcessorChainOwner::Main,
            device_id: 1,
            device_type: REVERB_TYPE_ID,
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: RATE,
            configuration: Some(Arc::clone(&configured)),
            definition: Some(PreparedProcessorFactory::Owned(prepare(RATE, &configured))),
            automation: Default::default(),
            modulation: None,
            meter: Default::default(),
        }])
        .unwrap()
    };
    let render = |chain: &mut NativeRuntimeChain, frames: &mut [[f32; 2]]| {
        for block in frames.chunks_mut(257) {
            chain.begin_block();
            chain.process_block(block).unwrap();
            chain.commit_block();
        }
    };

    let mut on = prepare_chain(false);
    let mut excited = [[0.25, -0.125]; 2048];
    render(&mut on, &mut excited);
    let mut tail = [SILENCE; 257];
    render(&mut on, &mut tail);
    assert!(tail.iter().any(|&frame| frame != SILENCE));

    let mut off = prepare_chain(true);
    off.transfer_history_from(&mut on);
    let mut fade = [SILENCE; 40];
    render(&mut off, &mut fade);
    assert!(fade[..39].iter().any(|&frame| frame != SILENCE));
    assert_eq!(fade[39], SILENCE);
    assert!(
        off.committed_range_is_quiescent(0..1),
        "settled Reverb kept its old room running"
    );
    let leaked_input = [0.125, 0.25];
    let mut dry = [leaked_input; 257];
    render(&mut off, &mut dry);
    assert_eq!(dry, [leaked_input; 257]);

    let mut restarted = prepare_chain(false);
    restarted.transfer_history_from(&mut off);
    let mut fresh = prepare_chain(false);
    let mut expected = [SILENCE; 1024];
    expected[128] = [0.5, -0.5];
    let mut actual = expected;
    render(&mut fresh, &mut expected);
    render(&mut restarted, &mut actual);
    assert_eq!(actual, expected);
    assert!(actual[128..].iter().any(|&frame| frame != SILENCE));
}

/// Matched release-build headless microbenchmark comparing 8 active stereo Reverbs
/// under O(written frames) OwnedProcessorRuntime dirty-range transactions vs a
/// legacy full-buffer twin-copy baseline on the exact same workload.
#[test]
#[cfg(not(debug_assertions))]
#[ignore = "run explicitly with --release --ignored --nocapture"]
fn multi_reverb_transaction_throughput() {
    const RATE: u32 = 48_000;
    const BLOCK: usize = 256;
    const INSTANCES: usize = 8;
    const WARMUP: usize = 100;
    const BLOCKS: usize = RATE as usize * 2 / BLOCK;

    let configured = state(2.5, 50.0);
    let factory = prepare(RATE, &configured);
    let mut owned: Vec<Box<dyn OwnedProcessorRuntime>> = (0..INSTANCES)
        .map(|_| factory.create_runtime().unwrap())
        .collect();
    let mut legacy_primary: Vec<Box<dyn OwnedProcessorRuntime>> = (0..INSTANCES)
        .map(|_| factory.create_runtime().unwrap())
        .collect();
    let mut legacy_scratch: Vec<Box<dyn OwnedProcessorRuntime>> = (0..INSTANCES)
        .map(|_| factory.create_runtime().unwrap())
        .collect();

    let input: [[f32; 2]; BLOCK] = std::array::from_fn(|i| {
        [
            (i as f32 * 0.17).sin() * 0.25,
            (i as f32 * 0.23).cos() * 0.25,
        ]
    });
    let mut work;
    for _ in 0..WARMUP {
        for r in &mut owned {
            work = input;
            r.begin_block();
            r.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            r.commit_block();
        }
        for (p, s) in legacy_primary.iter_mut().zip(&mut legacy_scratch) {
            work = input;
            s.inherit_from(p.as_ref());
            s.begin_block();
            s.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            s.commit_block();
            std::mem::swap(p, s);
        }
    }

    let started = std::time::Instant::now();
    for b in 0..BLOCKS {
        for r in &mut owned {
            work = input;
            r.begin_block();
            r.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            if b % 64 == 0 {
                r.rollback_block();
                work = input;
                r.begin_block();
                r.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            }
            r.commit_block();
            std::hint::black_box(&work);
        }
    }
    let owned_s = started.elapsed().as_secs_f64();

    let started = std::time::Instant::now();
    for b in 0..BLOCKS {
        for (p, s) in legacy_primary.iter_mut().zip(&mut legacy_scratch) {
            work = input;
            s.inherit_from(p.as_ref());
            s.begin_block();
            s.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            if b % 64 == 0 {
                s.rollback_block();
                work = input;
                s.inherit_from(p.as_ref());
                s.begin_block();
                s.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            }
            s.commit_block();
            std::mem::swap(p, s);
            std::hint::black_box(&work);
        }
    }
    let legacy_s = started.elapsed().as_secs_f64();

    let total_frames = BLOCKS * BLOCK;
    eprintln!(
        "reverb instances={INSTANCES} rate={RATE} block={BLOCK} frames={total_frames} owned_s={owned_s:.6} owned_rt={:.2}x legacy_twin_s={legacy_s:.6} legacy_twin_rt={:.2}x speedup={:.2}x pair_bytes={}",
        total_frames as f64 / f64::from(RATE) / owned_s,
        total_frames as f64 / f64::from(RATE) / legacy_s,
        legacy_s / owned_s,
        factory.declared_pair_bytes()
    );
}
