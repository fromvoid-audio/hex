//! Independent DSP, state, and runtime regression tests for EQ8.
use super::*;

fn response(coefficients: Coefficients, hz: f64, rate: f64) -> f64 {
    let angle = std::f64::consts::TAU * hz / rate;
    let (sin, cos) = angle.sin_cos();
    let (sin2, cos2) = (2.0 * angle).sin_cos();
    let re_num = coefficients.b0 + coefficients.b1 * cos + coefficients.b2 * cos2;
    let im_num = -coefficients.b1 * sin - coefficients.b2 * sin2;
    let re_den = 1.0 + coefficients.a1 * cos + coefficients.a2 * cos2;
    let im_den = -coefficients.a1 * sin - coefficients.a2 * sin2;
    ((re_num * re_num + im_num * im_num) / (re_den * re_den + im_den * im_den)).sqrt()
}

#[test]
fn state_roundtrips_all_five_banks_and_rejects_malformed_values() {
    let mut state = EQ8_DEFINITION.default_state().unwrap();
    for bank in 0..5 {
        state = EQ8_DEFINITION
            .patch_parameter(
                &state,
                parameter::enabled(bank, 2),
                ParameterValue::Bool(true),
            )
            .unwrap();
        state = EQ8_DEFINITION
            .patch_parameter(&state, parameter::kind(bank, 2), ParameterValue::Choice(7))
            .unwrap();
        state = EQ8_DEFINITION
            .patch_parameter(
                &state,
                parameter::frequency(bank, 2),
                ParameterValue::Float(777.0 + bank as f32),
            )
            .unwrap();
        state = EQ8_DEFINITION
            .patch_parameter(
                &state,
                parameter::gain(bank, 2),
                ParameterValue::Float(-3.5 + bank as f32),
            )
            .unwrap();
        state = EQ8_DEFINITION
            .patch_parameter(
                &state,
                parameter::q(bank, 2),
                ParameterValue::Float(2.0 + bank as f32),
            )
            .unwrap();
    }
    let migrated = EQ8_DEFINITION.migrate_state(&state).unwrap();
    assert_eq!(state, migrated);
    assert_eq!(state.payload().len(), STATE_BYTES);
    assert_eq!(DESCRIPTORS.len(), 202);

    let mut malformed = state.payload().to_vec();
    malformed[5 + 1] = 9;
    let malformed = NativeDeviceState::new(EQ8_TYPE_ID, STATE_VERSION, malformed).unwrap();
    assert_eq!(decode(&malformed), Err(DeviceContractError::MalformedState));

    let mut malformed = state.payload().to_vec();
    malformed[5 + 2..5 + 6].copy_from_slice(&f32::NAN.to_le_bytes());
    let malformed = NativeDeviceState::new(EQ8_TYPE_ID, STATE_VERSION, malformed).unwrap();
    assert_eq!(decode(&malformed), Err(DeviceContractError::MalformedState));
    assert!(
        EQ8_DEFINITION
            .patch_parameter(
                &state,
                parameter::frequency(0, 0),
                ParameterValue::Float(f32::INFINITY)
            )
            .is_err()
    );
}

#[test]
fn each_filter_shape_has_finite_prepared_response_and_expected_gain_or_cut() {
    let rate = 48_000;
    let frequency = 1_000.0_f32;
    for kind in 0..BAND_TYPES.len() as u8 {
        let band = Band {
            enabled: true,
            kind,
            frequency_hz: frequency,
            gain_db: 12.0,
            q: std::f32::consts::FRAC_1_SQRT_2,
        };
        let coefficients = design_band(band, rate).unwrap();
        assert!(coefficients.iter().all(|coefficient| {
            [
                coefficient.b0,
                coefficient.b1,
                coefficient.b2,
                coefficient.a1,
                coefficient.a2,
            ]
            .iter()
            .all(|value| value.is_finite())
        }));
        let magnitude = coefficients
            .iter()
            .map(|coefficient| response(*coefficient, f64::from(frequency), f64::from(rate)))
            .product::<f64>();
        match kind {
            0 => assert!((20.0 * magnitude.log10() - 12.0).abs() < 0.05),
            7 => assert!(magnitude < 0.02),
            3 | 4 => assert!(magnitude > 0.7),
            5 | 6 => assert!(magnitude < 0.8),
            _ => assert!(magnitude.is_finite() && magnitude > 0.0),
        }
    }
}

#[test]
fn prepared_processor_is_fixed_bounded_zero_latency_and_rate_safe() {
    const { assert!(EQ8_RUNTIME_BYTES <= 4_096) };
    for rate in [8_000, 22_050, 44_100, 48_000, 96_000, 192_000] {
        let settings = decode(&EQ8_DEFINITION.default_state().unwrap()).unwrap();
        let prepared = PreparedEq8::new(settings, rate).unwrap();
        assert_eq!(prepared.sample_rate, rate);
        assert_eq!(prepared.runtime().sample_rate, rate);
        let mut runtime = prepared.runtime();
        assert!(runtime.is_quiescent());
        let mut impulse = [[1.0, -0.5]; 128];
        runtime.process_block(&mut impulse);
        assert!(impulse.iter().flatten().all(|sample| sample.is_finite()));
        runtime.reset();
        assert!(runtime.is_quiescent());
    }
    assert!(
        PreparedEq8::new(
            decode(&EQ8_DEFINITION.default_state().unwrap()).unwrap(),
            7_999
        )
        .is_err()
    );
}

#[test]
fn coefficient_and_output_gain_edits_ramp_partition_independently() {
    let mut old_state = EQ8_DEFINITION.default_state().unwrap();
    for (id, value) in [
        (parameter::enabled(0, 3), ParameterValue::Bool(true)),
        (parameter::frequency(0, 3), ParameterValue::Float(700.0)),
        (parameter::gain(0, 3), ParameterValue::Float(10.0)),
    ] {
        old_state = EQ8_DEFINITION
            .patch_parameter(&old_state, id, value)
            .unwrap();
    }
    let old_prepared = PreparedEq8::new(decode(&old_state).unwrap(), 48_000).unwrap();
    let mut old_runtime = old_prepared.runtime();
    for _ in 0..32 {
        old_runtime.process_frame([0.4, -0.2]);
    }

    let mut new_state = EQ8_DEFINITION
        .patch_parameter(
            &old_state,
            parameter::frequency(0, 3),
            ParameterValue::Float(2_500.0),
        )
        .unwrap();
    new_state = EQ8_DEFINITION
        .patch_parameter(
            &new_state,
            parameter::OUTPUT_GAIN,
            ParameterValue::Float(1.5),
        )
        .unwrap();
    let new_prepared = PreparedEq8::new(decode(&new_state).unwrap(), 48_000).unwrap();
    let mut whole = new_prepared.runtime();
    let mut split = new_prepared.runtime();
    assert!(whole.inherit_history_from(&old_runtime));
    assert!(split.inherit_history_from(&old_runtime));

    let mut whole_audio = [[0.25, -0.125]; 300];
    whole.process(&mut whole_audio);
    let mut split_audio = [[0.25, -0.125]; 300];
    let mut offset = 0;
    for size in [1_usize, 17, 64, 127, 91] {
        let end = offset + size;
        let mut scratch = new_prepared.runtime();
        assert!(scratch.inherit_history_from(&split));
        scratch.process(&mut split_audio[offset..end]);
        split = scratch;
        offset = end;
    }
    assert_eq!(offset, split_audio.len());
    assert_eq!(whole_audio, split_audio);
    assert_eq!(whole.smooth_remaining, 0);
    assert_eq!(whole.current_gain, whole.target_gain);
    assert!(
        whole_audio
            .iter()
            .flatten()
            .all(|sample| sample.is_finite())
    );

    let low_q = design_band(
        Band {
            enabled: true,
            kind: 4,
            frequency_hz: 1_000.0,
            gain_db: 0.0,
            q: 0.5,
        },
        48_000,
    )
    .unwrap();
    let high_q = design_band(
        Band {
            enabled: true,
            kind: 4,
            frequency_hz: 1_000.0,
            gain_db: 0.0,
            q: 2.0,
        },
        48_000,
    )
    .unwrap();
    assert_ne!(low_q, high_q);

    let mut extreme = [[f32::MAX, -f32::MAX]; 128];
    whole.process(&mut extreme);
    assert!(extreme.iter().flatten().all(|sample| sample.is_finite()));
}

#[test]
fn mode_and_coefficients_keep_independent_runtime_histories() {
    let mut state = EQ8_DEFINITION.default_state().unwrap();
    for (id, value) in [
        (parameter::enabled(1, 0), ParameterValue::Bool(true)),
        (parameter::kind(1, 0), ParameterValue::Choice(3)),
        (parameter::enabled(2, 0), ParameterValue::Bool(true)),
        (parameter::kind(2, 0), ParameterValue::Choice(3)),
        (parameter::MODE, ParameterValue::Choice(1)),
    ] {
        state = EQ8_DEFINITION.patch_parameter(&state, id, value).unwrap();
    }
    let prepared = PreparedEq8::new(decode(&state).unwrap(), 48_000).unwrap();
    let mut runtime = prepared.runtime();
    let output = runtime.process_frame([1.0, 0.0]);
    assert!(output[0].abs() < 1.0);
    assert_eq!(output[1], 0.0);

    let mut compatible = prepared.runtime();
    assert!(compatible.inherit_history_from(&runtime));
    assert_eq!(compatible.history, runtime.history);
    let different_rate = PreparedEq8::new(decode(&state).unwrap(), 44_100).unwrap();
    let mut reset = different_rate.runtime();
    assert!(!reset.inherit_history_from(&runtime));
    assert!(reset.is_quiescent());
}

#[test]
fn all_cut_orders_remain_stable_at_extreme_q_and_sample_rates() {
    for rate in [8_000, 22_050, 48_000, 192_000] {
        for kind in [3, 4, 5, 6] {
            for q in [MIN_Q, MAX_Q] {
                let frequency = (rate as f32 * 0.47).min(MAX_HZ);
                let coefficients = design_band(
                    Band {
                        enabled: true,
                        kind,
                        frequency_hz: frequency,
                        gain_db: 0.0,
                        q,
                    },
                    rate,
                )
                .unwrap();
                for stage in coefficients {
                    assert!(stage.a1.is_finite() && stage.a2.is_finite());
                    assert!(stage.a2.abs() < 1.0);
                    assert!(1.0 + stage.a1 + stage.a2 > 0.0);
                    assert!(1.0 - stage.a1 + stage.a2 > 0.0);
                }
                let magnitude = coefficients
                    .iter()
                    .map(|stage| response(*stage, f64::from(frequency), f64::from(rate)))
                    .product::<f64>();
                assert!(magnitude.is_finite() && magnitude < 100.0);
            }
        }
    }
}

#[test]
fn channel_banks_are_independent_for_left_right_and_mid_side_modes() {
    for (mode, first_bank, second_bank) in [(1, 1, 2), (2, 3, 4)] {
        let mut state = EQ8_DEFINITION.default_state().unwrap();
        for (bank, gain) in [(first_bank, 9.0), (second_bank, -6.0)] {
            for (id, value) in [
                (parameter::enabled(bank, 3), ParameterValue::Bool(true)),
                (
                    parameter::frequency(bank, 3),
                    ParameterValue::Float(1_000.0),
                ),
                (parameter::gain(bank, 3), ParameterValue::Float(gain)),
            ] {
                state = EQ8_DEFINITION.patch_parameter(&state, id, value).unwrap();
            }
        }
        state = EQ8_DEFINITION
            .patch_parameter(&state, parameter::MODE, ParameterValue::Choice(mode))
            .unwrap();
        let prepared = PreparedEq8::new(decode(&state).unwrap(), 48_000).unwrap();
        let response = prepared.frequency_response_at_hz(1_000.0).unwrap();
        assert!(response[0].is_finite() && response[1].is_finite());
        assert!((response[0] - response[1]).abs() > 0.1);
    }
}

#[test]
fn registered_runtime_chain_accepts_routing_change_and_resets_incompatible_lanes() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorInstance, ProcessorChainOwner, StereoMatrix,
    };

    let mut old_state = EQ8_DEFINITION.default_state().unwrap();
    old_state = EQ8_DEFINITION
        .patch_parameter(
            &old_state,
            parameter::enabled(1, 0),
            ParameterValue::Bool(true),
        )
        .unwrap();
    old_state = EQ8_DEFINITION
        .patch_parameter(
            &old_state,
            parameter::gain(1, 0),
            ParameterValue::Float(12.0),
        )
        .unwrap();
    old_state = EQ8_DEFINITION
        .patch_parameter(&old_state, parameter::MODE, ParameterValue::Choice(1))
        .unwrap();
    let mut next_state = EQ8_DEFINITION
        .patch_parameter(&old_state, parameter::MODE, ParameterValue::Choice(2))
        .unwrap();
    // The new M/S mode has its own bank and should start from an empty lane history.
    next_state = EQ8_DEFINITION
        .patch_parameter(
            &next_state,
            parameter::enabled(3, 0),
            ParameterValue::Bool(true),
        )
        .unwrap();

    let chain_for = |state: NativeDeviceState| {
        let prepared = Arc::new(PreparedEq8::new(decode(&state).unwrap(), 48_000).unwrap());
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            owner: ProcessorChainOwner::Track(1),
            device_id: 9,
            device_type: EQ8_TYPE_ID,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state)),
            definition: Some(prepared),
        }])
        .unwrap()
    };

    let mut old = chain_for(old_state);
    old.begin_block();
    let mut material = [[1.0, 0.25]; 64];
    old.process_block(&mut material);
    old.commit_block();
    assert!(!old.committed_range_is_quiescent(0..1));

    let mut next = chain_for(next_state);
    assert!(next.can_inherit_from(&old));
    next.inherit_from(&old);
    assert!(next.committed_range_is_quiescent(0..1));
    next.begin_block();
    let mut output = [[0.25, -0.1]; 32];
    next.process_block(&mut output);
    next.commit_block();
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
}
