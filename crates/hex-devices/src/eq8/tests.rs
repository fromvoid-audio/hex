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
    println!("fixed eq8 runtime bytes: {EQ8_RUNTIME_BYTES}");
    // Fixed histories, one outgoing routing, 161 cached controls and inline
    // coefficient targets fit in 16 KiB; factories report their actual size.
    const { assert!(EQ8_RUNTIME_BYTES <= 16_384) };
    for rate in [8_000, 22_050, 44_100, 48_000, 96_000, 192_000] {
        let settings = decode(&EQ8_DEFINITION.default_state().unwrap()).unwrap();
        let prepared = PreparedEq8::new(settings, rate).unwrap();
        assert_eq!(prepared.sample_rate, rate);
        assert_eq!(prepared.runtime().sample_rate, rate);
        let mut runtime = prepared.runtime();
        assert!(runtime.is_quiescent());
        let mut impulse = [[1.0, -0.5]; 128];
        runtime.process_block(&mut impulse, &hex_lattice::EMPTY_DEVICE_RESOURCES);
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

pub(super) fn patch_band(
    mut state: NativeDeviceState,
    bank: usize,
    band: usize,
    kind: u32,
    frequency: f32,
    gain: f32,
) -> NativeDeviceState {
    for (id, value) in [
        (parameter::enabled(bank, band), ParameterValue::Bool(true)),
        (parameter::kind(bank, band), ParameterValue::Choice(kind)),
        (
            parameter::frequency(bank, band),
            ParameterValue::Float(frequency),
        ),
        (parameter::gain(bank, band), ParameterValue::Float(gain)),
    ] {
        state = EQ8_DEFINITION.patch_parameter(&state, id, value).unwrap();
    }
    state
}

fn test_signal(frames: usize) -> Vec<StereoFrame> {
    (0..frames)
        .map(|frame| {
            let time = frame as f64 / 48_000.0;
            [
                (0.37 * (std::f64::consts::TAU * 311.0 * time).sin()
                    + 0.19 * (std::f64::consts::TAU * 2_100.0 * time).sin()) as f32,
                (0.29 * (std::f64::consts::TAU * 701.0 * time).sin()
                    - 0.13 * (std::f64::consts::TAU * 3_700.0 * time).sin()) as f32,
            ]
        })
        .collect()
}

#[test]
fn audition_selects_only_one_band_for_every_shape_and_tracks_normal_history() {
    for kind in 0..BAND_TYPES.len() as u32 {
        let mut full_state = EQ8_DEFINITION.default_state().unwrap();
        full_state = patch_band(full_state, 0, 0, 0, 180.0, 8.0);
        full_state = patch_band(full_state, 0, 3, kind, 2_400.0, 6.0);
        full_state = EQ8_DEFINITION
            .patch_parameter(
                &full_state,
                parameter::OUTPUT_GAIN,
                ParameterValue::Float(1.4),
            )
            .unwrap();
        let full_prepared = PreparedEq8::new(decode(&full_state).unwrap(), 48_000).unwrap();

        let mut solo_state = EQ8_DEFINITION.default_state().unwrap();
        solo_state = patch_band(solo_state, 0, 3, kind, 2_400.0, 6.0);
        solo_state = EQ8_DEFINITION
            .patch_parameter(
                &solo_state,
                parameter::OUTPUT_GAIN,
                ParameterValue::Float(1.4),
            )
            .unwrap();
        let solo_prepared = PreparedEq8::new(decode(&solo_state).unwrap(), 48_000).unwrap();

        let input = test_signal(600);
        for lane in 0..2 {
            let mut audition = full_prepared.runtime();
            let mut normal = full_prepared.runtime();
            let mut expected_solo = solo_prepared.runtime();
            audition
                .set_audition_selection(Some(Eq8AuditionSelection::new(3, lane).unwrap()))
                .unwrap();
            let mut audition_out = input.clone();
            let mut normal_out = input.clone();
            let mut solo_out = input.clone();
            audition.process(&mut audition_out);
            normal.process(&mut normal_out);
            expected_solo.process(&mut solo_out);

            assert_eq!(
                audition_out[512..],
                solo_out[512..],
                "shape={kind}, lane={lane}"
            );
            assert!(
                audition_out[512..]
                    .iter()
                    .zip(&normal_out[512..])
                    .any(|(solo, full)| {
                        (solo[0] - full[0]).abs() > 1.0e-4 || (solo[1] - full[1]).abs() > 1.0e-4
                    }),
                "non-neutral other band did not distinguish solo for shape={kind}"
            );

            // The ordinary cascade keeps advancing while audition is active.
            audition.set_audition_selection(None).unwrap();
            let release_input = test_signal(320);
            let mut released = release_input.clone();
            let mut normal_reference = release_input;
            audition.process(&mut released);
            normal.process(&mut normal_reference);
            assert_eq!(
                released[256..],
                normal_reference[256..],
                "shape={kind}, lane={lane}"
            );
            assert_eq!(
                audition.history, normal.history,
                "shape={kind}, lane={lane}"
            );
        }
    }
}

#[test]
fn audition_respects_left_right_and_mid_side_edit_lane() {
    for mode in [1_u32, 2] {
        for lane in 0..2 {
            for kind in 0..BAND_TYPES.len() as u32 {
                let active_bank = match (mode, lane) {
                    (1, 0) => 1,
                    (1, 1) => 2,
                    (2, 0) => 3,
                    _ => 4,
                };
                let other_bank = match (mode, lane) {
                    (1, 0) => 2,
                    (1, 1) => 1,
                    (2, 0) => 4,
                    _ => 3,
                };
                let mut full_state = EQ8_DEFINITION.default_state().unwrap();
                full_state = patch_band(full_state, active_bank, 2, kind, 1_200.0, 9.0);
                full_state = patch_band(full_state, other_bank, 2, 0, 800.0, -7.0);
                full_state = EQ8_DEFINITION
                    .patch_parameter(&full_state, parameter::MODE, ParameterValue::Choice(mode))
                    .unwrap();
                let full_prepared = PreparedEq8::new(decode(&full_state).unwrap(), 48_000).unwrap();

                let mut solo_state = EQ8_DEFINITION.default_state().unwrap();
                solo_state = patch_band(solo_state, active_bank, 2, kind, 1_200.0, 9.0);
                solo_state = EQ8_DEFINITION
                    .patch_parameter(&solo_state, parameter::MODE, ParameterValue::Choice(mode))
                    .unwrap();
                let solo_prepared = PreparedEq8::new(decode(&solo_state).unwrap(), 48_000).unwrap();
                let mut audition = full_prepared.runtime();
                let mut expected = solo_prepared.runtime();
                audition
                    .set_audition_selection(Some(Eq8AuditionSelection::new(2, lane).unwrap()))
                    .unwrap();
                let mut actual_signal = test_signal(400);
                let mut expected_signal = actual_signal.clone();
                audition.process(&mut actual_signal);
                expected.process(&mut expected_signal);
                assert_eq!(
                    actual_signal[256..],
                    expected_signal[256..],
                    "mode={mode}, lane={lane}, kind={kind}"
                );
            }
        }
    }
}

#[test]
fn audition_validation_neutral_enabled_reset_and_disabled_handoff_are_defined() {
    assert!(Eq8AuditionSelection::new(BAND_COUNT, 0).is_err());
    assert!(Eq8AuditionSelection::new(0, 2).is_err());

    let state = EQ8_DEFINITION.default_state().unwrap();
    let mut disabled = PreparedEq8::new(decode(&state).unwrap(), 48_000)
        .unwrap()
        .runtime();
    assert!(
        disabled
            .set_audition_selection(Some(Eq8AuditionSelection::new(0, 0).unwrap()))
            .is_err()
    );
    assert_eq!(disabled.audition_selection, None);
    assert_eq!(disabled.audition_request, None);

    // Authored enabled state is separate from its prepared coefficient shape;
    // a neutral-gain bell remains selectable.
    let neutral_state = patch_band(state, 0, 0, 0, 1_000.0, 0.0);
    let neutral_prepared = PreparedEq8::new(decode(&neutral_state).unwrap(), 48_000).unwrap();
    let mut neutral = neutral_prepared.runtime();
    assert!(neutral.enabled_bands[0][0]);
    neutral
        .set_audition_selection(Some(Eq8AuditionSelection::new(0, 0).unwrap()))
        .unwrap();

    let enabled_state = patch_band(
        EQ8_DEFINITION.default_state().unwrap(),
        0,
        0,
        0,
        1_000.0,
        8.0,
    );
    let old = PreparedEq8::new(decode(&enabled_state).unwrap(), 48_000).unwrap();
    let mut previous = old.runtime();
    previous
        .set_audition_selection(Some(Eq8AuditionSelection::new(0, 1).unwrap()))
        .unwrap();
    let mut material = test_signal(40);
    previous.process(&mut material);
    let mut disabled_candidate = PreparedEq8::new(
        decode(&EQ8_DEFINITION.default_state().unwrap()).unwrap(),
        48_000,
    )
    .unwrap()
    .runtime();
    assert!(disabled_candidate.inherit_history_from(&previous));
    assert_eq!(disabled_candidate.audition_selection, None);
    assert_eq!(disabled_candidate.audition_request, None);
    assert_eq!(disabled_candidate.audition_mix, 0.0);
    assert_eq!(disabled_candidate.audition_history, [[[0.0; 2]; 4]; 2]);

    let mut active = old.runtime();
    active
        .set_audition_selection(Some(Eq8AuditionSelection::new(0, 0).unwrap()))
        .unwrap();
    let mut changed_mode = enabled_state;
    changed_mode = EQ8_DEFINITION
        .patch_parameter(&changed_mode, parameter::MODE, ParameterValue::Choice(1))
        .unwrap();
    let mut mode_candidate = PreparedEq8::new(decode(&changed_mode).unwrap(), 48_000)
        .unwrap()
        .runtime();
    assert!(!mode_candidate.inherit_history_from(&active));
    assert_eq!(mode_candidate.audition_selection, None);
    assert_eq!(mode_candidate.audition_request, None);
    assert_eq!(mode_candidate.audition_mix, 0.0);

    neutral.reset();
    assert_eq!(neutral.audition_selection, None);
    assert_eq!(neutral.audition_request, None);
    assert_eq!(neutral.audition_mix, 0.0);
    assert_eq!(neutral.audition_history, [[[0.0; 2]; 4]; 2]);
}

#[test]
fn changing_audition_selection_crossfades_through_normal_and_resets_history() {
    let mut state = EQ8_DEFINITION.default_state().unwrap();
    state = patch_band(state, 0, 0, 0, 250.0, 9.0);
    state = patch_band(state, 0, 1, 0, 3_000.0, -7.0);
    state = patch_band(state, 0, 2, 0, 7_000.0, 5.0);
    let mut runtime = PreparedEq8::new(decode(&state).unwrap(), 48_000)
        .unwrap()
        .runtime();
    let first = Eq8AuditionSelection::new(0, 0).unwrap();
    let second = Eq8AuditionSelection::new(1, 0).unwrap();
    runtime.set_audition_selection(Some(first)).unwrap();
    let mut first_audio = test_signal(256);
    runtime.process(&mut first_audio);
    assert_eq!(runtime.audition_selection, Some(first));
    assert_eq!(runtime.audition_mix, 1.0);
    assert!(
        runtime
            .audition_history
            .iter()
            .flatten()
            .flatten()
            .any(|v| *v != 0.0)
    );

    runtime.set_audition_selection(Some(second)).unwrap();
    let mut change_audio = test_signal(255);
    runtime.process(&mut change_audio);
    assert_eq!(runtime.audition_selection, Some(first));
    assert_eq!(runtime.audition_mix, 1.0 / 256.0);
    runtime.advance_audition();
    assert_eq!(runtime.audition_selection, Some(second));
    assert_eq!(runtime.audition_mix, 0.0);
    assert_eq!(runtime.audition_history, [[[0.0; 2]; 4]; 2]);

    let mut second_audio = test_signal(256);
    runtime.process(&mut second_audio);
    assert_eq!(runtime.audition_mix, 1.0);
    assert!(
        runtime
            .audition_history
            .iter()
            .flatten()
            .flatten()
            .any(|v| *v != 0.0)
    );
    assert!(
        first_audio
            .iter()
            .chain(&change_audio)
            .chain(&second_audio)
            .flatten()
            .all(|x| x.is_finite())
    );

    // If a requested switch is replaced during fade-out, the newest request
    // wins. The latest band starts only after returning fully to normal.
    let third = Eq8AuditionSelection::new(2, 0).unwrap();
    runtime.set_audition_selection(Some(first)).unwrap();
    let mut begin_superseded_switch = test_signal(64);
    runtime.process(&mut begin_superseded_switch);
    runtime.set_audition_selection(Some(third)).unwrap();
    let mut finish_superseded_switch = test_signal(191);
    runtime.process(&mut finish_superseded_switch);
    assert_eq!(runtime.audition_mix, 1.0 / 256.0);
    runtime.advance_audition();
    assert_eq!(runtime.audition_selection, Some(third));
    assert_eq!(runtime.audition_mix, 0.0);
    assert_eq!(runtime.audition_history, [[[0.0; 2]; 4]; 2]);
    let mut fade_to_third = test_signal(256);
    runtime.process(&mut fade_to_third);
    assert_eq!(runtime.audition_mix, 1.0);
    assert_eq!(runtime.audition_selection, Some(third));

    // Clearing a pending switch cancels it and returns the active band to the
    // normal cascade.
    runtime.set_audition_selection(Some(second)).unwrap();
    let mut begin_cancel = test_signal(48);
    runtime.process(&mut begin_cancel);
    runtime.set_audition_selection(None).unwrap();
    let mut finish_cancel = test_signal(208);
    runtime.process(&mut finish_cancel);
    assert_eq!(runtime.audition_selection, None);
    assert_eq!(runtime.audition_mix, 0.0);
    assert_eq!(runtime.audition_history, [[[0.0; 2]; 4]; 2]);
}

#[test]
fn audition_history_crossfades_copies_and_partitions_with_live_coefficients() {
    // Fixed histories, one outgoing routing, 161 cached controls and inline
    // coefficient targets fit in 16 KiB; factories report their actual size.
    const { assert!(EQ8_RUNTIME_BYTES <= 16_384) };
    let mut old_state = EQ8_DEFINITION.default_state().unwrap();
    old_state = patch_band(old_state, 0, 1, 0, 450.0, 8.0);
    old_state = patch_band(old_state, 0, 5, 0, 3_200.0, -5.0);
    let old = PreparedEq8::new(decode(&old_state).unwrap(), 48_000).unwrap();
    let mut source = old.runtime();
    source
        .set_audition_selection(Some(Eq8AuditionSelection::new(1, 0).unwrap()))
        .unwrap();
    let mut warm = test_signal(32);
    source.process(&mut warm);

    let mut new_state = EQ8_DEFINITION
        .patch_parameter(
            &old_state,
            parameter::frequency(0, 1),
            ParameterValue::Float(900.0),
        )
        .unwrap();
    new_state = EQ8_DEFINITION
        .patch_parameter(
            &new_state,
            parameter::OUTPUT_GAIN,
            ParameterValue::Float(1.4),
        )
        .unwrap();
    let new = PreparedEq8::new(decode(&new_state).unwrap(), 48_000).unwrap();
    let mut whole = new.runtime();
    let mut split = new.runtime();
    assert!(whole.inherit_history_from(&source));
    assert!(split.inherit_history_from(&source));
    assert!(whole.can_retarget_parameters_from(&source));
    assert_eq!(whole.audition_selection, source.audition_selection);
    assert_eq!(whole.audition_mix, source.audition_mix);
    assert_eq!(whole.audition_history, source.audition_history);

    let input = test_signal(640);
    let mut whole_audio = input.clone();
    whole.process(&mut whole_audio[..96]);
    let mut split_audio = input.clone();
    split.process(&mut split_audio[..1]);
    split.process(&mut split_audio[1..23]);
    split.process(&mut split_audio[23..96]);
    assert_eq!(whole_audio[..96], split_audio[..96]);

    // Reprepare the same live edit at an exact boundary and preserve both
    // normal cascade and audition recurrence while coefficient smoothing runs.
    let mut retarget_whole = new.runtime();
    let mut retarget_split = new.runtime();
    assert!(retarget_whole.inherit_history_from(&whole));
    assert!(retarget_split.inherit_history_from(&split));
    retarget_whole.process(&mut whole_audio[96..]);
    let mut offset = 96;
    for size in [1_usize, 17, 64, 127, 335] {
        let end = offset + size;
        retarget_split.process(&mut split_audio[offset..end]);
        offset = end;
    }
    assert_eq!(offset, split_audio.len());
    assert_eq!(whole_audio, split_audio);

    retarget_whole.set_audition_selection(None).unwrap();
    retarget_split.set_audition_selection(None).unwrap();
    let mut release_whole = test_signal(320);
    let mut release_split = release_whole.clone();
    retarget_whole.process(&mut release_whole);
    for part in release_split.chunks_mut(17) {
        retarget_split.process(part);
    }
    assert_eq!(
        release_whole, release_split,
        "release crossfade partition dependence"
    );
    assert_eq!(retarget_whole.history, retarget_split.history);
    assert_eq!(retarget_whole.audition_selection, None);
    assert_eq!(retarget_split.audition_selection, None);
}

#[test]
fn parameter_retarget_accepts_filter_edits_and_mode_crossfades_but_requires_same_rate_and_enabled_topology()
 {
    let mut state = EQ8_DEFINITION.default_state().unwrap();
    state = EQ8_DEFINITION
        .patch_parameter(&state, parameter::enabled(0, 1), ParameterValue::Bool(true))
        .unwrap();
    state = EQ8_DEFINITION
        .patch_parameter(&state, parameter::enabled(1, 1), ParameterValue::Bool(true))
        .unwrap();
    let source = PreparedEq8::new(decode(&state).unwrap(), 48_000)
        .unwrap()
        .runtime();

    let mut filter_edit = state.clone();
    for (parameter, value) in [
        (parameter::gain(0, 1), ParameterValue::Float(4.0)),
        (parameter::q(0, 1), ParameterValue::Float(1.4)),
        (parameter::kind(0, 1), ParameterValue::Choice(1)),
    ] {
        filter_edit = EQ8_DEFINITION
            .patch_parameter(&filter_edit, parameter, value)
            .unwrap();
    }
    let candidate = PreparedEq8::new(decode(&filter_edit).unwrap(), 48_000)
        .unwrap()
        .runtime();
    assert!(candidate.can_retarget_parameters_from(&source));

    let mode_changed = EQ8_DEFINITION
        .patch_parameter(&state, parameter::MODE, ParameterValue::Choice(1))
        .unwrap();
    let mode_candidate = PreparedEq8::new(decode(&mode_changed).unwrap(), 48_000)
        .unwrap()
        .runtime();
    // A mode change retargets through the ModeFade crossfade (Pulse #4106).
    assert!(mode_candidate.can_retarget_parameters_from(&source));

    let enabled_changed = EQ8_DEFINITION
        .patch_parameter(
            &state,
            parameter::enabled(0, 1),
            ParameterValue::Bool(false),
        )
        .unwrap();
    let enabled_candidate = PreparedEq8::new(decode(&enabled_changed).unwrap(), 48_000)
        .unwrap()
        .runtime();
    assert!(!enabled_candidate.can_retarget_parameters_from(&source));

    let wrong_rate = PreparedEq8::new(decode(&filter_edit).unwrap(), 44_100)
        .unwrap()
        .runtime();
    assert!(!wrong_rate.can_retarget_parameters_from(&source));
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
            meter: Default::default(),
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
    old.process_block(&mut material).unwrap();
    old.commit_block();
    assert!(!old.committed_range_is_quiescent(0..1));

    let mut next = chain_for(next_state);
    assert!(next.can_inherit_from(&old));
    next.inherit_from(&old);
    // The new lanes start empty, but the outgoing routing still crossfades
    // out (Pulse #4106), so the entry is not quiescent until the fade ends.
    assert!(!next.committed_range_is_quiescent(0..1));
    next.begin_block();
    let mut output = [[0.25, -0.1]; 32];
    next.process_block(&mut output).unwrap();
    next.commit_block();
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
}

#[test]
fn registered_eq8_chain_audition_uses_exact_enabled_owner_and_releases_both_twins() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorInstance, ProcessorAuditionSelection,
        ProcessorChainOwner, StereoMatrix,
    };

    let mut state = EQ8_DEFINITION.default_state().unwrap();
    state = EQ8_DEFINITION
        .patch_parameter(&state, parameter::enabled(0, 2), ParameterValue::Bool(true))
        .unwrap();
    let state = Arc::new(state);
    let definition: Arc<dyn hex_lattice::PreparedNativeProcessor> =
        Arc::new(PreparedEq8::new(decode(&state).unwrap(), 48_000).unwrap());
    let mut chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Return(7),
        device_id: 19,
        device_type: EQ8_TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(Arc::clone(&state)),
        definition: Some(Arc::clone(&definition)),
    }])
    .unwrap();
    let mut bypassed_chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Return(7),
        device_id: 19,
        device_type: EQ8_TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: true,
        sample_rate: 48_000,
        configuration: Some(state),
        definition: Some(definition),
    }])
    .unwrap();
    let selection = ProcessorAuditionSelection { index: 2, lane: 0 };

    assert!(!chain.select_processor_audition(
        ProcessorChainOwner::Track(7),
        19,
        EQ8_TYPE_ID,
        Some(selection),
    ));
    assert!(!chain.select_processor_audition(
        ProcessorChainOwner::Return(7),
        19,
        DeviceTypeId::from_static("hex.other"),
        Some(selection),
    ));
    assert!(!chain.select_processor_audition(
        ProcessorChainOwner::Return(7),
        19,
        EQ8_TYPE_ID,
        Some(ProcessorAuditionSelection { index: 3, lane: 0 }),
    ));
    assert!(!bypassed_chain.select_processor_audition(
        ProcessorChainOwner::Return(7),
        19,
        EQ8_TYPE_ID,
        Some(selection),
    ));
    assert!(chain.select_processor_audition(
        ProcessorChainOwner::Return(7),
        19,
        EQ8_TYPE_ID,
        Some(selection),
    ));
    assert!(chain.processor_audition_is_active(
        ProcessorChainOwner::Return(7),
        19,
        EQ8_TYPE_ID,
        selection,
    ));

    // A smooth release remains legal if the current authored band is disabled
    // or the device has since become bypassed.
    assert!(
        chain.select_processor_audition(ProcessorChainOwner::Return(7), 19, EQ8_TYPE_ID, None,)
    );
    assert!(chain.cancel_processor_audition(ProcessorChainOwner::Return(7), 19, EQ8_TYPE_ID,));
    assert!(!chain.processor_audition_is_active(
        ProcessorChainOwner::Return(7),
        19,
        EQ8_TYPE_ID,
        selection,
    ));
}

#[test]
fn registered_eq8_can_transfer_only_the_held_selector_to_a_compatible_candidate() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorInstance, ProcessorAuditionSelection,
        ProcessorChainOwner, StereoMatrix,
    };

    let owner = ProcessorChainOwner::Track(7);
    let selection = ProcessorAuditionSelection { index: 0, lane: 0 };
    let mut old_state = EQ8_DEFINITION.default_state().unwrap();
    for bank in 0..2 {
        old_state = EQ8_DEFINITION
            .patch_parameter(
                &old_state,
                parameter::enabled(bank, 0),
                ParameterValue::Bool(true),
            )
            .unwrap();
    }
    let mut new_state = EQ8_DEFINITION
        .patch_parameter(
            &old_state,
            parameter::gain(0, 0),
            ParameterValue::Float(6.0),
        )
        .unwrap();
    new_state = EQ8_DEFINITION
        .patch_parameter(&new_state, parameter::q(0, 0), ParameterValue::Float(1.7))
        .unwrap();

    let make_chain = |state: hex_lattice_contract::NativeDeviceState| {
        let state = Arc::new(state);
        let definition: Arc<dyn hex_lattice::PreparedNativeProcessor> =
            Arc::new(PreparedEq8::new(decode(&state).unwrap(), 48_000).unwrap());
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner,
            device_id: 19,
            device_type: EQ8_TYPE_ID,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(state),
            definition: Some(definition),
        }])
        .unwrap()
    };

    let mut old = make_chain(old_state.clone());
    assert!(old.select_processor_audition(owner, 19, EQ8_TYPE_ID, Some(selection)));
    let mut candidate = make_chain(new_state.clone());
    assert!(
        candidate.transfer_processor_audition_from(&old, owner, 19, EQ8_TYPE_ID, 0, selection,)
    );
    assert!(candidate.processor_audition_is_active(owner, 19, EQ8_TYPE_ID, selection));
    assert!(old.processor_audition_is_active(owner, 19, EQ8_TYPE_ID, selection));
    old.cancel_all_processor_auditions();
    assert!(!old.processor_audition_is_active(owner, 19, EQ8_TYPE_ID, selection));
    assert!(candidate.processor_audition_is_active(owner, 19, EQ8_TYPE_ID, selection));

    let mut old_for_excluded = make_chain(old_state);
    assert!(old_for_excluded.select_processor_audition(owner, 19, EQ8_TYPE_ID, Some(selection)));
    let mut excluded = make_chain(new_state);
    assert!(!excluded.transfer_processor_audition_from(
        &old_for_excluded,
        owner,
        19,
        EQ8_TYPE_ID,
        1,
        selection,
    ));
    assert!(!excluded.processor_audition_is_active(owner, 19, EQ8_TYPE_ID, selection));
}

/// Pulse #3961: handoff smoothing lasts the same time at every rate, so the
/// per-sample output-gain step shrinks as the rate grows.
#[test]
fn handoff_smoothing_time_is_rate_independent_with_bounded_gain_steps() {
    let old_state = EQ8_DEFINITION.default_state().unwrap();
    let new_state = EQ8_DEFINITION
        .patch_parameter(
            &old_state,
            parameter::OUTPUT_GAIN,
            ParameterValue::Float(1.5),
        )
        .unwrap();
    let reference_seconds = 256.0 / 48_000.0;
    for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        let old = PreparedEq8::new(decode(&old_state).unwrap(), rate)
            .unwrap()
            .runtime();
        let prepared = PreparedEq8::new(decode(&new_state).unwrap(), rate).unwrap();
        let step = (prepared.output_gain - old.current_gain).abs();
        let mut runtime = prepared.runtime();
        assert!(runtime.inherit_history_from(&old));
        let frames = usize::from(runtime.smooth_remaining);
        assert!(
            (frames as f64 / f64::from(rate) - reference_seconds).abs() <= 1.0 / f64::from(rate),
            "rate={rate}: {frames} frames"
        );
        let mut chunked = prepared.runtime();
        assert!(chunked.inherit_history_from(&old));
        let mut whole_audio = vec![[0.25_f32, -0.125]; frames + 8];
        let mut chunked_audio = whole_audio.clone();
        let mut previous = runtime.current_gain;
        let mut largest = 0.0_f64;
        for frame in whole_audio.iter_mut() {
            runtime.process(std::slice::from_mut(frame));
            largest = largest.max((runtime.current_gain - previous).abs());
            previous = runtime.current_gain;
        }
        let mut offset = 0;
        for size in [1_usize, 7, 64, 128].into_iter().cycle() {
            if offset == chunked_audio.len() {
                break;
            }
            let end = (offset + size).min(chunked_audio.len());
            chunked.process(&mut chunked_audio[offset..end]);
            offset = end;
        }
        assert_eq!(whole_audio, chunked_audio, "rate={rate}");
        let mut last_output = 0.25_f64 * old.current_gain;
        for frame in &whole_audio {
            let output = f64::from(frame[0]);
            assert!(
                (output - last_output).abs() <= 0.25 * step / frames as f64 + 1.0e-7,
                "rate={rate}: audible gain step exceeded the ramp"
            );
            last_output = output;
        }
        assert!((last_output - 0.25 * runtime.target_gain).abs() < 1.0e-7);
        assert_eq!(runtime.smooth_remaining, 0);
        assert_eq!(runtime.current_gain, runtime.target_gain);
        assert!(
            largest <= step / frames as f64 * (1.0 + 1.0e-9),
            "rate={rate}: step {largest} over {frames} frames"
        );
    }
    assert_eq!(coefficient_smooth_frames(48_000), 256);
    assert_eq!(coefficient_smooth_frames(192_000), 1_024);
}
