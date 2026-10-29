use super::*;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeProcessorRuntime,
    PreparedNativeProcessor, TuningTable,
};
fn patch(state: &NativeDeviceState, id: ParameterId, value: ParameterValue) -> NativeDeviceState {
    REVERB_DEFINITION.patch_parameter(state, id, value).unwrap()
}
fn state(decay: f32, wet: f32) -> NativeDeviceState {
    let s = REVERB_DEFINITION.default_state().unwrap();
    let s = patch(&s, parameter::DECAY, ParameterValue::Float(decay));
    patch(&s, parameter::DRY_WET, ParameterValue::Float(wet))
}
fn prepare(rate: u32, state: &NativeDeviceState) -> Box<dyn PreparedNativeProcessor> {
    REVERB_DEFINITION
        .prepare_processor(
            state,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap()
}
fn run(runtime: &mut dyn NativeProcessorRuntime, frames: &mut [[f32; 2]], partitions: &[usize]) {
    let mut start = 0;
    let mut i = 0;
    while start < frames.len() {
        let end = start
            .saturating_add(partitions[i % partitions.len()])
            .min(frames.len());
        runtime.process_block(&mut frames[start..end], &EMPTY_DEVICE_RESOURCES);
        start = end;
        i += 1;
    }
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
        assert!(p.declared_runtime_bytes() <= hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES);
        assert!(p.declared_runtime_bytes() > 0);
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
        prepare(8000, &s).create_runtime().as_mut(),
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
        prepare(8000, &s).create_runtime().as_mut(),
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
        let mut whole = p.create_runtime();
        let mut chunks = p.create_runtime();
        run(whole.as_mut(), &mut a, &[usize::MAX]);
        run(chunks.as_mut(), &mut b, &[1, 7, 131, 3, 257]);
        assert_eq!(a, b);
        assert!(a.iter().flatten().all(|v| v.is_finite()));
        assert!(a.iter().any(|v| v[0] != v[1]));
        assert!(energy(&a) > 0.0);
        let mut tail = vec![[0.0; 2]; rate as usize * 9];
        run(chunks.as_mut(), &mut tail, &[127, 1, 513]);
        assert!(chunks.is_quiescent());
        assert!(tail[tail.len() - 512..].iter().flatten().all(|v| *v == 0.0));
    }
    let mut short = vec![[0.0; 2]; 48000];
    short[0] = [1.0; 2];
    let mut long = short.clone();
    run(
        prepare(48000, &state(0.1, 100.0)).create_runtime().as_mut(),
        &mut short,
        &[127],
    );
    run(
        prepare(48000, &state(2.5, 100.0)).create_runtime().as_mut(),
        &mut long,
        &[127],
    );
    assert!(energy(&long[24000..]) > energy(&short[24000..]) * 100.0);
}
#[test]
fn freeze_flat_cut_holds_energy_and_stop_reset_clears_it_without_unbounded_state() {
    let mut s = state(1.2, 100.0);
    s = patch(&s, parameter::CHORUS, ParameterValue::Bool(false));
    let mut old = prepare(48000, &s).create_runtime();
    let mut excitation = vec![[0.0; 2]; 9600];
    excitation[0] = [1.0; 2];
    old.process_block(&mut excitation, &EMPTY_DEVICE_RESOURCES);
    s = patch(&s, parameter::FREEZE, ParameterValue::Bool(true));
    let p = prepare(48000, &s);
    assert_eq!(p.output_tail(), hex_lattice::OutputTail::Unbounded);
    let mut frozen = p.create_runtime();
    frozen.copy_history_from(old.as_ref());
    let mut settle = vec![[0.0; 2]; 4800];
    frozen.process_block(&mut settle, &EMPTY_DEVICE_RESOURCES);
    let mut silence = vec![[0.0; 2]; 48000];
    frozen.process_block(&mut silence, &EMPTY_DEVICE_RESOURCES);
    let first = energy(&silence);
    frozen.process_block(&mut silence, &EMPTY_DEVICE_RESOURCES);
    let second = energy(&silence);
    assert!(first > 1e-8);
    assert!(
        second > first * 0.5 && second < first * 2.0,
        "frozen energy {first} then {second}"
    );
    let mut twin = p.create_runtime();
    twin.copy_history_from(frozen.as_ref());
    let mut noise = vec![[16.0, -16.0]; 4096];
    let mut zero = vec![[0.0; 2]; 4096];
    frozen.process_block(&mut noise, &EMPTY_DEVICE_RESOURCES);
    twin.process_block(&mut zero, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(noise, zero, "cut blocks new excitation at 100% wet");
    frozen.reset();
    frozen.process_block(&mut silence, &EMPTY_DEVICE_RESOURCES);
    assert!(frozen.is_quiescent());
    assert_eq!(energy(&silence), 0.0);
}
#[test]
fn bypass_rate_change_and_mid_transition_copy_preserve_existing_host_contracts() {
    let p = prepare(48000, &state(1.2, 25.0));
    let mut a = p.create_runtime();
    let mut b = p.create_runtime();
    let mut input = vec![[0.2, -0.3]; 4097];
    let original = input.clone();
    let mut wet = input.clone();
    a.process_bypassed_block(&mut input, &EMPTY_DEVICE_RESOURCES);
    b.process_block(&mut wet, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(input, original);
    let mut x = vec![[0.0; 2]; 997];
    let mut y = x.clone();
    a.process_block(&mut x, &EMPTY_DEVICE_RESOURCES);
    b.process_block(&mut y, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(x, y);
    let changed = state(2.5, 70.0);
    let p = prepare(48000, &changed);
    let mut next = p.create_runtime();
    next.copy_history_from(a.as_ref());
    next.process_block(&mut x[..71], &EMPTY_DEVICE_RESOURCES);
    let mut twin = p.create_runtime();
    twin.copy_history_from(next.as_ref());
    x.fill([0.0; 2]);
    y.fill([0.0; 2]);
    run(next.as_mut(), &mut x, &[1, 7, 131]);
    run(twin.as_mut(), &mut y, &[997]);
    assert_eq!(x, y);
    assert!(
        !prepare(44100, &changed)
            .create_runtime()
            .can_copy_history_from(next.as_ref())
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
            prepare(48000, s).create_runtime().as_mut(),
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
    let mut r = prepare(48000, &state(60.0, 100.0)).create_runtime();
    let mut input = vec![[f32::MAX, f32::MIN]; 12000];
    input[0] = [f32::NAN, f32::INFINITY];
    r.process_block(&mut input, &EMPTY_DEVICE_RESOURCES);
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
        let old = prepare(48000, &s).create_runtime();
        let changed = patch(&s, parameter::DRY_WET, ParameterValue::Float(100.0));
        let mut next = prepare(48000, &changed).create_runtime();
        next.copy_history_from(old.as_ref());
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
    let mut cut = prepare(48000, &s).create_runtime();
    let mut no_cut = prepare(
        48000,
        &patch(&s, parameter::CUT, ParameterValue::Bool(false)),
    )
    .create_runtime();
    let mut a = vec![[0.0; 2]; 12000];
    a[0] = [1.0; 2];
    let mut b = a.clone();
    cut.process_block(&mut a, &EMPTY_DEVICE_RESOURCES);
    no_cut.process_block(&mut b, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(energy(&a), 0.0);
    assert!(energy(&b) > 0.0);
    let colored = patch(
        &patch(&s, parameter::FLAT, ParameterValue::Bool(false)),
        parameter::CUT,
        ParameterValue::Bool(false),
    );
    let mut colored = prepare(48000, &colored).create_runtime();
    let mut input = vec![[0.0; 2]; 12000];
    input[0] = [1.0; 2];
    colored.process_block(&mut input, &EMPTY_DEVICE_RESOURCES);
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
    let mut ordinary = prepare(8000, &s).create_runtime();
    let mut excitation = vec![[0.0; 2]; 2000];
    excitation[0] = [1.0; 2];
    ordinary.process_block(&mut excitation, &EMPTY_DEVICE_RESOURCES);
    let frozen_state = patch(&s, parameter::FREEZE, ParameterValue::Bool(true));
    let mut frozen = prepare(8000, &frozen_state).create_runtime();
    frozen.copy_history_from(ordinary.as_ref());
    let mut settle = vec![[0.0; 2]; 1000];
    frozen.process_block(&mut settle, &EMPTY_DEVICE_RESOURCES);
    let next = prepare(8000, &s);
    let hex_lattice::OutputTail::Finite { max_tail_frames } = next.output_tail() else {
        panic!("ordinary tail");
    };
    let mut next = next.create_runtime();
    next.copy_history_from(frozen.as_ref());
    let mut silence = vec![[0.0; 2]; max_tail_frames as usize];
    run(next.as_mut(), &mut silence, &[1, 7, 131]);
    assert!(next.is_quiescent());
    assert_eq!(silence.last(), Some(&[0.0; 2]));
}
