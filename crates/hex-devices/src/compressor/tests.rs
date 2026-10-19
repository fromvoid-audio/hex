use super::*;
use hex_lattice::{NativeDeviceRegistry, PreparedNativeProcessor};

static DEFINITIONS: [&dyn NativeDeviceDefinition; 1] = [&COMPRESSOR_DEFINITION];

fn registry() -> NativeDeviceRegistry<'static> {
    NativeDeviceRegistry::new(&DEFINITIONS).expect("valid compressor registry")
}

fn patch(state: &NativeDeviceState, id: ParameterId, value: f32) -> NativeDeviceState {
    COMPRESSOR_DEFINITION
        .patch_parameter(state, id, ParameterValue::Float(value))
        .expect("valid compressor parameter")
}

fn prepared(state: &NativeDeviceState, sample_rate: u32) -> Box<dyn PreparedNativeProcessor> {
    COMPRESSOR_DEFINITION
        .prepare_processor(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("valid compressor preparation")
        .expect("stateful processor")
}

fn runtime(state: &NativeDeviceState, sample_rate: u32) -> Box<dyn NativeProcessorRuntime> {
    prepared(state, sample_rate).create_runtime()
}

#[test]
fn defaults_and_generic_codec_roundtrip_all_seven_parameters() {
    let registry = registry();
    let state = registry
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("compressor defaults");
    assert_eq!(state.state_version(), COMPRESSOR_STATE_VERSION);
    assert_eq!(state.payload().len(), STATE_BYTES);
    assert_eq!(registry.migrate_state(&state).expect("migration"), state);
    for (id, expected) in [
        (parameter::THRESHOLD, -18.0),
        (parameter::RATIO, 4.0),
        (parameter::ATTACK, 10.0),
        (parameter::RELEASE, 100.0),
        (parameter::KNEE, 6.0),
        (parameter::MAKEUP, 0.0),
        (parameter::DRY_WET, 1.0),
    ] {
        assert_eq!(
            COMPRESSOR_DEFINITION.read_parameter(&state, id),
            Ok(ParameterValue::Float(expected))
        );
    }
    assert!(
        DESCRIPTORS
            .iter()
            .all(|descriptor| descriptor.validate_descriptor().is_ok())
    );
}

#[test]
fn malformed_states_parameters_and_sample_rates_are_rejected() {
    let registry = registry();
    let state = registry
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("compressor defaults");
    for (id, value) in [
        (parameter::THRESHOLD, f32::NAN),
        (parameter::RATIO, 0.99),
        (parameter::RATIO, 20.01),
        (parameter::ATTACK, 0.09),
        (parameter::RELEASE, 1_001.0),
        (parameter::KNEE, -0.01),
        (parameter::MAKEUP, 24.01),
    ] {
        assert!(
            registry
                .patch_parameter(&state, id, ParameterValue::Float(value))
                .is_err()
        );
    }
    let wrong_type = NativeDeviceState::new(
        DeviceTypeId::from_static("hex.utility"),
        1,
        state.payload().to_vec(),
    )
    .expect("bounded wrong type");
    assert_eq!(
        registry.migrate_state(&wrong_type),
        Err(DeviceContractError::WrongDeviceType)
    );
    let future = NativeDeviceState::new(
        COMPRESSOR_TYPE_ID,
        COMPRESSOR_STATE_VERSION + 1,
        state.payload().to_vec(),
    )
    .expect("future version");
    assert_eq!(
        registry.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    for length in [STATE_BYTES - 1, STATE_BYTES + 1] {
        let malformed = NativeDeviceState::new(
            COMPRESSOR_TYPE_ID,
            COMPRESSOR_STATE_VERSION,
            vec![0; length],
        )
        .expect("bounded malformed state");
        assert_eq!(
            registry.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }
    let mut invalid_number = state.payload().to_vec();
    invalid_number[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
    let malformed =
        NativeDeviceState::new(COMPRESSOR_TYPE_ID, COMPRESSOR_STATE_VERSION, invalid_number)
            .expect("bounded non-finite state");
    assert_eq!(
        registry.migrate_state(&malformed),
        Err(DeviceContractError::InvalidParameterValue)
    );
    for sample_rate in [0, 7_999, 192_001] {
        assert!(
            COMPRESSOR_DEFINITION
                .prepare_processor(
                    &state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn knee_law_hits_independent_known_points_and_is_continuous() {
    // With ratio 4, the hard-knee slope above threshold is -3/4 dB/dB.
    assert_eq!(gain_reduction_db(-24.0, -18.0, 4.0, 0.0), 0.0);
    assert_eq!(gain_reduction_db(-18.0, -18.0, 4.0, 0.0), 0.0);
    assert_eq!(gain_reduction_db(-14.0, -18.0, 4.0, 0.0), -3.0);
    assert_eq!(gain_reduction_db(-18.0, -18.0, 1.0, 0.0), 0.0);

    // A 6 dB knee spans -21 to -15 dB around the -18 dB threshold.
    assert_eq!(gain_reduction_db(-21.0, -18.0, 4.0, 6.0), 0.0);
    assert_eq!(gain_reduction_db(-15.0, -18.0, 4.0, 6.0), -2.25);
    assert_eq!(gain_reduction_db(-18.0, -18.0, 4.0, 6.0), -0.5625);
    let epsilon = 1.0e-7;
    let left_at_start = gain_reduction_db(-21.0 - epsilon, -18.0, 4.0, 6.0);
    let right_at_start = gain_reduction_db(-21.0 + epsilon, -18.0, 4.0, 6.0);
    let left_at_end = gain_reduction_db(-15.0 - epsilon, -18.0, 4.0, 6.0);
    let right_at_end = gain_reduction_db(-15.0 + epsilon, -18.0, 4.0, 6.0);
    assert!(left_at_start.abs() < 1.0e-12);
    assert!((right_at_start - left_at_start).abs() < 1.0e-6);
    assert!((right_at_end - left_at_end).abs() < 1.0e-6);
}

#[test]
fn one_sample_step_matches_an_independent_detector_and_gain_smoothing_calculation() {
    let state = registry()
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("defaults");
    let state = patch(&state, parameter::ATTACK, 0.1);
    let state = COMPRESSOR_DEFINITION
        .patch_parameter(&state, parameter::KNEE, ParameterValue::Float(0.0))
        .expect("hard knee");
    let mut runtime = runtime(&state, 48_000);
    let mut frame = [[1.0, -0.25]];

    let detector_alpha = (-1.0_f64 / 4.8).exp();
    let envelope = 1.0 - detector_alpha;
    let level_db = 20.0 * envelope.log10();
    let reduction_db = if level_db > -18.0 {
        (1.0 / 4.0 - 1.0) * (level_db + 18.0)
    } else {
        0.0
    };
    let gain_alpha = (-1.0_f64 / 240.0).exp();
    let expected_gain_db = (1.0 - gain_alpha) * reduction_db;
    let expected_gain = 10.0_f64.powf(expected_gain_db / 20.0);
    runtime.process_block(&mut frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!((f64::from(frame[0][0]) - expected_gain).abs() < 1.0e-7);
    assert!((f64::from(frame[0][1]) + 0.25 * expected_gain).abs() < 1.0e-7);
}

#[test]
fn linked_peak_detector_preserves_stereo_balance_and_ratio_one_is_uncompressed() {
    let defaults = registry()
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("defaults");
    let mut linked = runtime(&defaults, 48_000);
    let mut frame = [[0.75, -0.1875]];
    linked.process_block(&mut frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!((f64::from(frame[0][0]) / f64::from(frame[0][1]) + 4.0).abs() < 1.0e-6);

    let ratio_one = patch(&defaults, parameter::RATIO, 1.0);
    let mut identity = runtime(&ratio_one, 48_000);
    let mut frames = [[0.75, -0.1875], [0.5, -0.125]];
    let original = frames;
    identity.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(frames, original);
}

#[test]
fn release_and_makeup_controls_change_the_audible_envelope() {
    let defaults = registry()
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("defaults");
    let fast_release = patch(&defaults, parameter::RELEASE, 5.0);
    let slow_release = patch(&defaults, parameter::RELEASE, 1_000.0);
    let mut fast = runtime(&fast_release, 48_000);
    let mut slow = runtime(&slow_release, 48_000);
    let mut loud_fast = [[1.0, 0.0]; 200];
    let mut loud_slow = loud_fast;
    fast.process_block(&mut loud_fast, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    slow.process_block(&mut loud_slow, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut quiet_fast = [[0.1, 0.0]];
    let mut quiet_slow = quiet_fast;
    fast.process_block(&mut quiet_fast, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    slow.process_block(&mut quiet_slow, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_ne!(quiet_fast, quiet_slow);

    let ratio_one = patch(&defaults, parameter::RATIO, 1.0);
    let makeup = patch(&ratio_one, parameter::MAKEUP, 6.0);
    let mut boosted = runtime(&makeup, 48_000);
    let mut frames = [[0.25, -0.125]; 256];
    let original = frames;
    boosted.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(frames.last().unwrap()[0] > original[0][0]);
}

#[test]
fn reset_partitioning_and_bypass_preserve_the_fixed_runtime_lifecycle() {
    let defaults = registry()
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("defaults");
    let mut whole = runtime(&defaults, 48_000);
    let mut partitioned = runtime(&defaults, 48_000);
    let mut whole_frames = [[0.9, -0.3]; 257];
    let mut partition_frames = whole_frames;
    whole.process_block(&mut whole_frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    for chunk in partition_frames.chunks_mut(31) {
        partitioned.process_block(chunk, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    }
    assert_eq!(whole_frames, partition_frames);

    let mut bypassed = runtime(&defaults, 48_000);
    let mut active = runtime(&defaults, 48_000);
    let mut prime = [[0.9, -0.3]; 80];
    bypassed.process_block(&mut prime, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut prime = [[0.9, -0.3]; 80];
    active.process_block(&mut prime, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut untouched = [[0.6, -0.2]; 8];
    let original = untouched;
    bypassed.process_bypassed_block(&mut untouched, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(untouched, original);
    let mut active_signal = [[0.6, -0.2]; 8];
    active.process_block(&mut active_signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut next_a = [[0.4, 0.1]; 1];
    let mut next_b = next_a;
    bypassed.process_block(&mut next_a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    active.process_block(&mut next_b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(next_a, next_b);
    assert!(bypassed.is_quiescent());

    let mut zero_after_signal = [[0.0, 0.0]; 16];
    bypassed.process_block(&mut zero_after_signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(zero_after_signal.iter().all(|frame| *frame == [0.0, 0.0]));
    assert!(bypassed.is_quiescent());

    let mut reset = runtime(&defaults, 48_000);
    let mut fresh = runtime(&defaults, 48_000);
    let mut history = [[1.0, 0.0]; 200];
    reset.process_block(&mut history, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    reset.reset();
    let mut after_reset = [[0.5, -0.25]; 1];
    let mut after_fresh = after_reset;
    reset.process_block(&mut after_reset, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    fresh.process_block(&mut after_fresh, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(after_reset, after_fresh);
}

#[test]
fn compatible_retarget_copies_only_history_and_rejects_a_different_rate() {
    let defaults = registry()
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("defaults");
    let updated = patch(&defaults, parameter::THRESHOLD, -30.0);
    let old = prepared(&defaults, 48_000).create_runtime();
    let mut old = old;
    let mut signal = [[0.9, -0.4]; 96];
    old.process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let new_prepared = prepared(&updated, 48_000);
    let mut candidate = new_prepared.create_runtime();
    assert!(candidate.can_retarget_parameters_from(old.as_ref()));
    assert!(candidate.can_copy_history_from(old.as_ref()));
    candidate.copy_history_from(old.as_ref());
    let runtime = candidate
        .as_any()
        .downcast_ref::<CompressorRuntime>()
        .expect("compressor candidate");
    assert_eq!(runtime.settings.threshold_db, -30.0);
    assert_ne!(runtime.envelope, 0.0);

    let wrong_rate = prepared(&updated, 44_100).create_runtime();
    assert!(!wrong_rate.can_retarget_parameters_from(old.as_ref()));
    assert!(!wrong_rate.can_copy_history_from(old.as_ref()));
}

#[test]
fn invalid_frames_are_visible_and_do_not_poison_valid_following_history() {
    let state = registry()
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("defaults");
    let mut invalid_then_valid = runtime(&state, 48_000);
    let mut valid_only = runtime(&state, 48_000);
    let mut invalid = [[f32::NAN, f32::INFINITY]];
    invalid_then_valid.process_block(&mut invalid, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(invalid[0][0].is_nan());
    assert_eq!(invalid[0][1], f32::INFINITY);
    let mut a = [[0.5, -0.25]];
    let mut b = a;
    invalid_then_valid.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    valid_only.process_block(&mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);
}

#[test]
fn static_transfer_preview_matches_known_knee_anchors_and_validates_state() {
    let defaults = COMPRESSOR_DEFINITION.default_state().expect("defaults");
    let hard = patch(
        &patch(&defaults, parameter::THRESHOLD, -30.0),
        parameter::KNEE,
        0.0,
    );
    let hard_curve = transfer_curve(&hard).expect("hard curve");
    for (index, output_db) in [(0, -60.0), (64, -30.0), (128, -22.5)] {
        assert!((f64::from(hard_curve.points[index][1]) * 90.0 - 66.0 - output_db).abs() < 3.0e-6);
    }
    let soft = patch(&hard, parameter::KNEE, 12.0);
    let soft_curve = transfer_curve(&soft).expect("soft curve");
    assert!((f64::from(soft_curve.points[64][1]) * 90.0 - 66.0 + 31.125).abs() < 3.0e-6);
    let unity = patch(
        &patch(&soft, parameter::RATIO, 1.0),
        parameter::MAKEUP,
        24.0,
    );
    let curve = transfer_curve(&unity).expect("boosted unity curve");
    assert_eq!(curve.points[128], [1.0, 1.0]);
    for [x, y] in curve.points {
        assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
    }
    let extreme = patch(
        &patch(
            &patch(&defaults, parameter::THRESHOLD, -60.0),
            parameter::RATIO,
            20.0,
        ),
        parameter::KNEE,
        12.0,
    );
    let extreme_curve = transfer_curve(&extreme).expect("lowest threshold knee");
    assert!((f64::from(extreme_curve.points[0][1]) * 90.0 - 66.0 + 61.425).abs() < 3.0e-6);
    assert!(
        extreme_curve
            .points
            .iter()
            .all(|[x, y]| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y))
    );
    let malformed = NativeDeviceState::new(COMPRESSOR_TYPE_ID, COMPRESSOR_STATE_VERSION, vec![0])
        .expect("state");
    assert_eq!(
        transfer_curve(&malformed),
        Err(DeviceContractError::MalformedState)
    );
}

#[test]
fn parallel_mix_uses_original_dry_and_post_makeup_wet() {
    let defaults = COMPRESSOR_DEFINITION.default_state().unwrap();
    let makeup = patch(&defaults, parameter::MAKEUP, 12.0);
    let input = [[0.9, -0.23], [0.1, 0.7], [-0.4, 0.03], [-0.0, 0.0]];
    for mix in [0.0, 0.25, 0.75, 1.0] {
        let state = patch(&makeup, ParameterId::from_static(7), mix);
        let mut wet = input;
        runtime(&makeup, 48_000).process_block(&mut wet, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut actual = input;
        runtime(&state, 48_000).process_block(&mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        for (index, frame) in actual.iter().enumerate() {
            for (channel, sample) in frame.iter().enumerate() {
                let expected = if mix == 0.0 {
                    input[index][channel]
                } else if mix == 1.0 {
                    wet[index][channel]
                } else {
                    (f64::from(input[index][channel]) * (1.0 - f64::from(mix))
                        + f64::from(wet[index][channel]) * f64::from(mix))
                        as f32
                };
                assert!((*sample - expected).abs() < 2.0e-7);
                if mix == 0.0 || mix == 1.0 {
                    assert_eq!(sample.to_bits(), expected.to_bits());
                }
            }
        }
    }
}

#[test]
fn default_wet_preserves_original_asymmetric_pcm_bitwise() {
    let defaults = COMPRESSOR_DEFINITION.default_state().unwrap();
    for (rate, expected) in [
        (8000, 0xcf6d824693b1be4f_u64),
        (22050, 0xfe0ada58f8009c0c_u64),
        (44100, 0x513115d068c7dd07_u64),
        (48000, 0x877a97d4e417b766_u64),
        (96000, 0xaec3c632695233f7_u64),
        (192000, 0xb575c8d16138de5_u64),
    ] {
        let mut signal: Vec<_> = (0..2049)
            .map(|n| {
                let value = ((n * 97 % 521) as f32 - 260.0) / 270.0;
                [value, value * -0.173 + (n % 7) as f32 * 0.013]
            })
            .collect();
        runtime(&defaults, rate).process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut hash = 0xcbf29ce484222325_u64;
        for value in signal.iter().flatten() {
            for byte in value.to_bits().to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        assert_eq!(hash, expected, "pre-mix fully-wet PCM at {rate}");
    }
}

#[test]
fn mix_codec_rejects_legacy_nonfinite_and_preserves_host_metadata() {
    let registry = registry();
    let original = COMPRESSOR_DEFINITION
        .default_state()
        .unwrap()
        .with_input_gain_millidb(-3210)
        .unwrap()
        .with_output_gain_millidb(1234)
        .unwrap();
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.001, 1.001] {
        assert!(
            registry
                .patch_parameter(&original, parameter::DRY_WET, ParameterValue::Float(value))
                .is_err()
        );
    }
    assert!(
        registry
            .patch_parameter(&original, parameter::DRY_WET, ParameterValue::Bool(true))
            .is_err()
    );
    let legacy = NativeDeviceState::new(COMPRESSOR_TYPE_ID, 1, vec![0; 24]).unwrap();
    assert_eq!(
        registry.migrate_state(&legacy),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 1.1] {
        let mut bytes = original.payload().to_vec();
        bytes[24..28].copy_from_slice(&value.to_le_bytes());
        let invalid =
            NativeDeviceState::new(COMPRESSOR_TYPE_ID, COMPRESSOR_STATE_VERSION, bytes).unwrap();
        assert_eq!(
            registry.migrate_state(&invalid),
            Err(DeviceContractError::InvalidParameterValue)
        );
    }
    let changed = registry
        .patch_parameter(&original, parameter::DRY_WET, ParameterValue::Float(0.375))
        .unwrap();
    assert_eq!(changed.input_gain_millidb(), -3210);
    assert_eq!(changed.output_gain_millidb(), 1234);
    assert_eq!(registry.migrate_state(&changed).unwrap(), changed);
    assert_eq!(
        COMPRESSOR_DEFINITION
            .read_parameter(&changed, parameter::DRY_WET)
            .unwrap(),
        ParameterValue::Float(0.375)
    );
    // Independently owned opaque states can represent either generic A/B payload;
    // patching one never mutates the other. No bank owner is added to the provider.
    assert_eq!(
        COMPRESSOR_DEFINITION
            .read_parameter(&original, parameter::DRY_WET)
            .unwrap(),
        ParameterValue::Float(1.0)
    );
}

#[test]
fn mixed_static_curve_blends_after_makeup_in_linear_amplitude() {
    let original = patch(
        &patch(
            &COMPRESSOR_DEFINITION.default_state().unwrap(),
            parameter::RATIO,
            1.0,
        ),
        parameter::MAKEUP,
        12.0,
    );
    for mix in [0.0, 0.25, 0.75, 1.0] {
        let state = patch(&original, parameter::DRY_WET, mix);
        let curve = transfer_curve(&state).unwrap();
        let gain_db =
            20.0 * ((1.0 - f64::from(mix)) + f64::from(mix) * 10.0_f64.powf(12.0 / 20.0)).log10();
        for (index, [_, y]) in curve.points.iter().enumerate() {
            let input_db = -60.0 + index as f64 / 128.0 * 60.0;
            assert!((f64::from(*y) * 90.0 - 66.0 - input_db - gain_db).abs() < 4.0e-6);
        }
    }
}

#[test]
fn mix_edits_ramp_to_exact_endpoints_and_twin_copies_do_not_restart_it() {
    for rate in [8_000_u32, 22_050, 44_100, 48_000, 96_000, 192_000] {
        let defaults = COMPRESSOR_DEFINITION.default_state().unwrap();
        let unity = patch(
            &patch(&defaults, parameter::RATIO, 1.0),
            parameter::MAKEUP,
            12.0,
        );
        for (from, to) in [(0.0, 1.0), (1.0, 0.0), (0.2, 0.7)] {
            let old = runtime(&patch(&unity, parameter::DRY_WET, from), rate);
            let state = patch(&unity, parameter::DRY_WET, to);
            let mut whole = runtime(&state, rate);
            let mut partitioned = runtime(&state, rate);
            whole.copy_history_from(old.as_ref());
            partitioned.copy_history_from(old.as_ref());
            let ramp = rate.div_ceil(200) as usize;
            let input = [0.25, -0.125];
            let mut a = vec![input; ramp + 17];
            let mut b = a.clone();
            whole.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let mut start = 0;
            for length in [1, 7, 31, 2, 503].into_iter().cycle() {
                if start == b.len() {
                    break;
                }
                let end = (start + length).min(b.len());
                // Same-settings scratch runtime copied for each speculative block.
                let mut twin = runtime(&state, rate);
                twin.copy_history_from(partitioned.as_ref());
                twin.process_block(&mut b[start..end], &hex_lattice::EMPTY_DEVICE_RESOURCES);
                partitioned = twin;
                start = end;
            }
            assert_eq!(a, b, "partition/twin agreement at {rate}");
            let wet_gain = 10.0_f64.powf(12.0 / 20.0);
            for (index, actual) in a.iter().enumerate() {
                let progress = ((index + 1) as f64 / ramp as f64).min(1.0);
                let mix = f64::from(from) + (f64::from(to) - f64::from(from)) * progress;
                let gain = (1.0 - mix) + mix * wet_gain;
                for (channel, sample) in actual.iter().enumerate() {
                    assert!((f64::from(*sample) - f64::from(input[channel]) * gain).abs() < 2.0e-7);
                }
            }
            if to == 0.0 {
                assert_eq!(a[ramp], input, "finite ramp reaches exact dry endpoint");
            }
            // An abandoned candidate must not alter its committed source.
            let source = old.as_any().downcast_ref::<CompressorRuntime>().unwrap();
            assert_eq!(source.mix, f64::from(from));
            assert_eq!(source.mix_remaining, 0);
            whole.reset();
            let mut reset = [input];
            let mut fresh = reset;
            whole.process_block(&mut reset, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            runtime(&state, rate).process_block(&mut fresh, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert_eq!(reset, fresh);
        }
    }
}

#[test]
fn fully_dry_and_bypassed_mix_edits_still_advance_detector_and_smoothing() {
    let state = patch(
        &COMPRESSOR_DEFINITION.default_state().unwrap(),
        parameter::MAKEUP,
        9.0,
    );
    let dry = patch(&state, parameter::DRY_WET, 0.0);
    let mut wet_runtime = runtime(&state, 48_000);
    let mut dry_runtime = runtime(&dry, 48_000);
    let signal = [[0.9, -0.11]; 337];
    let mut a = signal;
    let mut b = signal;
    wet_runtime.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    dry_runtime.process_block(&mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(b, signal);
    let w = wet_runtime
        .as_any()
        .downcast_ref::<CompressorRuntime>()
        .unwrap();
    let d = dry_runtime
        .as_any()
        .downcast_ref::<CompressorRuntime>()
        .unwrap();
    assert_eq!((w.envelope, w.gain_db), (d.envelope, d.gain_db));
    let mut active = runtime(&dry, 48_000);
    let mut bypassed = runtime(&dry, 48_000);
    active.copy_history_from(wet_runtime.as_ref());
    bypassed.copy_history_from(wet_runtime.as_ref());
    let mut x = signal;
    let mut y = signal;
    active.process_block(&mut x, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    bypassed.process_bypassed_block(&mut y, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(y, signal);
    let mut next = [[-0.3, 0.07]; 23];
    let mut other = next;
    active.process_block(&mut next, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    bypassed.process_block(&mut other, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(next, other);
    assert_eq!(next, [[-0.3, 0.07]; 23]);
}
