use super::*;
use hex_lattice::{DevicePreparationContext, EMPTY_DEVICE_RESOURCES, TuningTable};
fn prepare(rate: u32, decay: f32, wet: f32) -> Box<dyn PreparedNativeProcessor> {
    REVERB_DEFINITION
        .prepare_processor(
            &encode([decay, wet]).unwrap(),
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap()
}
fn run(runtime: &mut dyn NativeProcessorRuntime, frames: &mut [StereoFrame], partitions: &[usize]) {
    let mut start = 0;
    let mut i = 0;
    while start < frames.len() {
        let end = (start + partitions[i % partitions.len()]).min(frames.len());
        runtime.process_block(&mut frames[start..end], &EMPTY_DEVICE_RESOURCES);
        start = end;
        i += 1;
    }
}
#[test]
fn strict_versioned_state_ranges_and_runtime_admission() {
    let state = REVERB_DEFINITION.default_state().unwrap();
    assert_eq!(decode(&state).unwrap(), [1.2, 25.0]);
    assert_eq!(REVERB_DEFINITION.migrate_state(&state).unwrap(), state);
    for version in [0, 2] {
        if let Ok(state) = NativeDeviceState::new(REVERB_TYPE_ID, version, state.payload().to_vec())
        {
            assert!(decode(&state).is_err());
        }
    }
    for len in [0, 4, 7, 9, 12] {
        let malformed = NativeDeviceState::new(REVERB_TYPE_ID, 1, vec![0; len]).unwrap();
        assert!(decode(&malformed).is_err());
    }
    for values in [
        [f32::NAN, 50.0],
        [f32::INFINITY, 50.0],
        [0.0, 50.0],
        [2.6, 50.0],
        [1.0, -1.0],
        [1.0, 101.0],
    ] {
        assert!(encode(values).is_err());
        let payload = values.into_iter().flat_map(f32::to_le_bytes).collect();
        assert!(decode(&NativeDeviceState::new(REVERB_TYPE_ID, 1, payload).unwrap()).is_err());
    }
    assert!(
        REVERB_DEFINITION
            .patch_parameter(&state, parameter::DECAY, ParameterValue::Bool(true))
            .is_err()
    );
    for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        let p = prepare(rate, 2.5, 100.0);
        assert!(p.declared_runtime_bytes() <= hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES);
        let r = p.create_runtime();
        let r = r.as_any().downcast_ref::<ReverbRuntime>().unwrap();
        assert_eq!(
            p.declared_runtime_bytes(),
            std::mem::size_of::<ReverbRuntime>()
                + r.lines
                    .iter()
                    .flatten()
                    .map(|l| std::mem::size_of_val(l.samples.as_ref()))
                    .sum::<usize>()
        );
        assert!(
            r.feedback
                .iter()
                .flatten()
                .all(|g| g.value > 0.0 && g.value < 1.0)
        );
    }
    for rate in [0, 7_999, 192_001, u32::MAX] {
        assert!(
            REVERB_DEFINITION
                .prepare_processor(
                    &state,
                    &DevicePreparationContext {
                        sample_rate: rate,
                        tuning: &TuningTable::default(),
                        resources: &EMPTY_DEVICE_RESOURCES,
                    }
                )
                .is_err()
        );
    }
}
#[test]
fn impulse_onset_and_comb_recurrence_have_independent_timing_and_decay_oracles() {
    // 8 kHz avoids conversion. First comb: round(.0297*8000)=238;
    // two allpass direct coefficients multiply to .25, parallel gain is .25.
    let mut frames = vec![[0.0; 2]; 239];
    frames[0] = [1.0, 0.0];
    let mut runtime = prepare(8_000, 1.0, 100.0).create_runtime();
    run(runtime.as_mut(), &mut frames, &[1, 7, 31, 3, 127]);
    assert!(frames[..238].iter().all(|f| *f == [0.0; 2]));
    assert_eq!(frames[238], [0.0625, 0.0]);
    // Isolated primitive oracle checks the RT60 equation over an exact round trip.
    let mut line = Line {
        samples: vec![0.0; 238].into_boxed_slice(),
        cursor: 0,
    };
    let feedback = 10.0_f32.powf(-3.0 * 238.0 / 8_000.0);
    for t in 0..=476 {
        let actual = line.tick(if t == 0 { 1.0 } else { 0.0 }, feedback, false);
        let expected = if t == 238 {
            1.0
        } else if t == 476 {
            feedback
        } else {
            0.0
        };
        assert_eq!(actual, expected);
    }
    assert!((feedback.powf(8_000.0 / 238.0) - 0.001).abs() < 1e-8);
}
#[test]
fn stereo_mono_partitions_decay_and_finite_tail_at_representative_rates() {
    for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        let mut mono = vec![[0.0; 2]; rate as usize * 3];
        mono[0] = [1.0; 2];
        let mut chunks = mono.clone();
        let p = prepare(rate, 2.5, 100.0);
        let mut a = p.create_runtime();
        let mut b = p.create_runtime();
        run(a.as_mut(), &mut mono, &[usize::MAX]);
        run(b.as_mut(), &mut chunks, &[1, 7, 131, 3, 257]);
        assert_eq!(mono, chunks);
        assert!(mono.iter().all(|f| f.iter().all(|v| v.is_finite())));
        assert!(mono.iter().any(|f| f[0] != f[1]));
        let energy = |start: usize, end: usize| {
            mono[start..end]
                .iter()
                .flatten()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
        };
        assert!(energy(rate as usize, rate as usize * 2) < energy(0, rate as usize) * 0.01);
        let mut tail = vec![[0.0; 2]; rate as usize * 8];
        run(b.as_mut(), &mut tail, &[127, 1, 513]);
        assert!(b.is_quiescent());
        assert_eq!(&tail[tail.len() - 512..], &[[0.0; 2]; 512]);
        b.reset();
        assert!(b.is_quiescent());
    }
    let mut short = vec![[0.0; 2]; 48_000];
    short[0] = [1.0; 2];
    let mut long = short.clone();
    run(
        prepare(48_000, 0.1, 100.0).create_runtime().as_mut(),
        &mut short,
        &[127],
    );
    run(
        prepare(48_000, 2.5, 100.0).create_runtime().as_mut(),
        &mut long,
        &[127],
    );
    let energy = |v: &[StereoFrame]| v[24_000..].iter().flatten().map(|v| v * v).sum::<f32>();
    assert!(energy(&long) > energy(&short) * 1000.0);
}
#[test]
fn bypass_reset_history_rate_change_and_mid_ramp_copy() {
    let p = prepare(48_000, 1.2, 25.0);
    let mut a = p.create_runtime();
    let mut b = p.create_runtime();
    let mut input = vec![[0.2, -0.3]; 4097];
    let original = input.clone();
    let mut wet = input.clone();
    a.process_bypassed_block(&mut input, &EMPTY_DEVICE_RESOURCES);
    b.process_block(&mut wet, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(input, original);
    let mut tail_a = vec![[0.0; 2]; 997];
    let mut tail_b = tail_a.clone();
    a.process_block(&mut tail_a, &EMPTY_DEVICE_RESOURCES);
    b.process_block(&mut tail_b, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(tail_a, tail_b);
    let mut changed = prepare(48_000, 2.5, 100.0).create_runtime();
    assert!(changed.can_retarget_parameters_from(a.as_ref()));
    changed.copy_history_from(a.as_ref());
    let mut segment = [[0.0; 2]; 71];
    changed.process_block(&mut segment, &EMPTY_DEVICE_RESOURCES);
    let mut twin = prepare(48_000, 2.5, 100.0).create_runtime();
    twin.copy_history_from(changed.as_ref());
    let mut x = [[0.0; 2]; 1024];
    let mut y = x;
    changed.process_block(&mut x, &EMPTY_DEVICE_RESOURCES);
    run(twin.as_mut(), &mut y, &[1, 7, 83]);
    assert_eq!(x, y);
    let different_rate = prepare(44_100, 2.5, 100.0).create_runtime();
    assert!(!different_rate.can_copy_history_from(changed.as_ref()));
    changed.reset();
    let mut silent = [[0.0; 2]; 2000];
    changed.process_block(&mut silent, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(silent, [[0.0; 2]; 2000]);
    assert!(changed.is_quiescent());
}
#[test]
fn dry_endpoint_and_nonfinite_audio_do_not_poison_history() {
    let mut r = prepare(48_000, 2.5, 0.0).create_runtime();
    let mut input = [[0.25, -0.5]; 201];
    let original = input;
    r.process_block(&mut input, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(input, original);
    let mut input = [[f32::NAN, f32::INFINITY], [f32::MAX, f32::MIN]];
    r.process_block(&mut input, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(input, [[0.0; 2], [16.0, -16.0]]);
    let mut tail = vec![[0.0; 2]; 48_000];
    r.process_block(&mut tail, &EMPTY_DEVICE_RESOURCES);
    assert!(
        tail.iter()
            .flatten()
            .all(|v| v.is_finite() && v.abs() <= 16.0)
    );
}
#[test]
fn parameter_transition_starts_at_next_sample_and_reaches_exact_target() {
    let source = prepare(48_000, 0.1, 0.0).create_runtime();
    let mut target = prepare(48_000, 2.5, 100.0).create_runtime();
    target.copy_history_from(source.as_ref());
    let mut output = [[1.0; 2]; 960];
    target.process_block(&mut output, &EMPTY_DEVICE_RESOURCES);
    // Earliest comb response is after 1426 host frames, so only dry ramp is audible.
    for (i, frame) in output.iter().enumerate() {
        let expected = 1.0 - (i + 1) as f32 / 960.0;
        assert!((frame[0] - expected).abs() < 0.00002);
    }
    assert_eq!(output[959], [0.0; 2]);
}

#[test]
fn recursive_floor_prevents_denormal_feedback_history() {
    let mut line = Line {
        samples: vec![0.0; 5].into_boxed_slice(),
        cursor: 0,
    };
    assert_eq!(line.tick(1e-25, 0.9, false), 0.0);
    assert!(line.samples.iter().all(|v| *v == 0.0));
    for _ in 0..20 {
        assert_eq!(line.tick(0.0, 0.9, false), 0.0);
    }
}
