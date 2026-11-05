use super::*;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, OwnedProcessorRuntime,
    PreparedOwnedProcessorRuntime, StereoFrame, TuningTable,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use parameter::*;
mod bypass;
mod true_peak_oracle;
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
fn block_bypass_matches_single_frame_history_and_meter() {
    for size in [1, 64, 256, 513] {
        let factory = factory(48_000, &[]);
        let mut block = factory.create_runtime().unwrap();
        let mut reference = factory.create_runtime().unwrap();
        let input: Vec<_> = (0..size)
            .map(|i| [i as f32 / 256.0, -(i as f32) / 512.0])
            .collect();
        let mut dry = input.clone();
        block.begin_block();
        reference.begin_block();
        block.process_bypassed_block_with_clock(&mut dry, None, &EMPTY_DEVICE_RESOURCES);
        for frame in &input {
            let mut discarded = [*frame];
            reference.process_block(&mut discarded, &EMPTY_DEVICE_RESOURCES);
        }
        block.commit_block();
        reference.commit_block();
        assert_eq!(dry, input);
        assert_eq!(block.take_dynamics_meter(), reference.take_dynamics_meter());
        let tail = [[0.0; 2]; 512];
        assert_eq!(
            render(block.as_mut(), &tail, &[64]),
            render(reference.as_mut(), &tail, &[64])
        );
    }
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
fn falling_ceiling_limits_first_sample_after_compatible_handoff() {
    let rate = 8_000;
    let ceiling = 10.0_f32.powf(-24.0 / 20.0);
    for mode in 0..3 {
        for routing in 0..2 {
            let settings = |db| {
                [
                    (MODE, ParameterValue::Choice(mode)),
                    (ROUTING, ParameterValue::Choice(routing)),
                    (CEILING, ParameterValue::Float(db)),
                    (LINK, ParameterValue::Float(0.0)),
                ]
            };
            let old_factory = factory(rate, &settings(0.0));
            let new_factory = factory(rate, &settings(-24.0));
            let mut old = old_factory.create_runtime().unwrap();
            let warm = vec![[0.8, -0.4]; 4_000];
            let previous = render(old.as_mut(), &warm, &[127, 31]);
            assert!(
                previous.last().unwrap()[0] > 0.2,
                "mode={mode} routing={routing}"
            );
            let mut whole = new_factory.create_runtime().unwrap();
            let mut split = new_factory.create_runtime().unwrap();
            assert!(whole.can_inherit_from(old.as_ref()));
            assert!(split.can_inherit_from(old.as_ref()));
            whole.inherit_from(old.as_ref());
            split.inherit_from(old.as_ref());

            // An abandoned render must not consume the retained predecessor.
            split.begin_block();
            let mut abandoned = [[4.0, -2.0]; 19];
            split.process_block(&mut abandoned, &EMPTY_DEVICE_RESOURCES);
            split.rollback_block();
            let continuation = [[0.8, -0.4]; 257];
            let output = render(whole.as_mut(), &continuation, &[257]);
            assert_eq!(output, render(split.as_mut(), &continuation, &[1, 7, 53]));
            assert_ne!(output[0], [0.0; 2], "retained audio must continue");
            assert!(
                output
                    .iter()
                    .flatten()
                    .all(|sample| sample.is_finite() && sample.abs() <= ceiling + 1e-7),
                "falling ceiling violated: mode={mode} routing={routing} first={:?}",
                output[0]
            );
        }
    }
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

fn check_sample_ceiling_and_partition_independence(mode: u32, routing: u32) {
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
    // Both isolated runtimes consume the same immutable full signal. Running
    // them concurrently preserves the complete partition oracle while keeping
    // this relatively expensive 48 kHz fixture within a short feedback loop.
    let (output, partitioned) = std::thread::scope(|scope| {
        let first = scope.spawn(|| render(a.as_mut(), &input, &[input.len()]));
        let second = render(b.as_mut(), &input, &[1, 63, 257, 17]);
        (first.join().unwrap(), second)
    });
    assert_eq!(output, partitioned);
    assert!(
        output
            .iter()
            .flatten()
            .all(|v| v.is_finite() && v.abs() <= 10.0f32.powf(-6.0 / 20.0) + 1e-6),
        "mode={mode} routing={routing}"
    );
}

macro_rules! sample_ceiling_case {
    ($name:ident, $mode:expr, $routing:expr) => {
        #[test]
        fn $name() {
            check_sample_ceiling_and_partition_independence($mode, $routing);
        }
    };
}

sample_ceiling_case!(sample_ceiling_standard_lr, 0, 0);
sample_ceiling_case!(sample_ceiling_standard_ms, 0, 1);
sample_ceiling_case!(sample_ceiling_soft_lr, 1, 0);
sample_ceiling_case!(sample_ceiling_soft_ms, 1, 1);
sample_ceiling_case!(sample_ceiling_true_peak_lr, 2, 0);
sample_ceiling_case!(sample_ceiling_true_peak_ms, 2, 1);

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
fn bs1770_meter_recovers_intersample_peaks_of_a_known_signal() {
    // A quarter-rate sine offset by 45 degrees has 0.7071 sample peaks and a
    // 1.0 continuous peak; the standard 4x meter must recover it closely.
    let tone: Vec<StereoFrame> = (0..4096)
        .map(|i| {
            let value =
                (std::f64::consts::FRAC_PI_2 * i as f64 + std::f64::consts::FRAC_PI_4).sin() as f32;
            [value, -value]
        })
        .collect();
    let sample_peak = tone.iter().flatten().fold(0.0_f32, |a, s| a.max(s.abs()));
    assert!(sample_peak < 0.71);
    let true_peak = true_peak_oracle::bs1770_true_peak(&tone).unwrap();
    assert!((true_peak - 1.0).abs() < 0.03, "{true_peak}");
    assert!(true_peak_oracle::bs1770_true_peak(&[]).is_none());
    assert!(true_peak_oracle::bs1770_true_peak(&[[f32::NAN, 0.0]]).is_none());
}

/// Render a gated-burst fixture through True Peak mode at 48 and 192 kHz in
/// both routings, returning the largest BS.1770-4 meter reading.
///
/// Gated bursts make the gain move inside the 32-tap detector window. The
/// detector reconstructs the pre-gain input, so modulation-induced
/// intersample peaks are not tracked (Pulse #3379). The 0.1 criterion is the
/// standard BS.1770-4 4x meter with a recorded +0.3 dB bound; ideal
/// reconstruction reads higher and remains a documented limitation.
fn gated_burst_bs1770_peak(high: f32, low: f32) -> f64 {
    let ceiling = 10.0_f64.powf(-1.0 / 20.0);
    let bound = ceiling * 10.0_f64.powf(0.3 / 20.0);
    let mut worst = 0.0_f64;
    for rate in [48_000, 192_000] {
        for routing in [0, 1] {
            let factory = factory(
                rate,
                &[
                    (MODE, ParameterValue::Choice(2)),
                    (ROUTING, ParameterValue::Choice(routing)),
                    (LINK, ParameterValue::Float(0.0)),
                    (RELEASE, ParameterValue::Float(0.01)),
                    (CEILING, ParameterValue::Float(-1.0)),
                ],
            );
            let mut runtime = factory.create_runtime().unwrap();
            let mut input: Vec<StereoFrame> = (0..4000)
                .map(|i| {
                    let envelope = if i % 137 < 17 { 10.0 } else { 1.0 };
                    [
                        envelope * (i as f32 * high + 0.2).sin(),
                        envelope * (i as f32 * low + 0.7).sin(),
                    ]
                })
                .collect();
            input.extend(vec![[0.0; 2]; 2048]);
            let output = render(runtime.as_mut(), &input, &[1, 63, 257]);
            let sample_peak = output
                .iter()
                .flatten()
                .fold(0.0_f64, |a, s| a.max(f64::from(s.abs())));
            assert!(sample_peak <= ceiling + 1e-6, "{high} {rate} {routing}");
            let measured = true_peak_oracle::bs1770_true_peak(&output).unwrap();
            assert!(
                measured <= bound,
                "{high} {rate} {routing}: {:+.3} dB",
                20.0 * (measured / ceiling).log10()
            );
            worst = worst.max(measured);
        }
    }
    // Guard against the fixture silently becoming trivial.
    assert!(worst > ceiling * 0.99, "{worst}");
    worst
}

#[test]
fn true_peak_mode_stays_within_bs1770_bound_on_near_nyquist_bursts() {
    gated_burst_bs1770_peak(2.7, 2.1);
}

#[test]
fn true_peak_mode_stays_within_bs1770_bound_on_mid_band_bursts() {
    gated_burst_bs1770_peak(0.8, 0.6);
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

#[test]
fn reduction_meter_matches_original_factor_oracle_across_routing_and_rollback() {
    // Captured from the original per-factor dB owner at base fd44b147; each
    // value represents the same delayed sample, not independent stage peaks.
    type MeterReference = ([f32; 2], bool);
    const EXPECTED: [[[MeterReference; 2]; 6]; 2] = [
        [
            [
                ([17.97917_f32, 18.7157_f32], false),
                ([399.6043_f32, 398.4103_f32], false),
            ],
            [
                ([405.5046_f32, 399.9174_f32], false),
                ([402.7183_f32, 401.1347_f32], false),
            ],
            [
                ([18.59695_f32, 19.33349_f32], true),
                ([400.2221_f32, 399.028_f32], true),
            ],
            [
                ([404.4733_f32, 399.8142_f32], true),
                ([403.3361_f32, 400.4533_f32], true),
            ],
            [
                ([401.8966_f32, 399.7475_f32], false),
                ([400.2112_f32, 399.8473_f32], false),
            ],
            [
                ([402.8987_f32, 398.1146_f32], false),
                ([401.1973_f32, 400.0852_f32], false),
            ],
        ],
        [
            [
                ([17.97917_f32, 18.7157_f32], false),
                ([600_f32, 18.57331_f32], false),
            ],
            [
                ([606.0206_f32, 606.0206_f32], false),
                ([606.0206_f32, 606.0206_f32], false),
            ],
            [
                ([18.59695_f32, 19.33349_f32], true),
                ([601.8317_f32, 19.19109_f32], true),
            ],
            [
                ([607.8524_f32, 607.8524_f32], true),
                ([607.8524_f32, 607.8524_f32], true),
            ],
            [
                ([600_f32, 600_f32], false),
                ([600_f32, 18.90832_f32], false),
            ],
            [
                ([606.0029_f32, 606.0029_f32], false),
                ([606.0206_f32, 606.0206_f32], false),
            ],
        ],
    ];
    for (extreme, cases) in EXPECTED.iter().enumerate() {
        for (case, expected) in cases.iter().enumerate() {
            let mode = (case / 2) as u32;
            let routing = (case % 2) as u32;
            let f = factory(
                8_000,
                &[
                    (MODE, ParameterValue::Choice(mode)),
                    (ROUTING, ParameterValue::Choice(routing)),
                    (LINK, ParameterValue::Float(0.0)),
                    (LINK_MID_SIDE, ParameterValue::Float(0.0)),
                    (RELEASE, ParameterValue::Float(0.01)),
                    (CEILING, ParameterValue::Float(-1.0)),
                ],
            );
            let mut runtime = f.create_runtime().unwrap();
            for block in 0..20 {
                let mut frames: Vec<StereoFrame> = (0..64)
                    .map(|n| {
                        let i = block * 64 + n;
                        let scale = if i % 173 < 5 {
                            if extreme == 0 {
                                1e20_f32
                            } else {
                                f32::MAX / 4.0
                            }
                        } else if i % 97 < 9 {
                            8.0
                        } else {
                            1.0
                        };
                        [
                            scale * (i * 37 % 257 - 128) as f32 / 128.0,
                            scale * (i * 71 % 251 - 125) as f32 / 128.0,
                        ]
                    })
                    .collect();
                if block % 5 == 0 {
                    let mut discarded = frames.clone();
                    runtime.begin_block();
                    runtime.process_block(&mut discarded, &EMPTY_DEVICE_RESOURCES);
                    runtime.rollback_block();
                    assert_eq!(
                        runtime.take_dynamics_meter().unwrap(),
                        hex_lattice::DynamicsMeter::default()
                    );
                }
                runtime.begin_block();
                runtime.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
                runtime.commit_block();
                let actual = runtime.take_dynamics_meter().unwrap();
                if let Some(index) = [3, 19].iter().position(|&value| value == block) {
                    let (levels, clipped) = expected[index];
                    assert_eq!(actual.clipped, clipped, "case={case} block={block}");
                    for (lane, &expected_level) in levels.iter().enumerate() {
                        let tolerance = if expected_level <= 120.0 { 1e-4 } else { 1e-3 };
                        assert!(
                            (actual.reduction_db[lane] - expected_level).abs() <= tolerance,
                            "extreme={extreme} case={case} block={block} lane={lane}: actual={} expected={}",
                            actual.reduction_db[lane],
                            expected_level
                        );
                    }
                }
            }
        }
    }
}
