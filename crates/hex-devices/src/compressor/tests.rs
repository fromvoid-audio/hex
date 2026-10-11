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
        .prepare_processor(state, sample_rate)
        .expect("valid compressor preparation")
        .expect("stateful processor")
}

fn runtime(state: &NativeDeviceState, sample_rate: u32) -> Box<dyn NativeProcessorRuntime> {
    prepared(state, sample_rate).create_runtime()
}

#[test]
fn defaults_and_generic_codec_roundtrip_all_six_parameters() {
    let registry = registry();
    let state = registry
        .default_state(COMPRESSOR_TYPE_ID)
        .expect("compressor defaults");
    assert_eq!(state.state_version(), 1);
    assert_eq!(state.payload().len(), STATE_BYTES);
    assert_eq!(registry.migrate_state(&state).expect("migration"), state);
    for (id, expected) in [
        (parameter::THRESHOLD, -18.0),
        (parameter::RATIO, 4.0),
        (parameter::ATTACK, 10.0),
        (parameter::RELEASE, 100.0),
        (parameter::KNEE, 6.0),
        (parameter::MAKEUP, 0.0),
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
    let future = NativeDeviceState::new(COMPRESSOR_TYPE_ID, 2, state.payload().to_vec())
        .expect("future version");
    assert_eq!(
        registry.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    for length in [STATE_BYTES - 1, STATE_BYTES + 1] {
        let malformed = NativeDeviceState::new(COMPRESSOR_TYPE_ID, 1, vec![0; length])
            .expect("bounded malformed state");
        assert_eq!(
            registry.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }
    let mut invalid_number = state.payload().to_vec();
    invalid_number[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
    let malformed = NativeDeviceState::new(COMPRESSOR_TYPE_ID, 1, invalid_number)
        .expect("bounded non-finite state");
    assert_eq!(
        registry.migrate_state(&malformed),
        Err(DeviceContractError::InvalidParameterValue)
    );
    for sample_rate in [0, 7_999, 192_001] {
        assert!(
            COMPRESSOR_DEFINITION
                .prepare_processor(&state, sample_rate)
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
    runtime.process_block(&mut frame);
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
    linked.process_block(&mut frame);
    assert!((f64::from(frame[0][0]) / f64::from(frame[0][1]) + 4.0).abs() < 1.0e-6);

    let ratio_one = patch(&defaults, parameter::RATIO, 1.0);
    let mut identity = runtime(&ratio_one, 48_000);
    let mut frames = [[0.75, -0.1875], [0.5, -0.125]];
    let original = frames;
    identity.process_block(&mut frames);
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
    fast.process_block(&mut loud_fast);
    slow.process_block(&mut loud_slow);
    let mut quiet_fast = [[0.1, 0.0]];
    let mut quiet_slow = quiet_fast;
    fast.process_block(&mut quiet_fast);
    slow.process_block(&mut quiet_slow);
    assert_ne!(quiet_fast, quiet_slow);

    let ratio_one = patch(&defaults, parameter::RATIO, 1.0);
    let makeup = patch(&ratio_one, parameter::MAKEUP, 6.0);
    let mut boosted = runtime(&makeup, 48_000);
    let mut frames = [[0.25, -0.125]; 256];
    let original = frames;
    boosted.process_block(&mut frames);
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
    whole.process_block(&mut whole_frames);
    for chunk in partition_frames.chunks_mut(31) {
        partitioned.process_block(chunk);
    }
    assert_eq!(whole_frames, partition_frames);

    let mut bypassed = runtime(&defaults, 48_000);
    let mut active = runtime(&defaults, 48_000);
    let mut prime = [[0.9, -0.3]; 80];
    bypassed.process_block(&mut prime);
    let mut prime = [[0.9, -0.3]; 80];
    active.process_block(&mut prime);
    let mut untouched = [[0.6, -0.2]; 8];
    let original = untouched;
    bypassed.process_bypassed_block(&mut untouched);
    assert_eq!(untouched, original);
    let mut active_signal = [[0.6, -0.2]; 8];
    active.process_block(&mut active_signal);
    let mut next_a = [[0.4, 0.1]; 1];
    let mut next_b = next_a;
    bypassed.process_block(&mut next_a);
    active.process_block(&mut next_b);
    assert_eq!(next_a, next_b);
    assert!(bypassed.is_quiescent());

    let mut zero_after_signal = [[0.0, 0.0]; 16];
    bypassed.process_block(&mut zero_after_signal);
    assert!(zero_after_signal.iter().all(|frame| *frame == [0.0, 0.0]));
    assert!(bypassed.is_quiescent());

    let mut reset = runtime(&defaults, 48_000);
    let mut fresh = runtime(&defaults, 48_000);
    let mut history = [[1.0, 0.0]; 200];
    reset.process_block(&mut history);
    reset.reset();
    let mut after_reset = [[0.5, -0.25]; 1];
    let mut after_fresh = after_reset;
    reset.process_block(&mut after_reset);
    fresh.process_block(&mut after_fresh);
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
    old.process_block(&mut signal);
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
    invalid_then_valid.process_block(&mut invalid);
    assert!(invalid[0][0].is_nan());
    assert_eq!(invalid[0][1], f32::INFINITY);
    let mut a = [[0.5, -0.25]];
    let mut b = a;
    invalid_then_valid.process_block(&mut a);
    valid_only.process_block(&mut b);
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
    let malformed = NativeDeviceState::new(COMPRESSOR_TYPE_ID, 1, vec![0]).expect("state");
    assert_eq!(
        transfer_curve(&malformed),
        Err(DeviceContractError::MalformedState)
    );
}
