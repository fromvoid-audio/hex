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
fn defaults_and_generic_codec_roundtrip_seven_floats_and_mode() {
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
    // v1 decodes (crate::state_migration_tests); its values are still validated.
    let legacy = NativeDeviceState::new(COMPRESSOR_TYPE_ID, 1, vec![0; 24]).unwrap();
    assert_eq!(
        registry.migrate_state(&legacy),
        Err(DeviceContractError::InvalidParameterValue)
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

#[test]
fn rms_choice_reduces_actual_short_burst_compression() {
    let base = patch(
        &patch(
            &patch(
                &COMPRESSOR_DEFINITION.default_state().unwrap(),
                parameter::ATTACK,
                0.1,
            ),
            parameter::THRESHOLD,
            -30.0,
        ),
        parameter::KNEE,
        0.0,
    );
    let rms = COMPRESSOR_DEFINITION
        .patch_parameter(
            &base,
            ParameterId::from_static(8),
            ParameterValue::Choice(1),
        )
        .expect("select actual RMS");
    let mut peak_signal = [[1.0, -0.25]; 48];
    let mut rms_signal = peak_signal;
    runtime(&base, 48000).process_block(&mut peak_signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    runtime(&rms, 48000).process_block(&mut rms_signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(
        rms_signal[47][0] > peak_signal[47][0] + 0.01,
        "RMS really changes transient gain"
    );
}

fn mode(state: &NativeDeviceState, value: u32) -> NativeDeviceState {
    registry()
        .patch_parameter(state, parameter::DETECTOR, ParameterValue::Choice(value))
        .unwrap()
}
fn history(value: &dyn NativeProcessorRuntime) -> &CompressorRuntime {
    value.as_any().downcast_ref::<CompressorRuntime>().unwrap()
}

#[test]
fn rms_energy_matches_dc_impulse_and_burst_closed_forms_at_six_rates() {
    let state = mode(
        &patch(
            &COMPRESSOR_DEFINITION.default_state().unwrap(),
            parameter::DRY_WET,
            0.0,
        ),
        1,
    );
    for rate in [8000_u32, 22050, 44100, 48000, 96000, 192000] {
        let a = (-100.0 / f64::from(rate)).exp();
        let n = rate as usize / 100;
        let mut dc = runtime(&state, rate);
        dc.process_block(
            &mut vec![[0.5, -0.25]; n],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let expected = 0.25 * (1.0 - a.powi(n as i32));
        assert!((history(dc.as_ref()).energy[0] - expected).abs() < 2e-14);
        assert!((history(dc.as_ref()).energy[1] - expected / 4.0).abs() < 2e-14);
        let mut impulse = runtime(&state, rate);
        impulse.process_block(&mut [[0.5, -0.25]], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        impulse.process_block(&mut vec![[0.0; 2]; n], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(
            (history(impulse.as_ref()).energy[0] - (1.0 - a) * 0.25 * a.powi(n as i32)).abs()
                < 2e-14
        );
        dc.process_block(&mut vec![[0.0; 2]; n], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!((history(dc.as_ref()).energy[0] - expected * a.powi(n as i32)).abs() < 2e-14);
        assert_eq!(history(dc.as_ref()).detector_mix, 1.0);
    }
}

#[test]
fn rms_periodic_sine_uses_mean_square_not_mean_absolute_and_preserves_phase_ripple() {
    let state = mode(
        &patch(
            &COMPRESSOR_DEFINITION.default_state().unwrap(),
            parameter::DRY_WET,
            0.0,
        ),
        1,
    );
    let cycle: Vec<f32> = (0..48)
        .map(|i| (0.5 * (std::f64::consts::TAU * i as f64 / 48.0).sin()) as f32)
        .collect();
    for rate in [8000_u32, 22050, 44100, 48000, 96000, 192000] {
        let a = (-100.0 / f64::from(rate)).exp();
        let mut pan = runtime(&state, rate);
        let count = (rate as usize / 2).div_ceil(48) * 48;
        let mut warm: Vec<_> = (0..count)
            .map(|i| [cycle[i % 48], -cycle[i % 48]])
            .collect();
        pan.process_block(&mut warm, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut sum = 0.0;
        for phase in 0..48 {
            pan.process_block(
                &mut [[cycle[phase], -cycle[phase]]],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            // Closed periodic convolution; independent of the streaming implementation.
            let expected = (1.0 - a) / (1.0 - a.powi(48))
                * (0..48)
                    .map(|k| a.powi(k as i32) * f64::from(cycle[(phase + 48 - k) % 48]).powi(2))
                    .sum::<f64>();
            let e = history(pan.as_ref()).energy;
            assert!((e[0] - expected).abs() < 2e-13, "rate{rate} phase{phase}");
            assert_eq!(e[0], e[1]);
            sum += e[0];
        }
        assert!(
            (sum / 48.0 - 0.125).abs() < 1e-8,
            "sine mean energy, not squared mean absolute"
        );
    }
}

#[test]
fn rms_links_max_after_channel_averaging_and_not_instantaneous_max() {
    let state = mode(
        &patch(
            &COMPRESSOR_DEFINITION.default_state().unwrap(),
            parameter::DRY_WET,
            0.0,
        ),
        1,
    );
    let mut alternating = runtime(&state, 48000);
    let mut input: Vec<_> = (0..24000)
        .map(|i| if i % 2 == 0 { [1.0, 0.0] } else { [0.0, -1.0] })
        .collect();
    alternating.process_block(&mut input, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let e = history(alternating.as_ref()).energy;
    let a = (-100.0_f64 / 48000.0).exp();
    assert!((e[0] - a / (1.0 + a)).abs() < 1e-13);
    assert!((e[1] - 1.0 / (1.0 + a)).abs() < 1e-13);
    assert!(
        e[0].max(e[1]) < 0.501,
        "max before averaging would incorrectly give1"
    );
    let mut left = runtime(&state, 48000);
    let mut dual = runtime(&state, 48000);
    left.process_block(
        &mut [[0.8, 0.0]; 1000],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    dual.process_block(
        &mut [[-0.8, 0.8]; 1000],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(
        history(left.as_ref()).envelope,
        history(dual.as_ref()).envelope
    );
    assert_eq!(
        history(left.as_ref()).gain_db,
        history(dual.as_ref()).gain_db
    );
}

#[test]
fn rms_mode_history_ramps_survive_partitions_bypass_rapid_retarget_and_reset() {
    for rate in [8000_u32, 22050, 44100, 48000, 96000, 192000] {
        let base = patch(
            &COMPRESSOR_DEFINITION.default_state().unwrap(),
            parameter::DRY_WET,
            0.0,
        );
        let changed = mode(&patch(&base, parameter::DRY_WET, 0.75), 1);
        let mut committed = runtime(&base, rate);
        committed.process_block(
            &mut [[0.7, -0.23]; 1021],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let count = rate.div_ceil(200) as usize;
        let mut whole = runtime(&changed, rate);
        whole.copy_history_from(committed.as_ref());
        let mut split = runtime(&changed, rate);
        split.copy_history_from(committed.as_ref());
        let energy = history(committed.as_ref()).energy;
        assert_eq!(
            history(whole.as_ref()).energy,
            energy,
            "warm energy survives Peak→RMS"
        );
        let mut a = vec![[0.8, -0.27]; count + 13];
        let mut b = a.clone();
        whole.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut start = 0;
        for len in [1, 7, 31, 2, 503].into_iter().cycle() {
            if start == b.len() {
                break;
            }
            let end = (start + len).min(b.len());
            let mut twin = runtime(&changed, rate);
            twin.copy_history_from(split.as_ref());
            twin.process_block(&mut b[start..end], &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert!(
                (history(twin.as_ref()).detector_mix - ((end as f64 / count as f64).min(1.0)))
                    .abs()
                    < 3e-14
            );
            split = twin;
            start = end;
        }
        assert_eq!(a, b);
        assert_eq!(
            history(whole.as_ref()).energy,
            history(split.as_ref()).energy
        );
        assert_eq!(history(whole.as_ref()).detector_remaining, 0);
        assert_eq!(
            history(committed.as_ref()).energy,
            energy,
            "scratch did not mutate committed"
        );
        let mut bypass = runtime(&changed, rate);
        bypass.copy_history_from(committed.as_ref());
        let mut dry = vec![[0.8, -0.27]; count + 13];
        bypass.process_bypassed_block(&mut dry, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(dry.iter().all(|s| *s == [0.8, -0.27]));
        let mut next = [[0.4, -0.2]; 23];
        let mut other = next;
        bypass.process_block(&mut next, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        whole.process_block(&mut other, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(next, other);
        let mut partial = runtime(&changed, rate);
        partial.copy_history_from(committed.as_ref());
        partial.process_block(&mut [[0.8, -0.2]], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let previous = history(partial.as_ref());
        let fraction = previous.detector_mix;
        let mut reverse = runtime(&base, rate);
        reverse.copy_history_from(partial.as_ref());
        assert_eq!(history(reverse.as_ref()).detector_mix, fraction);
        assert_eq!(history(reverse.as_ref()).detector_remaining, count as u32);
        reverse.process_block(
            &mut vec![[0.8, -0.2]; count],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        assert_eq!(history(reverse.as_ref()).detector_mix, 0.0);
        split.reset();
        assert_eq!(history(split.as_ref()).energy, [0.0; 2]);
        assert_eq!(history(split.as_ref()).detector_mix, 1.0);
        let mut reset = [[0.8, -0.2]; 19];
        let mut fresh = reset;
        split.process_block(&mut reset, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        runtime(&changed, rate).process_block(&mut fresh, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(reset, fresh);
    }
}

#[test]
fn rms_state_validation_invalid_frames_and_static_detector_preview_are_truthful() {
    let base = COMPRESSOR_DEFINITION.default_state().unwrap();
    assert_eq!(base.payload().len(), 32);
    assert_eq!(
        COMPRESSOR_DEFINITION
            .read_parameter(&base, parameter::DETECTOR)
            .unwrap(),
        ParameterValue::Choice(0)
    );
    for value in [
        ParameterValue::Choice(2),
        ParameterValue::Choice(u32::MAX),
        ParameterValue::Float(f32::NAN),
        ParameterValue::Float(1.0),
        ParameterValue::Bool(true),
    ] {
        assert!(
            registry()
                .patch_parameter(&base, parameter::DETECTOR, value)
                .is_err()
        );
    }
    // Older versions decode (crate::state_migration_tests); zero values stay invalid.
    for (version, len) in [(1, 24), (2, 28)] {
        assert_eq!(
            registry().migrate_state(
                &NativeDeviceState::new(COMPRESSOR_TYPE_ID, version, vec![0; len]).unwrap()
            ),
            Err(DeviceContractError::InvalidParameterValue)
        );
    }
    let rms = mode(&base, 1);
    assert_eq!(registry().migrate_state(&rms).unwrap(), rms);
    assert_eq!(
        transfer_curve(&base).unwrap(),
        transfer_curve(&rms).unwrap(),
        "same steady detector-level curve; no fictitious3dBshift"
    );
    let mut corrupt = rms.payload().to_vec();
    corrupt[28..32].copy_from_slice(&2_u32.to_le_bytes());
    assert!(
        transfer_curve(&NativeDeviceState::new(COMPRESSOR_TYPE_ID, 3, corrupt).unwrap()).is_err()
    );
    let mut pan = runtime(&patch(&rms, parameter::DRY_WET, 0.0), 48000);
    pan.process_block(&mut [[0.8, -0.2]; 9], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let before = history(pan.as_ref());
    let saved = (
        before.energy,
        before.envelope,
        before.gain_db,
        before.detector_mix,
        before.detector_remaining,
    );
    for frame in [[f32::NAN, 1.0], [1.0, f32::INFINITY]] {
        pan.process_block(&mut [frame], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let h = history(pan.as_ref());
        assert_eq!(
            (
                h.energy,
                h.envelope,
                h.gain_db,
                h.detector_mix,
                h.detector_remaining
            ),
            saved
        );
    }
    for frame in [[f32::MAX, -f32::MAX], [f32::from_bits(1), 0.0], [0.0, 0.0]] {
        let mut sample = [frame];
        pan.process_block(&mut sample, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(sample[0], frame, "dry exact");
        let h = history(pan.as_ref());
        assert!(
            h.energy.iter().all(|v| v.is_finite())
                && h.envelope.is_finite()
                && h.gain_db.is_finite()
        );
    }
}

#[test]
fn rms_pcm_matches_analytic_dc_burst_energy_then_original_attack_release_and_makeup_mix() {
    for rate in [8000_u32, 22050, 44100, 48000, 96000, 192000] {
        for (attack, release) in [(0.1_f32, 5.0_f32), (100.0, 1000.0)] {
            let mut state = mode(&COMPRESSOR_DEFINITION.default_state().unwrap(), 1);
            for (id, value) in [
                (parameter::THRESHOLD, -24.0),
                (parameter::KNEE, 0.0),
                (parameter::MAKEUP, 6.0),
                (parameter::DRY_WET, 0.375),
                (parameter::ATTACK, attack),
                (parameter::RELEASE, release),
            ] {
                state = patch(&state, id, value);
            }
            let mut actual = runtime(&state, rate);
            let a = (-100.0 / f64::from(rate)).exp();
            let attack_a = (-1000.0 / (f64::from(attack) * f64::from(rate))).exp();
            let release_a = (-1000.0 / (f64::from(release) * f64::from(rate))).exp();
            let gain_a = (-200.0 / f64::from(rate)).exp();
            let burst = rate as usize / 100;
            let mut env = 0.0;
            let mut gain_db = 6.0;
            for n in 1..=2 * burst {
                let e = if n <= burst {
                    0.25 * (1.0 - a.powi(n as i32))
                } else {
                    0.25 * (1.0 - a.powi(burst as i32)) * a.powi((n - burst) as i32)
                };
                let detector = e.sqrt();
                let coefficient = if detector > env { attack_a } else { release_a };
                env = coefficient * env + (1.0 - coefficient) * detector;
                let level = if env > 0.0 {
                    20.0 * env.log10()
                } else {
                    -120.0
                };
                let target = 6.0 - 0.75 * (level + 24.0).max(0.0);
                gain_db = gain_a * gain_db + (1.0 - gain_a) * target;
                let input = if n <= burst { [0.5, -0.25] } else { [0.0, 0.0] };
                let mut frame = [input];
                actual.process_block(&mut frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                let h = history(actual.as_ref());
                assert!((h.envelope - env).abs() < 2e-12);
                assert!((h.gain_db - gain_db).abs() < 2e-9);
                for c in 0..2 {
                    let x = f64::from(input[c]);
                    let expected = x * 0.625 + x * 10.0_f64.powf(gain_db / 20.0) * 0.375;
                    assert!(
                        (f64::from(frame[0][c]) - expected).abs() < 1e-7,
                        "actual PCM rate{rate} frame{n}"
                    );
                }
            }
        }
    }
}

#[test]
fn rms_quadrature_channels_have_independently_averaged_energy_and_choice_preserves_host_metadata() {
    let base = COMPRESSOR_DEFINITION
        .default_state()
        .unwrap()
        .with_input_gain_millidb(-3210)
        .unwrap()
        .with_output_gain_millidb(1234)
        .unwrap();
    let state = mode(&base, 1);
    assert_eq!(state.input_gain_millidb(), -3210);
    assert_eq!(state.output_gain_millidb(), 1234);
    let mut pan = runtime(&patch(&state, parameter::DRY_WET, 0.0), 48000);
    let cycle: Vec<_> = (0..48)
        .map(|i| {
            let phase = std::f64::consts::TAU * i as f64 / 48.0;
            [phase.sin() as f32, phase.cos() as f32]
        })
        .collect();
    pan.process_block(
        &mut (0..24000).map(|i| cycle[i % 48]).collect::<Vec<_>>(),
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let a = (-100.0_f64 / 48000.0).exp();
    for phase in 0..48 {
        pan.process_block(&mut [cycle[phase]], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let e = history(pan.as_ref()).energy;
        for c in 0..2 {
            let expected = (1.0 - a) / (1.0 - a.powi(48))
                * (0..48)
                    .map(|k| a.powi(k as i32) * f64::from(cycle[(phase + 48 - k) % 48][c]).powi(2))
                    .sum::<f64>();
            assert!((e[c] - expected).abs() < 1e-13);
        }
        assert!(e[0].max(e[1]) < 0.51, "not instantaneous stereo-max power");
    }
}

#[test]
fn rms_digital_silence_retires_warm_energy_in_all_runtime_paths() {
    let mut failures = Vec::new();
    let mut cases = 0;
    for rate in [8000_u32, 22050, 44100, 48000, 96000, 192000] {
        for stimulus in ["dc", "impulse", "tone"] {
            for path in 0..8 {
                let initial_mode = u32::from(matches!(path, 1 | 3 | 5 | 6));
                let mut state = mode(
                    &COMPRESSOR_DEFINITION.default_state().unwrap(),
                    initial_mode,
                );
                if path >= 6 {
                    state = patch(&state, parameter::DRY_WET, 0.0);
                }
                let mut actual = runtime(&state, rate);
                let mut prime = [[0.5, -0.25]; 257];
                if stimulus == "impulse" {
                    prime[1..].fill([0.0; 2]);
                } else if stimulus == "tone" {
                    for (index, frame) in prime.iter_mut().enumerate() {
                        let phase = std::f64::consts::TAU * index as f64 / 48.0;
                        *frame = [(0.5 * phase.sin()) as f32, (0.25 * phase.cos()) as f32];
                    }
                }
                actual.process_block(&mut prime, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                let initial_energy = history(actual.as_ref()).energy;
                assert!(initial_energy.iter().all(|energy| *energy > 0.0));
                // The independent 10 ms energy law puts these primes below
                // the floor before one second. Verify that premise rather than
                // repeating eight seconds of settled silence in every matrix cell.
                assert!(
                    initial_energy
                        .iter()
                        .all(|energy| energy * (-100.0_f64).exp() < 1e-30)
                );
                if matches!(path, 4 | 5) {
                    let changed = mode(&state, 1 - initial_mode);
                    let mut next = runtime(&changed, rate);
                    next.copy_history_from(actual.as_ref());
                    actual = next;
                }
                let mut silence = [[0.0; 2]; 257];
                // Retain the full long-tail calibration for every runtime path
                // at both supported sample-rate extremes. All rates/stimuli/paths
                // still exercise the real runtime and exact-zero assertions.
                let silence_seconds = if stimulus == "dc" && matches!(rate, 8_000 | 192_000) {
                    8
                } else {
                    1
                };
                let mut remaining = rate as usize * silence_seconds;
                while remaining > 0 {
                    let count = remaining.min(silence.len());
                    if matches!(path, 2 | 3) {
                        actual.process_bypassed_block(
                            &mut silence[..count],
                            &hex_lattice::EMPTY_DEVICE_RESOURCES,
                        );
                    } else {
                        actual.process_block(
                            &mut silence[..count],
                            &hex_lattice::EMPTY_DEVICE_RESOURCES,
                        );
                    }
                    assert!(silence[..count].iter().all(|frame| *frame == [0.0; 2]));
                    remaining -= count;
                }
                let energy = history(actual.as_ref()).energy;
                actual.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                let later = history(actual.as_ref()).energy;
                println!(
                    "RMS_SILENCE rate={rate} stimulus={stimulus} path={path} seconds={silence_seconds} energy={energy:?} subnormal={:?} persistent_nonzero={}",
                    energy.map(f64::is_subnormal),
                    energy == later && energy != [0.0; 2]
                );
                assert!(energy.iter().all(|value| value.is_finite()));
                if energy != [0.0; 2] || later != [0.0; 2] {
                    failures.push((rate, stimulus, path, energy, later));
                }
                cases += 1;
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}/{cases} real runtime paths retain nonzero energy after settling or extended digital silence: {failures:?}",
        failures.len()
    );
    println!("RMS_SILENCE_CASES {cases} finite exact-zero histories, no native timing claim");
}

#[test]
fn rms_energy_floor_preserves_closed_form_above_floor_and_retires_at_analytic_crossing() {
    for rate in [8000_u32, 22050, 44100, 48000, 96000, 192000] {
        for detector in 0..2 {
            let state = mode(&COMPRESSOR_DEFINITION.default_state().unwrap(), detector);
            let mut actual = runtime(&state, rate);
            actual.process_block(
                &mut [[0.5, -0.25]; 257],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            let initial = history(actual.as_ref()).energy;
            let alpha = (-100.0 / f64::from(rate)).exp();
            let expected_crossing =
                initial.map(|energy| ((1e-30 / energy).ln() / alpha.ln()).ceil() as usize);
            let mut first_zero = [None; 2];
            for n in 1..=rate as usize {
                actual.process_block(&mut [[0.0; 2]], &hex_lattice::EMPTY_DEVICE_RESOURCES);
                for channel in 0..2 {
                    let energy = history(actual.as_ref()).energy[channel];
                    assert!(energy == 0.0 || energy >= 1e-30);
                    let analytic = initial[channel] * alpha.powi(n as i32);
                    if energy == 0.0 {
                        if first_zero[channel].is_none() {
                            first_zero[channel] = Some(n);
                        }
                    } else {
                        assert!(
                            (energy / analytic - 1.0).abs() < 1e-9,
                            "unchanged10ms law rate{rate} channel{channel} frame{n}"
                        );
                    }
                }
            }
            for channel in 0..2 {
                assert!(
                    first_zero[channel]
                        .unwrap()
                        .abs_diff(expected_crossing[channel])
                        <= 1,
                    "actual/closed-form crossing rate{rate} channel{channel}"
                );
            }
            println!(
                "RMS_FLOOR_CROSSING rate={rate} detector={detector} actual={first_zero:?} analytic={expected_crossing:?}"
            );
        }
    }
}

#[test]
fn rms_floor_partition_history_retarget_invalid_frames_and_reset_remain_coherent() {
    for rate in [8000_u32, 22050, 44100, 48000, 96000, 192000] {
        let base = COMPRESSOR_DEFINITION.default_state().unwrap();
        let changed = mode(&patch(&base, parameter::DRY_WET, 0.375), 1);
        let mut prior = runtime(&base, rate);
        prior.process_block(
            &mut [[0.75, -0.125]; 257],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let before = history(prior.as_ref()).energy;
        let mut whole = runtime(&changed, rate);
        whole.copy_history_from(prior.as_ref());
        let mut split = runtime(&changed, rate);
        split.copy_history_from(prior.as_ref());
        let mut a = vec![[0.0; 2]; rate as usize];
        let mut b = a.clone();
        whole.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut offset = 0;
        for count in [1, 31, 257, 3, 509].into_iter().cycle() {
            if offset == b.len() {
                break;
            }
            let end = (offset + count).min(b.len());
            let mut scratch = runtime(&changed, rate);
            scratch.copy_history_from(split.as_ref());
            scratch.process_block(&mut b[offset..end], &hex_lattice::EMPTY_DEVICE_RESOURCES);
            split = scratch;
            offset = end;
        }
        assert_eq!(a, b);
        assert_eq!(history(whole.as_ref()).energy, [0.0; 2]);
        assert_eq!(history(split.as_ref()).energy, [0.0; 2]);
        assert_eq!(
            history(whole.as_ref()).envelope,
            history(split.as_ref()).envelope
        );
        assert_eq!(
            history(whole.as_ref()).gain_db,
            history(split.as_ref()).gain_db
        );
        assert_eq!(
            history(prior.as_ref()).energy,
            before,
            "scratch processing/cancel did not change original history"
        );
        let mut peak = runtime(&base, rate);
        peak.copy_history_from(split.as_ref());
        peak.process_bypassed_block(&mut [[0.0; 2]; 1021], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(
            history(peak.as_ref()).energy,
            [0.0; 2],
            "retarget/bypass cannot resurrect a retired tail"
        );
        peak.process_block(
            &mut [[f32::NAN, 1.0], [1.0, f32::INFINITY]],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        assert_eq!(history(peak.as_ref()).energy, [0.0; 2]);
        peak.process_block(
            &mut [[f32::from_bits(1), -f32::from_bits(1)]],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        assert_eq!(
            history(peak.as_ref()).energy,
            [0.0; 2],
            "subfloor input does not recreate tiny history"
        );
        peak.reset();
        let mut actual = [[0.75, -0.125]; 257];
        let mut fresh = actual;
        peak.process_block(&mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        runtime(&base, rate).process_block(&mut fresh, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(actual, fresh, "reset retains exact original Peak PCM");
    }
}

/// Historical states keep opening (Pulse #3956). Layouts come from commits
/// d236adc6e (v1, 24 bytes) and 1ce92988b (v2, 28 bytes).
#[test]
fn compressor_v1_and_v2_states_migrate_with_their_original_behaviour() {
    let state = |version, values: &[f32]| {
        let payload = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        NativeDeviceState::new(COMPRESSOR_TYPE_ID, version, payload).unwrap()
    };
    // The registry is the engine's path when a saved project is reopened.
    let registry = crate::native_registry().unwrap();
    let migrate = |old: &NativeDeviceState| {
        let migrated = registry.migrate_state(old).unwrap();
        assert_eq!(
            registry.migrate_state(&migrated).unwrap(),
            migrated,
            "idempotent"
        );
        migrated
    };
    let read =
        |state: &NativeDeviceState, id| COMPRESSOR_DEFINITION.read_parameter(state, id).unwrap();
    let migrated = migrate(&state(1, &[-24.0, 4.0, 10.0, 120.0, 6.0, 2.0]));
    assert_eq!(migrated.state_version(), COMPRESSOR_STATE_VERSION);
    assert_eq!(
        read(&migrated, parameter::THRESHOLD),
        ParameterValue::Float(-24.0)
    );
    assert_eq!(
        read(&migrated, parameter::MAKEUP),
        ParameterValue::Float(2.0)
    );
    assert_eq!(
        read(&migrated, parameter::DRY_WET),
        ParameterValue::Float(1.0),
        "v1 was fully wet"
    );
    assert_eq!(
        read(&migrated, parameter::DETECTOR),
        ParameterValue::Choice(0),
        "v1/v2 were Peak"
    );
    let migrated = migrate(&state(2, &[-24.0, 4.0, 10.0, 120.0, 6.0, 2.0, 0.25]));
    assert_eq!(
        read(&migrated, parameter::DRY_WET),
        ParameterValue::Float(0.25)
    );
    assert_eq!(
        read(&migrated, parameter::DETECTOR),
        ParameterValue::Choice(0)
    );
    for (version, length) in [(1, 28), (2, 24), (2, 32), (3, 28)] {
        let malformed =
            NativeDeviceState::new(COMPRESSOR_TYPE_ID, version, vec![0; length]).unwrap();
        assert_eq!(
            COMPRESSOR_DEFINITION.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }
    assert_eq!(
        COMPRESSOR_DEFINITION.migrate_state(&state(4, &[0.0; 9])),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
}

#[test]
fn auxiliary_key_drives_both_channels_and_a_missing_key_is_silence() {
    assert_eq!(
        COMPRESSOR_DEFINITION.auxiliary_input(),
        hex_lattice_contract::AuxiliaryInput::Stereo
    );
    let input = [[0.5, -0.125]; 256];
    let key = [[0.0, 1.0]; 256];
    for mode in [Detector::Peak, Detector::Rms] {
        let state = encode(CompressorSettings {
            detector: mode,
            attack_ms: 0.1,
            ..CompressorSettings::default()
        })
        .unwrap();
        let mut processor = runtime(&state, 48_000);
        let mut silence = [[1.0; 2]; 256];
        let mut disconnected = input;
        let block =
            hex_lattice::StereoAuxiliaryBlock::new(&mut disconnected, None, &mut silence, 256)
                .unwrap();
        processor
            .process_block_with_auxiliary(
                block,
                None,
                None,
                false,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        assert_eq!(disconnected, input);

        let mut wet = input;
        let block = hex_lattice::StereoAuxiliaryBlock::new(&mut wet, Some(&key), &mut silence, 256)
            .unwrap();
        processor
            .process_block_with_auxiliary(
                block,
                None,
                None,
                false,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        assert!(wet[255][0] < input[255][0] * 0.7, "{mode:?}");
        assert_eq!(wet[255][1], -wet[255][0] * 0.25);
        assert_eq!(key, [[0.0, 1.0]; 256]);
    }
}

#[test]
fn auxiliary_bypass_advances_the_same_history_across_block_partitions() {
    let state = COMPRESSOR_DEFINITION.default_state().unwrap();
    let mut wet = runtime(&state, 48_000);
    let mut bypassed = runtime(&state, 48_000);
    let key = [[0.0, 1.0]; 128];
    let dry = [[0.125, -0.25]; 128];
    let mut scratch = [];
    let mut main = dry;
    let block =
        hex_lattice::StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut scratch, 128).unwrap();
    wet.process_block_with_auxiliary(
        block,
        None,
        None,
        false,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    )
    .unwrap();

    let mut dry_blocks = dry;
    for (main, key) in dry_blocks.chunks_mut(19).zip(key.chunks(19)) {
        let block =
            hex_lattice::StereoAuxiliaryBlock::new(main, Some(key), &mut scratch, 128).unwrap();
        bypassed
            .process_block_with_auxiliary(
                block,
                None,
                None,
                true,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
    }
    assert_eq!(dry_blocks, dry);
    assert_ne!(main, dry);

    let mut after_wet = dry;
    let mut after_bypass = dry;
    for (processor, main) in [
        (&mut wet, &mut after_wet),
        (&mut bypassed, &mut after_bypass),
    ] {
        let block =
            hex_lattice::StereoAuxiliaryBlock::new(main, Some(&key), &mut scratch, 128).unwrap();
        processor
            .process_block_with_auxiliary(
                block,
                None,
                None,
                false,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
    }
    assert_eq!(after_wet, after_bypass);
}

#[test]
fn auxiliary_rejects_unsupported_automation_before_processing() {
    struct ThresholdAutomation;

    impl NativeParameterAutomation for ThresholdAutomation {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[parameter::THRESHOLD]
        }

        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(ParameterValue::Float(-60.0))
        }
    }

    let state = COMPRESSOR_DEFINITION.default_state().unwrap();
    let mut processor = runtime(&state, 48_000);
    let mut fresh = runtime(&state, 48_000);
    let mut main = [[0.5, -0.25]; 128];
    let dry = main;
    let key = [[1.0, 0.0]; 128];
    let mut scratch = [];
    let block = StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut scratch, 128).unwrap();
    assert_eq!(
        processor.process_block_with_auxiliary(
            block,
            None,
            Some(&ThresholdAutomation),
            false,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        ),
        Err(DeviceContractError::UnsupportedProcessingClass)
    );
    assert_eq!(main, dry);
    let mut expected = dry;
    processor.process_block(&mut main, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    fresh.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(main, expected);
}
