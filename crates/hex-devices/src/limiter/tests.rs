use super::*;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, OwnedProcessorRuntime,
    PreparedOwnedProcessorRuntime, StereoFrame, TuningTable,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use parameter::*;
fn state(changes: &[(ParameterId, ParameterValue)]) -> NativeDeviceState {
    let mut state = DEFINITION.default_state().unwrap();
    for (id, value) in changes {
        state = DEFINITION.patch_parameter(&state, *id, *value).unwrap();
    }
    state
}
fn factory(
    rate: u32,
    changes: &[(ParameterId, ParameterValue)],
) -> Arc<dyn PreparedOwnedProcessorRuntime> {
    let Some(PreparedProcessorFactory::Owned(factory)) = DEFINITION
        .prepare_runtime_factory(
            &state(changes),
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
    else {
        panic!("owned limiter")
    };
    factory
}
fn render(
    runtime: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    partitions: &[usize],
) -> Vec<StereoFrame> {
    let mut output = input.to_vec();
    let mut offset = 0;
    for count in partitions.iter().cycle() {
        if offset == output.len() {
            break;
        }
        let end = (offset + count).min(output.len());
        runtime.begin_block();
        runtime.process_block(&mut output[offset..end], &EMPTY_DEVICE_RESOURCES);
        runtime.commit_block();
        offset = end;
    }
    output
}
#[test]
fn state_is_strict_and_inactive_values_survive_mode_switching() {
    for descriptor in state::DESCRIPTORS {
        descriptor.validate_descriptor().unwrap();
    }
    let default = state(&[]);
    assert_eq!(DEFINITION.migrate_state(&default).unwrap(), default);
    for d in state::DESCRIPTORS {
        assert_eq!(
            DEFINITION.read_parameter(&default, d.id).unwrap(),
            d.default
        );
    }
    let changed = state(&[
        (AUTO_RELEASE, ParameterValue::Bool(true)),
        (LINK_MID_SIDE, ParameterValue::Float(0.37)),
        (MAXIMIZE, ParameterValue::Bool(true)),
    ]);
    assert!(!DEFINITION.parameter_enabled(&changed, RELEASE).unwrap());
    assert!(!DEFINITION.parameter_enabled(&changed, CEILING).unwrap());
    assert!(DEFINITION.parameter_enabled(&changed, THRESHOLD).unwrap());
    assert!(
        !DEFINITION
            .parameter_enabled(&changed, LINK_MID_SIDE)
            .unwrap()
    );
    let ms = DEFINITION
        .patch_parameter(&changed, ROUTING, ParameterValue::Choice(1))
        .unwrap();
    assert_eq!(
        DEFINITION.read_parameter(&ms, LINK_MID_SIDE).unwrap(),
        ParameterValue::Float(0.37)
    );
    assert!(DEFINITION.parameter_enabled(&ms, LINK_MID_SIDE).unwrap());
    assert!(!DEFINITION.parameter_enabled(&ms, LINK).unwrap());
    for id in [
        GAIN,
        CEILING,
        RELEASE,
        LINK,
        THRESHOLD,
        OUTPUT,
        LINK_MID_SIDE,
    ] {
        assert!(
            DEFINITION
                .patch_parameter(&default, id, ParameterValue::Float(f32::NAN))
                .is_err()
        );
        assert!(
            DEFINITION
                .patch_parameter(&default, id, ParameterValue::Float(f32::INFINITY))
                .is_err()
        );
    }
    for offset in [12, 20, 24, 28, 32] {
        let mut bytes = default.payload().to_vec();
        bytes[offset..offset + 4].copy_from_slice(&0.5f32.to_le_bytes());
        let invalid = NativeDeviceState::new(TYPE_ID, STATE_VERSION, bytes).unwrap();
        assert!(DEFINITION.migrate_state(&invalid).is_err());
    }
    for length in [0, 47, 49] {
        let invalid = NativeDeviceState::new(TYPE_ID, STATE_VERSION, vec![0; length]).unwrap();
        assert!(DEFINITION.migrate_state(&invalid).is_err());
    }
    assert!(
        DEFINITION
            .migrate_state(&NativeDeviceState::new(TYPE_ID, 2, default.payload().to_vec()).unwrap())
            .is_err()
    );
    assert!(
        DEFINITION
            .read_parameter(&default, ParameterId::from_static(99))
            .is_err()
    );
    assert!(
        DEFINITION
            .patch_parameter(&default, MODE, ParameterValue::Choice(3))
            .is_err()
    );
    assert!(
        DEFINITION
            .patch_parameter(&default, GAIN, ParameterValue::Bool(true))
            .is_err()
    );
}
#[test]
fn declared_latency_storage_and_impulse_are_exact_for_all_configurations() {
    for rate in [8000, 44100, 48000, 96000, 192000] {
        for lookahead in 0..3 {
            for routing in 0..2 {
                for mode in 0..3 {
                    let f = factory(
                        rate,
                        &[
                            (LOOKAHEAD, ParameterValue::Choice(lookahead)),
                            (ROUTING, ParameterValue::Choice(routing)),
                            (MODE, ParameterValue::Choice(mode)),
                        ],
                    );
                    let expected = (([1.5, 3.0, 6.0][lookahead as usize] * f64::from(rate) / 1000.0)
                        .ceil() as usize
                        + if mode == 2 { 31 } else { 0 })
                        * if routing == 1 { 2 } else { 1 };
                    assert_eq!(f.algorithmic_latency_frames(), Some(expected as u32));
                    let mut runtime = f.create_runtime().unwrap();
                    assert_eq!(
                        runtime.fixed_storage_pair_bytes(),
                        Some(f.declared_pair_bytes())
                    );
                    assert!(f.declared_pair_bytes() < hex_lattice::MAX_OWNED_PROCESSOR_PAIR_BYTES);
                    let mut input = vec![[0.0; 2]; expected + 1];
                    input[0] = [0.1, -0.03];
                    let output = render(runtime.as_mut(), &input, &[1, 19, 257]);
                    if mode == 1 {
                        // Soft clipping has a gentle transfer even below the
                        // limiting threshold; measured model is not a dry path.
                        let ceiling = 10.0f64.powf(-0.3 / 20.0);
                        let ratios = input[0].map(|v| {
                            let x = f64::from(v).abs() / ceiling;
                            (x * x).tanh().sqrt() / x
                        });
                        let ratios = if routing == 1 {
                            [ratios[0].min(ratios[1]); 2]
                        } else {
                            ratios
                        };
                        for lane in 0..2 {
                            assert!(
                                (f64::from(output[expected][lane])
                                    - f64::from(input[0][lane]) * ratios[lane])
                                    .abs()
                                    < 1e-7
                            );
                        }
                    } else {
                        assert_eq!(output[expected], input[0]);
                    }
                    assert_eq!(output.iter().filter(|frame| **frame != [0.0; 2]).count(), 1);
                    assert!(runtime.committed_is_quiescent());
                }
            }
        }
    }
}
#[test]
fn sample_ceiling_and_partition_independence_with_extreme_stereo_transients() {
    for mode in 0..3 {
        for routing in 0..2 {
            let f = factory(
                48000,
                &[
                    (MODE, ParameterValue::Choice(mode)),
                    (ROUTING, ParameterValue::Choice(routing)),
                    (CEILING, ParameterValue::Float(-6.0)),
                    (LINK, ParameterValue::Float(0.0)),
                    (RELEASE, ParameterValue::Float(0.01)),
                ],
            );
            let input: Vec<_> = (0..16000)
                .map(|i| {
                    [
                        (i as f32 * 0.713).sin() * if i % 997 == 0 { 100.0 } else { 2.0 },
                        (i as f32 * 2.7).sin() * 3.0,
                    ]
                })
                .collect();
            let mut a = f.create_runtime().unwrap();
            let mut b = f.create_runtime().unwrap();
            let output = render(a.as_mut(), &input, &[input.len()]);
            assert_eq!(output, render(b.as_mut(), &input, &[1, 63, 257, 17]));
            assert!(
                output
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && v.abs() <= 10.0f32.powf(-6.0 / 20.0) + 1e-6),
                "mode={mode} routing={routing}"
            );
        }
    }
}
#[test]
fn rollback_and_reopen_preserve_pcm_and_publish_only_committed_reduction() {
    let f = factory(8000, &[]);
    let mut runtime = f.create_runtime().unwrap();
    let mut reference = f.create_runtime().unwrap();
    let warm = vec![[0.1, -0.1]; 91];
    render(runtime.as_mut(), &warm, &[17]);
    render(reference.as_mut(), &warm, &[91]);
    runtime.take_dynamics_meter();
    reference.take_dynamics_meter();
    runtime.begin_block();
    let mut loud = vec![[100.0, 50.0]; 177];
    runtime.process_block(&mut loud, &EMPTY_DEVICE_RESOURCES);
    runtime.rollback_block();
    assert_eq!(
        runtime.take_dynamics_meter().unwrap().reduction_db,
        [0.0; 2]
    );
    runtime.begin_block();
    runtime.process_block(&mut loud, &EMPTY_DEVICE_RESOURCES);
    runtime.begin_block();
    runtime.rollback_block();
    let quiet = vec![[0.2, -0.2]; 1000];
    assert_eq!(
        render(runtime.as_mut(), &quiet, &[19]),
        render(reference.as_mut(), &quiet, &[1000])
    );
    render(runtime.as_mut(), &vec![[4.0, -2.0]; 300], &[300]);
    let reading = runtime.take_dynamics_meter().unwrap();
    assert!(reading.reduction_db[0] > 10.0);
    assert_eq!(reading.reduction_db[0], reading.reduction_db[1]);
    assert_eq!(
        runtime.take_dynamics_meter().unwrap().reduction_db,
        [0.0; 2]
    );
    runtime.reset();
    assert!(runtime.committed_is_quiescent());
}
#[test]
fn maximize_threshold_and_output_are_independent_of_inactive_gain_and_ceiling() {
    let changes = [
        (MAXIMIZE, ParameterValue::Bool(true)),
        (THRESHOLD, ParameterValue::Float(-6.0)),
        (OUTPUT, ParameterValue::Float(-3.0)),
    ];
    let mut a = factory(8000, &changes).create_runtime().unwrap();
    let mut alternate = changes.to_vec();
    alternate.extend([
        (GAIN, ParameterValue::Float(24.0)),
        (CEILING, ParameterValue::Float(-24.0)),
    ]);
    let mut b = factory(8000, &alternate).create_runtime().unwrap();
    let signal = vec![[0.1, -0.1]; 1000];
    let output = render(a.as_mut(), &signal, &[1000]);
    assert_eq!(output, render(b.as_mut(), &signal, &[17]));
    assert!((output[999][0] - 0.1 * 10.0f32.powf(3.0 / 20.0)).abs() < 1e-7);
}
#[test]
fn link_and_release_controls_change_real_audio_and_soft_clip_reports_knee() {
    let signal = vec![[2.0, 0.1]; 1000];
    let mut linked = factory(8000, &[]).create_runtime().unwrap();
    let mut independent = factory(8000, &[(LINK, ParameterValue::Float(0.0))])
        .create_runtime()
        .unwrap();
    let a = render(linked.as_mut(), &signal, &[1000]);
    let b = render(independent.as_mut(), &signal, &[1000]);
    assert!((a[999][0] - b[999][0]).abs() < 1e-7);
    assert!(b[999][1] > a[999][1]);
    let mut soft = factory(8000, &[(MODE, ParameterValue::Choice(1))])
        .create_runtime()
        .unwrap();
    let c = render(soft.as_mut(), &signal, &[1000]);
    assert_ne!(a[999][0], c[999][0]);
    assert!(soft.take_dynamics_meter().unwrap().clipped);
    let auto_config = [
        (AUTO_RELEASE, ParameterValue::Bool(true)),
        (RELEASE, ParameterValue::Float(0.01)),
    ];
    let mut auto_fast = factory(8000, &auto_config).create_runtime().unwrap();
    let mut auto_slow = factory(
        8000,
        &[
            (AUTO_RELEASE, ParameterValue::Bool(true)),
            (RELEASE, ParameterValue::Float(3000.0)),
        ],
    )
    .create_runtime()
    .unwrap();
    render(auto_fast.as_mut(), &signal, &[1000]);
    render(auto_slow.as_mut(), &signal, &[1000]);
    let recovery = vec![[0.1; 2]; 3000];
    assert_eq!(
        render(auto_fast.as_mut(), &recovery, &[3000]),
        render(auto_slow.as_mut(), &recovery, &[17])
    );
    let mut slow = factory(8000, &[(RELEASE, ParameterValue::Float(3000.0))])
        .create_runtime()
        .unwrap();
    let mut fast = factory(8000, &[(RELEASE, ParameterValue::Float(1.0))])
        .create_runtime()
        .unwrap();
    render(slow.as_mut(), &signal, &[1000]);
    render(fast.as_mut(), &signal, &[1000]);
    let quiet = vec![[0.1; 2]; 3000];
    assert!(
        render(fast.as_mut(), &quiet, &[3000])[2999][0]
            > render(slow.as_mut(), &quiet, &[3000])[2999][0]
    );
}
#[test]
fn live_history_edits_keep_delay_but_reject_latency_and_routing_changes() {
    let f = factory(8000, &[]);
    let mut a = f.create_runtime().unwrap();
    render(a.as_mut(), &vec![[0.1; 2]; 99], &[99]);
    let mut b = factory(8000, &[(CEILING, ParameterValue::Float(-6.0))])
        .create_runtime()
        .unwrap();
    assert!(b.can_retarget_parameters_from(a.as_ref()));
    b.inherit_from(a.as_ref());
    assert!(render(b.as_mut(), &[[0.1; 2]; 1], &[1])[0][0] > 0.0);
    for edits in [
        [(LOOKAHEAD, ParameterValue::Choice(1))],
        [(ROUTING, ParameterValue::Choice(1))],
        [(MODE, ParameterValue::Choice(2))],
    ] {
        let c = factory(8000, &edits).create_runtime().unwrap();
        assert!(!c.can_inherit_from(a.as_ref()));
    }
}
#[test]
fn true_peak_bounds_independent_dense_reconstruction_under_fast_gain_modulation() {
    let f = factory(
        48000,
        &[
            (MODE, ParameterValue::Choice(2)),
            (LINK, ParameterValue::Float(0.0)),
            (RELEASE, ParameterValue::Float(0.01)),
            (CEILING, ParameterValue::Float(-1.0)),
        ],
    );
    let mut runtime = f.create_runtime().unwrap();
    let input: Vec<_> = (0..4000)
        .map(|i| {
            let envelope = if i % 137 < 17 { 10.0 } else { 1.0 };
            [
                envelope * (i as f32 * 2.7 + 0.2).sin(),
                envelope * (i as f32 * 2.1 + 0.7).sin(),
            ]
        })
        .collect();
    let output = render(runtime.as_mut(), &input, &[1, 63, 257]);
    let ceiling = 10.0f64.powf(-1.0 / 20.0);
    // Independent 32x, 32-tap Blackman interpolation of actual output,
    // using a separately implemented oracle with precomputed normalized kernels. This is a numerical oracle, not
    // physical DAC or reference-device parity evidence.
    let (peak, index) = dense_reconstructed_peak(&output);
    assert!(
        peak <= ceiling + 1e-5,
        "interpolated peak {peak} at frame {index}"
    );
}
fn dense_reconstructed_peak(output: &[StereoFrame]) -> (f64, usize) {
    let phases: [[f64; 32]; 32] = std::array::from_fn(|phase| {
        let position = 15.0 + phase as f64 / 32.0;
        let mut coefficients = std::array::from_fn(|tap| {
            let d = position - tap as f64;
            let sinc = if d.abs() < 1e-12 {
                1.0
            } else {
                (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d)
            };
            sinc * (0.42
                + 0.5 * (std::f64::consts::PI * d / 16.0).cos()
                + 0.08 * (std::f64::consts::PI * d / 8.0).cos())
        });
        let normalization: f64 = coefficients.iter().sum();
        for c in &mut coefficients {
            *c /= normalization;
        }
        coefficients
    });
    let mut maximum = (0.0, 0);
    for (index, window) in output.windows(32).enumerate() {
        for coefficients in phases {
            for lane in 0..2 {
                let value: f64 = coefficients
                    .iter()
                    .zip(window)
                    .map(|(c, s)| c * f64::from(s[lane]))
                    .sum();
                if value.abs() > maximum.0 {
                    maximum = (value.abs(), index + 15);
                }
            }
        }
    }
    maximum
}
#[test]
fn invalid_rates_and_nonfinite_pcm_are_contained() {
    for rate in [0, 7999, 192001, u32::MAX] {
        assert!(
            DEFINITION
                .prepare_runtime_factory(
                    &state(&[]),
                    &DevicePreparationContext {
                        sample_rate: rate,
                        tuning: &TuningTable::default(),
                        resources: &EMPTY_DEVICE_RESOURCES
                    }
                )
                .is_err()
        );
    }
    let mut runtime = factory(8000, &[]).create_runtime().unwrap();
    let input = [[f32::NAN, f32::INFINITY], [f32::MAX, -f32::MAX]];
    let output = render(runtime.as_mut(), &input, &[1]);
    assert!(output.iter().flatten().all(|v| v.is_finite()));
    let output = render(runtime.as_mut(), &vec![[0.0; 2]; 100], &[100]);
    assert!(output.iter().flatten().all(|v| v.is_finite()));
}

/// Explicit optimized microbenchmark, including begin/copy/commit and a
/// rollback/retry every 64 blocks. No physical callback deadline is inferred.
#[test]
#[cfg(not(debug_assertions))]
#[ignore = "run explicitly with --release --ignored --nocapture"]
fn optimized_transaction_throughput() {
    for (rate, mode, routing, block) in [(48000, 0, 0, 256), (192000, 2, 1, 64)] {
        let f = factory(
            rate,
            &[
                (MODE, ParameterValue::Choice(mode)),
                (ROUTING, ParameterValue::Choice(routing)),
                (LOOKAHEAD, ParameterValue::Choice(2)),
                (AUTO_RELEASE, ParameterValue::Bool(true)),
            ],
        );
        let mut runtime = f.create_runtime().unwrap();
        let input: Vec<_> = (0..block)
            .map(|i| [(i as f32 * 0.713).sin() * 2.0, (i as f32 * 2.17).sin()])
            .collect();
        let mut work = input.clone();
        for _ in 0..100 {
            work.copy_from_slice(&input);
            runtime.begin_block();
            runtime.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            runtime.commit_block();
        }
        let blocks = rate as usize * 2 / block;
        let started = std::time::Instant::now();
        for i in 0..blocks {
            work.copy_from_slice(&input);
            runtime.begin_block();
            runtime.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            if i % 64 == 0 {
                runtime.rollback_block();
                work.copy_from_slice(&input);
                runtime.begin_block();
                runtime.process_block(&mut work, &EMPTY_DEVICE_RESOURCES);
            }
            runtime.commit_block();
            std::hint::black_box(runtime.take_dynamics_meter());
            std::hint::black_box(&work);
        }
        let elapsed = started.elapsed().as_secs_f64();
        eprintln!(
            "limiter rate={rate} block={block} mode={mode} routing={routing} warmup_blocks=100 frames={} elapsed_s={elapsed:.6} realtime_ratio={:.3} pair_bytes={}",
            blocks * block,
            (blocks * block) as f64 / f64::from(rate) / elapsed,
            f.declared_pair_bytes()
        );
    }
}

#[test]
fn measured_soft_clip_and_true_peak_families_keep_the_reference_operating_shape() {
    let ceiling = 10.0f32.powf(-3.0 / 20.0);
    let mut soft = factory(
        44100,
        &[
            (MODE, ParameterValue::Choice(1)),
            (CEILING, ParameterValue::Float(-3.0)),
        ],
    )
    .create_runtime()
    .unwrap();
    let quiet = vec![[0.45 * ceiling; 2]; 1000];
    let output = render(soft.as_mut(), &quiet, &[256]);
    assert!((output[999][0] / ceiling - 0.44695).abs() < 0.0001);
    let loud = render(soft.as_mut(), &vec![[10.0; 2]; 1000], &[256]);
    assert!((loud[999][0] / ceiling - 0.93135).abs() < 0.0001);
    // Original steady997Hz and quarter-rate waveforms. Compare steady windows
    // against finite measured reference ranges, not bit identity or all-DAC proof.
    for (frequency, phase, target) in [
        (997.0, 0.0, 0.707767),
        (11025.0, std::f64::consts::FRAC_PI_4, 0.500459),
    ] {
        let mut tp = factory(
            44100,
            &[
                (MODE, ParameterValue::Choice(2)),
                (CEILING, ParameterValue::Float(-3.0)),
            ],
        )
        .create_runtime()
        .unwrap();
        let input: Vec<_> = (0..12000)
            .map(|i| {
                let value = (4.0
                    * (2.0 * std::f64::consts::PI * frequency * i as f64 / 44100.0 + phase).sin())
                    as f32;
                [value; 2]
            })
            .collect();
        let output = render(tp.as_mut(), &input, &[256]);
        let peak = output[6000..]
            .iter()
            .map(|v| v[0].abs())
            .fold(0.0f32, f32::max);
        assert!(
            (20.0 * (peak / target).log10()).abs() < 0.02,
            "freq={frequency} peak={peak}"
        );
    }
}

#[test]
fn release_regression_recovers_toward_a_changed_still_limiting_target() {
    let rate = 48000;
    let f = factory(
        rate,
        &[
            (CEILING, ParameterValue::Float(-6.0)),
            (RELEASE, ParameterValue::Float(100.0)),
        ],
    );
    let latency = f.algorithmic_latency_frames().unwrap() as usize;
    let warm = 4800;
    let elapsed = rate as usize / 20;
    let mut input = vec![[1.0; 2]; warm];
    input.resize(warm + latency + elapsed + 65, [0.8; 2]);
    let mut whole = f.create_runtime().unwrap();
    let mut segmented = f.create_runtime().unwrap();
    let output = render(whole.as_mut(), &input, &[input.len()]);
    assert_eq!(
        output,
        render(segmented.as_mut(), &input, &[1, 17, 64, 257])
    );
    let ceiling = 10.0_f64.powf(-6.0 / 20.0);
    let target = ceiling / 0.8;
    let expected =
        0.8 * (target + (ceiling - target) * (-(0.05 + 1.0 / f64::from(rate)) / 0.1).exp());
    let actual = f64::from(output[warm + latency + elapsed][0]);
    assert!(
        (actual - expected).abs() < 1e-5,
        "actual={actual} expected={expected}"
    );
    assert!(
        actual < ceiling - 0.04,
        "recovery is gradual while still limiting"
    );
}

#[test]
fn release_regression_mid_side_physical_guard_has_independent_recovery() {
    let rate = 48000;
    let f = factory(
        rate,
        &[
            (GAIN, ParameterValue::Float(12.0)),
            (CEILING, ParameterValue::Float(-6.0)),
            (RELEASE, ParameterValue::Float(100.0)),
            (ROUTING, ParameterValue::Choice(1)),
        ],
    );
    let latency = f.algorithmic_latency_frames().unwrap() as usize;
    let warm = 4800;
    let elapsed = rate as usize / 20;
    let mut input = vec![[1.0, 0.0]; warm];
    input.resize(warm + latency + elapsed + 65, [1.0; 2]);
    let mut whole = f.create_runtime().unwrap();
    let mut segmented = f.create_runtime().unwrap();
    let output = render(whole.as_mut(), &input, &[input.len()]);
    assert_eq!(
        output,
        render(segmented.as_mut(), &input, &[1, 17, 64, 257])
    );
    let ceiling = 10.0_f64.powf(-6.0 / 20.0);
    let expected = ceiling * (1.0 - 0.5 * (-0.05_f64 / 0.22).exp());
    let actual = output[warm + latency + elapsed];
    assert!(
        (f64::from(actual[0]) - expected).abs() < 0.003,
        "actual={actual:?} expected={expected}"
    );
    assert_eq!(actual[0], actual[1]);
}
