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
fn meter_reports_committed_post_input_gain_in_both_modes() {
    for settings in [
        vec![(GAIN, ParameterValue::Float(6.0))],
        vec![
            (MAXIMIZE, ParameterValue::Bool(true)),
            (THRESHOLD, ParameterValue::Float(-6.0)),
        ],
    ] {
        let mut runtime = factory(48_000, &settings).create_runtime().unwrap();
        let mut input = [[0.125, -0.25]; 16];
        runtime.begin_block();
        runtime.process_block(&mut input, &EMPTY_DEVICE_RESOURCES);
        runtime.rollback_block();
        assert_eq!(runtime.take_dynamics_meter().unwrap().post_input_peak, 0.0);

        let mut input = [[0.125, -0.25]; 16];
        runtime.begin_block();
        runtime.process_block(&mut input, &EMPTY_DEVICE_RESOURCES);
        runtime.commit_block();
        let peak = runtime.take_dynamics_meter().unwrap().post_input_peak;
        assert!(
            (peak - 0.25 * 10.0_f32.powf(6.0 / 20.0)).abs() < 1e-5,
            "{peak}"
        );
        assert_eq!(runtime.take_dynamics_meter().unwrap().post_input_peak, 0.0);
    }
}

#[test]
fn meter_tracks_the_actual_gain_ramp_and_silence() {
    let mut changed = factory(
        48_000,
        &[
            (GAIN, ParameterValue::Float(6.0)),
            (ROUTING, ParameterValue::Choice(1)),
        ],
    )
    .create_runtime()
    .unwrap();
    let old_ms = factory(48_000, &[(ROUTING, ParameterValue::Choice(1))])
        .create_runtime()
        .unwrap();
    assert!(changed.can_inherit_from(old_ms.as_ref()));
    changed.inherit_from(old_ms.as_ref());
    let mut frame = [[0.25, -0.5]];
    changed.begin_block();
    changed.process_block(&mut frame, &EMPTY_DEVICE_RESOURCES);
    changed.commit_block();
    let peak = changed.take_dynamics_meter().unwrap().post_input_peak;
    let target = 10.0_f32.powf(6.0 / 20.0);
    let expected = 0.5 * (1.0 + (target - 1.0) / 240.0);
    assert!((peak - expected).abs() < 1e-6, "{peak} != {expected}");

    let mut silence = [[0.0; 2]; 16];
    changed.begin_block();
    changed.process_block(&mut silence, &EMPTY_DEVICE_RESOURCES);
    changed.commit_block();
    assert_eq!(changed.take_dynamics_meter().unwrap().post_input_peak, 0.0);
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
fn standard_anticipates_a_transient_without_a_large_background_step() {
    let mut runtime = factory(48_000, &[(CEILING, ParameterValue::Float(0.0))])
        .create_runtime()
        .unwrap();
    let mut input = vec![[0.25; 2]; 2_400];
    input[2_000] = [4.0; 2];
    let output = render(runtime.as_mut(), &input, &[31, 64, 7]);
    let max_step = output[1_000..2_070]
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(max_step < 0.01, "background step {max_step}");
    assert!(
        output
            .iter()
            .flatten()
            .all(|sample| sample.abs() <= 1.000_001)
    );
}

#[test]
fn standard_attack_respects_link_and_sample_ceiling_across_rates() {
    for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        for link in [0.0, 1.0] {
            let f = factory(
                rate,
                &[
                    (CEILING, ParameterValue::Float(0.0)),
                    (LINK, ParameterValue::Float(link)),
                ],
            );
            let latency = f.algorithmic_latency_frames().unwrap() as usize;
            let transient = 2 * latency + 500;
            let mut input = vec![[0.25; 2]; transient + latency + 100];
            input[transient] = [4.0, 0.25];
            let mut runtime = f.create_runtime().unwrap();
            let output = render(runtime.as_mut(), &input, &[1, 63, 257, 17]);
            let preceding = output[transient - 1..transient + latency]
                .windows(2)
                .map(|pair| (pair[1][0] - pair[0][0]).abs())
                .fold(0.0_f32, f32::max);
            assert!(preceding < 0.02, "rate={rate} link={link} step={preceding}");
            assert!(
                output
                    .iter()
                    .flatten()
                    .all(|sample| sample.abs() <= 1.000_001),
                "rate={rate} link={link}"
            );
            if link == 0.0 {
                assert_eq!(output[transient + latency][1], 0.25);
            } else {
                assert!(output[transient + latency][1] < 0.07);
            }
        }
    }
}

#[test]
fn standard_attack_keeps_earlier_deadlines_when_stronger_peaks_arrive() {
    let f = factory(
        8_000,
        &[
            (CEILING, ParameterValue::Float(0.0)),
            (LINK, ParameterValue::Float(1.0)),
        ],
    );
    let latency = f.algorithmic_latency_frames().unwrap() as usize;
    let mut input = vec![[0.25; 2]; 200];
    input[100][0] = 2.0;
    input[110][0] = 8.0;
    let mut runtime = f.create_runtime().unwrap();
    let output = render(runtime.as_mut(), &input, &[3, 17, 1]);
    assert!(output[100 + latency][0] <= 1.0);
    assert!(output[110 + latency][0] <= 1.0);
    let max_step = output[99..=110 + latency]
        .windows(2)
        .map(|pair| (pair[1][1] - pair[0][1]).abs())
        .fold(0.0_f32, f32::max);
    assert!(max_step < 0.025, "linked background step {max_step}");
}

#[test]
fn standard_attack_rollback_and_compatible_handoff_preserve_envelope() {
    let f = factory(48_000, &[(CEILING, ParameterValue::Float(0.0))]);
    let mut original = f.create_runtime().unwrap();
    let mut reference = f.create_runtime().unwrap();
    let mut input = vec![[0.25; 2]; 1_010];
    input[1_000] = [4.0; 2];
    assert_eq!(
        render(original.as_mut(), &input, &[1010]),
        render(reference.as_mut(), &input, &[1010])
    );
    original.begin_block();
    let mut abandoned = [[0.25; 2]; 20];
    original.process_block(&mut abandoned, &EMPTY_DEVICE_RESOURCES);
    original.rollback_block();

    let mut successor = f.create_runtime().unwrap();
    assert!(successor.can_inherit_from(original.as_ref()));
    successor.inherit_from(original.as_ref());
    let continuation = [[0.25; 2]; 200];
    assert_eq!(
        render(successor.as_mut(), &continuation, &[7, 53]),
        render(reference.as_mut(), &continuation, &[200])
    );
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
#[ignore = "diagnostic independent reconstruction; a limiter correction is not yet implemented"]
fn study_true_peak_with_independent_lanczos_reconstruction() {
    let factory = factory(
        48_000,
        &[
            (MODE, ParameterValue::Choice(2)),
            (LINK, ParameterValue::Float(0.0)),
            (RELEASE, ParameterValue::Float(0.01)),
            (CEILING, ParameterValue::Float(-1.0)),
        ],
    );
    let mut runtime = factory.create_runtime().unwrap();
    let mut input: Vec<_> = (0..4000)
        .map(|i| {
            let envelope = if i % 137 < 17 { 10.0 } else { 1.0 };
            [
                envelope * (i as f32 * 2.7 + 0.2).sin(),
                envelope * (i as f32 * 2.1 + 0.7).sin(),
            ]
        })
        .collect();
    input.extend(vec![[0.0; 2]; 2048]);
    let output = render(runtime.as_mut(), &input, &[1, 63, 257]);
    let sinc = |x: f64| {
        if x.abs() < 1e-12 {
            1.0
        } else {
            (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
        }
    };
    let kernels: [[f64; 64]; 32] = std::array::from_fn(|phase| {
        let mut kernel = std::array::from_fn(|tap| {
            let d = 31.0 + phase as f64 / 32.0 - tap as f64;
            sinc(d) * sinc(d / 32.0)
        });
        let sum: f64 = kernel.iter().sum();
        for coefficient in &mut kernel {
            *coefficient /= sum;
        }
        kernel
    });
    let mut peak = 0.0_f64;
    let mut at = 0;
    for (index, window) in output.windows(64).enumerate() {
        for kernel in &kernels {
            for lane in 0..2 {
                let value: f64 = kernel
                    .iter()
                    .zip(window)
                    .map(|(c, s)| c * f64::from(s[lane]))
                    .sum();
                if value.abs() > peak {
                    peak = value.abs();
                    at = index + 31;
                }
            }
        }
    }
    let ceiling = 10.0_f64.powf(-1.0 / 20.0);
    println!("independent Lanczos peak {peak:.9}, ceiling {ceiling:.9}, frame {at}");
    assert!(
        peak > ceiling + 0.01,
        "expected the documented counterexample"
    );
    // A single gain for the entire rendered file preserves every cancellation,
    // but requires future knowledge and cannot serve as a streaming algorithm.
    let uniform_gain = ceiling / peak;
    println!(
        "offline uniform gain {uniform_gain:.6} ({:.2} dB)",
        20.0 * uniform_gain.log10()
    );

    // Offline feasibility experiment only: bound each reconstruction by the
    // sum of absolute contributions and apply the tightest window gain to
    // every contributing sample. This sacrifices phase cancellation and has
    // unbounded whole-file storage; it is not a real-time candidate.
    let mut gains = vec![[1.0_f64; 2]; output.len()];
    for (index, window) in output.windows(64).enumerate() {
        for lane in 0..2 {
            let mut maximum = 0.0_f64;
            for kernel in &kernels {
                let bound: f64 = kernel
                    .iter()
                    .zip(window)
                    .map(|(c, sample)| (c * f64::from(sample[lane])).abs())
                    .sum();
                maximum = maximum.max(bound);
            }
            if maximum > ceiling {
                let gain = ceiling / maximum;
                for frame in &mut gains[index..index + 64] {
                    frame[lane] = frame[lane].min(gain);
                }
            }
        }
    }
    let guarded: Vec<_> = output
        .iter()
        .zip(&gains)
        .map(|(sample, gain)| {
            [
                (f64::from(sample[0]) * gain[0]) as f32,
                (f64::from(sample[1]) * gain[1]) as f32,
            ]
        })
        .collect();
    let mut guarded_peak = 0.0_f64;
    for window in guarded.windows(64) {
        // Spot-check eight phases; the absolute bound above covers all 32.
        for kernel in kernels.iter().step_by(4) {
            for lane in 0..2 {
                let value: f64 = kernel
                    .iter()
                    .zip(window)
                    .map(|(c, sample)| c * f64::from(sample[lane]))
                    .sum();
                guarded_peak = guarded_peak.max(value.abs());
            }
        }
    }
    let min_gain = gains.iter().flatten().copied().fold(1.0_f64, f64::min);
    println!(
        "absolute-bound guarded eight-phase peak {guarded_peak:.9}, minimum gain {min_gain:.6} ({:.2} dB)",
        20.0 * min_gain.log10()
    );
    assert!(guarded_peak <= ceiling + 1e-6);
}

#[test]
#[ignore = "offline feasibility experiment, not a real-time limiter correction"]
fn study_feasible_output_gain_plan() {
    const TAPS: usize = 32;
    const PHASES: usize = 8;
    const COUNT: usize = 128;
    let f = factory(
        48_000,
        &[
            (MODE, ParameterValue::Choice(2)),
            (LINK, ParameterValue::Float(0.0)),
            (RELEASE, ParameterValue::Float(0.01)),
            (CEILING, ParameterValue::Float(-1.0)),
        ],
    );
    let mut runtime = f.create_runtime().unwrap();
    let input: Vec<_> = (0..512)
        .map(|i| {
            let envelope = if i % 137 < 17 { 10.0 } else { 1.0 };
            [
                envelope * (i as f32 * 2.7 + 0.2).sin(),
                envelope * (i as f32 * 2.1 + 0.7).sin(),
            ]
        })
        .collect();
    let output = render(runtime.as_mut(), &input, &[1, 63, 257]);
    let sinc = |x: f64| {
        if x.abs() < 1e-12 {
            1.0
        } else {
            (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
        }
    };
    let kernels: [[f64; TAPS]; PHASES] = std::array::from_fn(|phase| {
        let mut kernel = std::array::from_fn(|tap| {
            let d = 15.0 + phase as f64 / PHASES as f64 - tap as f64;
            sinc(d) * sinc(d / 16.0)
        });
        let sum: f64 = kernel.iter().sum();
        for coefficient in &mut kernel {
            *coefficient /= sum;
        }
        kernel
    });
    let ceiling = 10.0_f64.powf(-1.0 / 20.0);
    let limit = ceiling - 1e-6;
    let mut samples = vec![[0.0_f64; 2]; COUNT + 2 * (TAPS - 1)];
    for (destination, sample) in samples[TAPS - 1..TAPS - 1 + COUNT]
        .iter_mut()
        .zip(&output[64..64 + COUNT])
    {
        *destination = sample.map(f64::from);
    }
    let mut gains = vec![[0.0_f64; 2]; samples.len()];
    // All-zero output is feasible. Each coordinate may only increase while
    // every overlapping signed reconstruction remains within the limit.
    for _ in 0..2 {
        for index in TAPS - 1..TAPS - 1 + COUNT {
            for lane in 0..2 {
                let mut step = 1.0 - gains[index][lane];
                for start in index.saturating_sub(TAPS - 1)..=index.min(samples.len() - TAPS) {
                    for kernel in &kernels {
                        let value: f64 = (0..TAPS)
                            .map(|tap| {
                                kernel[tap] * samples[start + tap][lane] * gains[start + tap][lane]
                            })
                            .sum();
                        let slope = kernel[index - start] * samples[index][lane];
                        if slope > 0.0 {
                            step = step.min((limit - value) / slope);
                        }
                        if slope < 0.0 {
                            step = step.min((-limit - value) / slope);
                        }
                    }
                }
                gains[index][lane] += step.max(0.0);
            }
        }
    }
    let result: Vec<_> = samples
        .iter()
        .zip(&gains)
        .map(|(sample, gain)| [(sample[0] * gain[0]) as f32, (sample[1] * gain[1]) as f32])
        .collect();
    let mut max_model = 0.0_f64;
    for window in result.windows(TAPS) {
        for kernel in &kernels {
            for lane in 0..2 {
                let value: f64 = kernel
                    .iter()
                    .zip(window)
                    .map(|(c, s)| c * f64::from(s[lane]))
                    .sum();
                max_model = max_model.max(value.abs());
            }
        }
    }
    let selected = &gains[TAPS - 1..TAPS - 1 + COUNT];
    let min_gain = selected.iter().flatten().copied().fold(1.0_f64, f64::min);
    let mean_gain = selected.iter().flatten().sum::<f64>() / (COUNT * 2) as f64;
    let jump = selected
        .windows(2)
        .flat_map(|pair| (0..2).map(move |lane| (pair[1][lane] - pair[0][lane]).abs()))
        .fold(0.0_f64, f64::max);
    let wider: [[f64; 64]; 32] = std::array::from_fn(|phase| {
        let mut kernel = std::array::from_fn(|tap| {
            let d = 31.0 + phase as f64 / 32.0 - tap as f64;
            sinc(d) * sinc(d / 32.0)
        });
        let sum: f64 = kernel.iter().sum();
        for coefficient in &mut kernel {
            *coefficient /= sum;
        }
        kernel
    });
    let mut independent_peak = 0.0_f64;
    for window in result.windows(64) {
        for kernel in &wider {
            for lane in 0..2 {
                let value: f64 = kernel
                    .iter()
                    .zip(window)
                    .map(|(c, s)| c * f64::from(s[lane]))
                    .sum();
                independent_peak = independent_peak.max(value.abs());
            }
        }
    }
    println!(
        "32-tap/8-phase feasible peak {max_model:.9}; gain min {min_gain:.6}, mean {mean_gain:.6}, max adjacent gain jump {jump:.6}"
    );
    println!("independent 64-tap/32-phase peak {independent_peak:.9}, ceiling {ceiling:.9}");
    assert!(max_model <= ceiling + 1e-7);

    // A second offline plan uses eight-frame linear knots, with zero endpoints
    // and a 64-tap/16-phase signed constraint. It still has no emitted past.
    const STRIDE: usize = 8;
    const KNOTS: usize = COUNT / STRIDE - 1;
    let source = &samples[TAPS - 1..TAPS - 1 + COUNT];
    let mut padded = vec![[0.0_f64; 2]; COUNT + 126];
    padded[63..63 + COUNT].copy_from_slice(source);
    let windows = padded.len() - 63;
    let mut knot_gain = [[0.0_f64; KNOTS]; 2];
    let mut planned = vec![[0.0_f32; 2]; padded.len()];
    for lane in 0..2 {
        // Precompute each knot's linear contribution to every constraint.
        let mut effects = vec![vec![0.0_f64; windows * 16]; KNOTS];
        for (knot, effect) in effects.iter_mut().enumerate() {
            let center = (knot + 1) * STRIDE;
            for (start, window) in padded.windows(64).enumerate() {
                for (phase, kernel) in wider.iter().step_by(2).enumerate() {
                    effect[start * 16 + phase] = kernel
                        .iter()
                        .zip(window)
                        .enumerate()
                        .map(|(tap, (coefficient, frame))| {
                            let absolute = start + tap;
                            let weight = absolute.checked_sub(63).map_or(0.0, |index| {
                                (1.0 - index.abs_diff(center) as f64 / STRIDE as f64).max(0.0)
                            });
                            coefficient * frame[lane] * weight
                        })
                        .sum();
                }
            }
        }
        let mut constraints = vec![0.0_f64; windows * 16];
        for _ in 0..3 {
            for knot in 0..KNOTS {
                let mut step = 1.0 - knot_gain[lane][knot];
                for (&value, &slope) in constraints.iter().zip(&effects[knot]) {
                    if slope > 0.0 {
                        step = step.min((limit - value) / slope);
                    }
                    if slope < 0.0 {
                        step = step.min((-limit - value) / slope);
                    }
                }
                let step = step.max(0.0);
                knot_gain[lane][knot] += step;
                for (value, slope) in constraints.iter_mut().zip(&effects[knot]) {
                    *value += step * slope;
                }
            }
        }
        for (index, frame) in planned[63..63 + COUNT].iter_mut().enumerate() {
            let gain: f64 = knot_gain[lane]
                .iter()
                .enumerate()
                .map(|(knot, gain)| {
                    let center = (knot + 1) * STRIDE;
                    gain * (1.0 - index.abs_diff(center) as f64 / STRIDE as f64).max(0.0)
                })
                .sum();
            frame[lane] = (source[index][lane] * gain) as f32;
        }
    }
    let mut smooth_peak = 0.0_f64;
    let mut constrained_peak = 0.0_f64;
    for window in planned.windows(64) {
        for (phase, kernel) in wider.iter().enumerate() {
            for lane in 0..2 {
                let value: f64 = kernel
                    .iter()
                    .zip(window)
                    .map(|(c, frame)| c * f64::from(frame[lane]))
                    .sum();
                smooth_peak = smooth_peak.max(value.abs());
                if phase % 2 == 0 {
                    constrained_peak = constrained_peak.max(value.abs());
                }
            }
        }
    }
    let max_knot_jump = knot_gain
        .iter()
        .flat_map(|lane| lane.windows(2).map(|pair| (pair[1] - pair[0]).abs()))
        .fold(0.0_f64, f64::max);
    println!(
        "smooth knot plan: constrained peak {constrained_peak:.9}, independent peak {smooth_peak:.9}, max knot jump {max_knot_jump:.6}, mean knot gain {:.6}",
        knot_gain.iter().flatten().sum::<f64>() / (2 * KNOTS) as f64
    );
    assert!(constrained_peak <= ceiling + 1e-7);
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

fn attenuated_quiet_runtime(
    f: &dyn PreparedOwnedProcessorRuntime,
) -> Box<dyn OwnedProcessorRuntime> {
    let mut runtime = f.create_runtime().unwrap();
    render(runtime.as_mut(), &[[4.0, -2.0]; 200], &[17, 64]);
    let latency = f.algorithmic_latency_frames().unwrap() as usize;
    render(runtime.as_mut(), &vec![[0.0; 2]; latency], &[1, 19]);
    assert!(runtime.committed_is_quiescent());
    // Silence exhausted pending output, but did not erase attenuation memory.
    let mut continuation = f.create_runtime().unwrap();
    continuation.inherit_from(runtime.as_ref());
    let output = render(
        continuation.as_mut(),
        &vec![[0.1, -0.03]; latency + 20],
        &[17],
    );
    assert!(output[latency][0] > 0.0 && output[latency][0] < 0.05);
    runtime
}

#[test]
fn quiet_layout_handoffs_reset_preused_histories_in_both_directions() {
    for (old_edits, new_edits) in [
        (
            vec![(LOOKAHEAD, ParameterValue::Choice(0))],
            vec![(LOOKAHEAD, ParameterValue::Choice(2))],
        ),
        (
            vec![(LOOKAHEAD, ParameterValue::Choice(2))],
            vec![(LOOKAHEAD, ParameterValue::Choice(0))],
        ),
        (
            vec![(ROUTING, ParameterValue::Choice(0))],
            vec![(ROUTING, ParameterValue::Choice(1))],
        ),
        (
            vec![(ROUTING, ParameterValue::Choice(1))],
            vec![(ROUTING, ParameterValue::Choice(0))],
        ),
        (
            vec![(MODE, ParameterValue::Choice(0))],
            vec![(MODE, ParameterValue::Choice(2))],
        ),
        (
            vec![(MODE, ParameterValue::Choice(2))],
            vec![(MODE, ParameterValue::Choice(0))],
        ),
    ] {
        let mut old_settings = old_edits;
        old_settings.push((RELEASE, ParameterValue::Float(3000.0)));
        let old_factory = factory(8000, &old_settings);
        let old = attenuated_quiet_runtime(old_factory.as_ref());
        let mut new_settings = new_edits;
        new_settings.push((AUTO_RELEASE, ParameterValue::Bool(true)));
        let new_factory = factory(8000, &new_settings);
        let latency = new_factory.algorithmic_latency_frames().unwrap() as usize;
        let mut candidate = new_factory.create_runtime().unwrap();
        render(candidate.as_mut(), &[[8.0, -4.0]; 200], &[19]);
        assert!(!candidate.committed_is_quiescent());
        candidate.begin_block();
        candidate.process_block(&mut [[9.0; 2]; 77], &EMPTY_DEVICE_RESOURCES);
        candidate.rollback_block();
        assert!(candidate.can_inherit_from(old.as_ref()));
        assert!(candidate.can_retarget_parameters_from(old.as_ref()));
        candidate.inherit_from(old.as_ref());
        assert!(candidate.committed_is_quiescent());
        assert!(candidate.speculative_is_quiescent());
        assert_eq!(
            candidate.take_dynamics_meter().unwrap(),
            hex_lattice::DynamicsMeter::default()
        );
        assert_eq!(
            candidate.fixed_storage_pair_bytes(),
            Some(new_factory.declared_pair_bytes())
        );
        // Reopened speculation and cancellation must start from the candidate's
        // reset history, even though both buffers had previously carried audio.
        candidate.begin_block();
        candidate.process_block(&mut [[99.0; 2]; 31], &EMPTY_DEVICE_RESOURCES);
        candidate.rollback_block();
        let mut input = vec![[0.1, -0.03]; latency * 2 + 4];
        input.extend_from_slice(&[[3.0, 2.0]; 13]);
        input.resize(input.len() + latency + 100, [0.1, -0.03]);
        let mut fresh = new_factory.create_runtime().unwrap();
        assert_eq!(
            render(candidate.as_mut(), &input, &[1, 17, 64]),
            render(fresh.as_mut(), &input, &[input.len()])
        );
    }
}

#[test]
fn layout_handoffs_refuse_pending_final_stage_open_transactions_and_other_rates() {
    let ms_factory = factory(8000, &[(ROUTING, ParameterValue::Choice(1))]);
    let mut old = ms_factory.create_runtime().unwrap();
    let stage_delay = ms_factory.algorithmic_latency_frames().unwrap() as usize / 2;
    let mut input = vec![[0.0; 2]; stage_delay + 1];
    input[0] = [4.0, 0.0];
    assert!(
        render(old.as_mut(), &input, &[1, 7])
            .iter()
            .all(|v| *v == [0.0; 2])
    );
    // The equivalent encoded first stage has drained; only the final m/s
    // physical-output guard still owns the impulse.
    let lr_factory = factory(8000, &[]);
    let mut encoded = lr_factory.create_runtime().unwrap();
    input[0] = [2.0; 2];
    render(encoded.as_mut(), &input, &[input.len()]);
    assert!(encoded.committed_is_quiescent());
    assert!(!old.committed_is_quiescent());
    let mut candidate = lr_factory.create_runtime().unwrap();
    assert!(!candidate.can_inherit_from(old.as_ref()));
    assert!(!candidate.can_retarget_parameters_from(old.as_ref()));
    candidate.inherit_from(old.as_ref());
    assert!(candidate.committed_is_quiescent());
    let drained = render(old.as_mut(), &vec![[0.0; 2]; stage_delay], &[1]);
    assert!(drained.iter().any(|v| *v != [0.0; 2]));
    assert!(old.committed_is_quiescent());
    assert!(candidate.can_inherit_from(old.as_ref()));
    old.begin_block();
    assert!(!candidate.can_inherit_from(old.as_ref()));
    old.rollback_block();
    candidate.begin_block();
    assert!(!candidate.can_inherit_from(old.as_ref()));
    candidate.rollback_block();
    assert!(candidate.can_inherit_from(old.as_ref()));
    let other_rate = factory(16000, &[]).create_runtime().unwrap();
    assert!(!other_rate.can_inherit_from(old.as_ref()));
}

#[test]
fn compatible_quiet_handoffs_preserve_envelopes_and_gain_ramps() {
    let f = factory(8000, &[(RELEASE, ParameterValue::Float(3000.0))]);
    let old = attenuated_quiet_runtime(f.as_ref());
    let changed = factory(
        8000,
        &[
            (RELEASE, ParameterValue::Float(3000.0)),
            (GAIN, ParameterValue::Float(12.0)),
        ],
    );
    let mut ramping = changed.create_runtime().unwrap();
    ramping.inherit_from(old.as_ref());
    render(ramping.as_mut(), &[[0.0; 2]], &[1]);
    assert!(ramping.committed_is_quiescent());
    let mut candidate = changed.create_runtime().unwrap();
    render(candidate.as_mut(), &[[7.0; 2]; 100], &[17]);
    assert!(candidate.can_inherit_from(ramping.as_ref()));
    candidate.inherit_from(ramping.as_ref());
    let input = [[0.01, -0.003]; 150];
    let continuation = render(ramping.as_mut(), &input, &[input.len()]);
    assert_eq!(
        render(candidate.as_mut(), &input, &[1, 17, 64]),
        continuation
    );
    let mut fresh = changed.create_runtime().unwrap();
    assert_ne!(render(fresh.as_mut(), &input, &[17]), continuation);
}
