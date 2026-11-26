use super::*;

#[test]
fn concrete_memoryless_preparation_refuses_unresolved_asset_bindings() {
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    let source = UTILITY_DEFINITION.default_state().unwrap();
    assert!(
        UtilityDefinition::prepare_owned_filter(&source, 48000)
            .unwrap()
            .is_none()
    );
    let bound = source
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(42).unwrap(),
        }])
        .unwrap();
    assert!(matches!(
        UtilityDefinition::prepare_owned_filter(&bound, 48000),
        Err(DeviceContractError::AssetResourcesUnavailable)
    ));
}

#[allow(clippy::too_many_arguments)]
fn settings(
    gain: f32,
    balance: f32,
    width: f32,
    mode: ChannelMode,
    invert_left: bool,
    invert_right: bool,
    mono: bool,
    mute: bool,
    mid_side_balance: Option<f32>,
) -> UtilitySettings {
    UtilitySettings::new(
        gain,
        balance,
        width,
        mode,
        invert_left,
        invert_right,
        mono,
        mute,
        mid_side_balance,
    )
    .unwrap()
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.000_01,
        "{actual} != {expected}"
    );
}

#[test]
fn default_is_exact_identity_and_declares_zero_latency() {
    let utility = PreparedUtility::new(UtilitySettings::default()).unwrap();
    let input = [f32::from_bits(0x8000_0000), 0.75];
    let output = utility.process_frame(input);
    assert_eq!(output[0].to_bits(), input[0].to_bits());
    assert_eq!(output[1].to_bits(), input[1].to_bits());
    assert_eq!(utility.latency_frames(), 0);
}

#[test]
fn prepared_utility_filter_variants_declare_zero_algorithmic_latency() {
    let default_state = UTILITY_DEFINITION.default_state().unwrap();
    for (dc_remove, bass_mono) in [(true, false), (false, true), (true, true)] {
        let state = UTILITY_DEFINITION
            .patch_parameter(
                &default_state,
                parameter::DC_REMOVE,
                ParameterValue::Bool(dc_remove),
            )
            .and_then(|state| {
                UTILITY_DEFINITION.patch_parameter(
                    &state,
                    parameter::BASS_MONO,
                    ParameterValue::Bool(bass_mono),
                )
            })
            .unwrap();
        let prepared = UTILITY_DEFINITION
            .prepare_processor(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(prepared.algorithmic_latency_frames(), Some(0));
        assert_eq!(prepared.output_tail(), OutputTail::Unbounded);
        assert!(prepared.create_runtime().is_quiescent());
    }
}

#[test]
fn registered_utility_mute_ramps_and_suppresses_the_settled_filter_residual() {
    use hex_lattice_contract::ParameterValue;

    let default_state = UTILITY_DEFINITION.default_state().unwrap();
    for (dc_remove, bass_mono, audition) in [
        (true, false, false),
        (false, true, false),
        (false, true, true),
    ] {
        let mut state = default_state.clone();
        for (id, value) in [
            (parameter::DC_REMOVE, ParameterValue::Bool(dc_remove)),
            (parameter::BASS_MONO, ParameterValue::Bool(bass_mono)),
            (
                parameter::BASS_MONO_AUDITION,
                ParameterValue::Bool(audition),
            ),
        ] {
            state = UTILITY_DEFINITION
                .patch_parameter(&state, id, value)
                .unwrap();
        }
        let mut source = UTILITY_DEFINITION
            .prepare_processor(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap()
            .create_runtime();
        let matrix = UTILITY_DEFINITION.prepare_effect_matrix(&state).unwrap();
        for _ in 0..512 {
            let mut frame = [[0.75, -0.25]];
            matrix.apply_block(&mut frame);
            source.process_block(&mut frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        }

        let muted_state = UTILITY_DEFINITION
            .patch_parameter(&state, parameter::MUTE, ParameterValue::Bool(true))
            .unwrap();
        let muted_matrix = UTILITY_DEFINITION
            .prepare_effect_matrix(&muted_state)
            .unwrap();
        let mut candidate = UTILITY_DEFINITION
            .prepare_processor(
                &muted_state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap()
            .create_runtime();
        assert!(candidate.can_copy_history_from(source.as_ref()));
        candidate.copy_history_from(source.as_ref());

        const RAMP_FRAMES: usize = 240;
        let mut muted = [[0.5, -0.125]; RAMP_FRAMES];
        muted_matrix.apply_block(&mut muted);
        let mut oracle = muted;
        oracle[RAMP_FRAMES - 1] = [0.0; 2];
        source.process_block(&mut oracle, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        candidate.process_block(&mut muted, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        for (frame, (actual, expected)) in muted.into_iter().zip(oracle).enumerate() {
            let gain = 1.0 - (frame + 1) as f32 / RAMP_FRAMES as f32;
            for channel in 0..2 {
                assert!((actual[channel] - expected[channel] * gain).abs() < 1.0e-7);
            }
        }

        let mut silence = [[0.5, -0.125]; 16];
        muted_matrix.apply_block(&mut silence);
        candidate.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(
            silence.iter().all(|frame| *frame == [0.0, 0.0]),
            "mute leaked filter residual for dc={dc_remove}, bass={bass_mono}, audition={audition}"
        );

        let mut oracle_silence = [[0.0, 0.0]; 16];
        source.process_block(&mut oracle_silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut unmuted = UTILITY_DEFINITION
            .prepare_processor(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap()
            .create_runtime();
        assert!(unmuted.can_copy_history_from(candidate.as_ref()));
        unmuted.copy_history_from(candidate.as_ref());
        for frame in 0..RAMP_FRAMES + 16 {
            let input = [[0.2 + frame as f32 * 0.001, -0.4 + frame as f32 * 0.002]];
            let mut actual = input;
            let mut expected = input;
            unmuted.process_block(&mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            source.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let gain = ((frame + 1) as f32 / RAMP_FRAMES as f32).min(1.0);
            for channel in 0..2 {
                assert!((actual[0][channel] - expected[0][channel] * gain).abs() < 1.0e-7);
            }
        }
        candidate.copy_history_from(unmuted.as_ref());
        let mut signal = [[0.5, -0.125]];
        candidate.process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_ne!(signal, [[0.0; 2]], "mute starts from the audible history");
        candidate.reset();
        let mut signal = [[0.5, -0.125]];
        candidate.process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(signal, [[0.0; 2]], "reset applies the authored mute");
    }
}

#[test]
fn channel_modes_polarity_and_stereo_width_have_defined_matrices() {
    let frame = [0.4, -0.2];
    let left = PreparedUtility::new(settings(
        1.0,
        0.0,
        3.0,
        ChannelMode::Left,
        false,
        false,
        false,
        false,
        Some(1.0),
    ))
    .unwrap()
    .process_frame(frame);
    assert_eq!(left, [0.4, 0.4]);
    let right = PreparedUtility::new(settings(
        1.0,
        0.0,
        1.0,
        ChannelMode::Right,
        false,
        true,
        false,
        false,
        None,
    ))
    .unwrap()
    .process_frame(frame);
    assert_eq!(right, [0.2, 0.2]);
    let swapped_and_inverted = PreparedUtility::new(settings(
        1.0,
        0.0,
        1.0,
        ChannelMode::Swap,
        true,
        false,
        false,
        false,
        None,
    ))
    .unwrap()
    .process_frame(frame);
    assert_eq!(swapped_and_inverted, [-0.2, -0.4]);
    let mono_width = PreparedUtility::new(settings(
        1.0,
        0.0,
        0.0,
        ChannelMode::Stereo,
        false,
        false,
        false,
        false,
        None,
    ))
    .unwrap()
    .process_frame(frame);
    assert_eq!(mono_width, [0.1, 0.1]);
    let wide = PreparedUtility::new(settings(
        1.0,
        0.0,
        2.0,
        ChannelMode::Stereo,
        false,
        false,
        false,
        false,
        None,
    ))
    .unwrap()
    .process_frame(frame);
    close(wide[0], 0.7);
    close(wide[1], -0.5);
}

#[test]
fn mid_side_balance_endpoints_and_mono_are_explicit() {
    let input = [0.8, 0.2];
    let mid_only = PreparedUtility::new(settings(
        1.0,
        0.0,
        3.0,
        ChannelMode::Stereo,
        false,
        false,
        false,
        false,
        Some(-1.0),
    ))
    .unwrap()
    .process_frame(input);
    assert_eq!(mid_only, [0.5, 0.5]);
    let side_only = PreparedUtility::new(settings(
        1.0,
        0.0,
        3.0,
        ChannelMode::Stereo,
        false,
        false,
        false,
        false,
        Some(1.0),
    ))
    .unwrap()
    .process_frame(input);
    assert_eq!(side_only, [0.3, -0.3]);
    let mono = PreparedUtility::new(settings(
        1.0,
        0.0,
        2.0,
        ChannelMode::Stereo,
        false,
        false,
        true,
        false,
        None,
    ))
    .unwrap()
    .process_frame(input);
    assert_eq!(mono, [0.5, 0.5]);
}

#[test]
fn output_balance_gain_zero_mute_and_block_processing_are_defined() {
    let right_only = PreparedUtility::new(settings(
        2.0,
        1.0,
        1.0,
        ChannelMode::Stereo,
        false,
        false,
        false,
        false,
        None,
    ))
    .unwrap();
    assert_eq!(right_only.process_frame([0.4, 0.3]), [0.0, 0.6]);
    let silent = PreparedUtility::new(settings(
        0.0,
        0.0,
        1.0,
        ChannelMode::Stereo,
        false,
        false,
        false,
        false,
        None,
    ))
    .unwrap();
    assert_eq!(silent.process_frame([1.0, -1.0]), [0.0, 0.0]);
    let muted = PreparedUtility::new(settings(
        1.0,
        0.0,
        1.0,
        ChannelMode::Stereo,
        false,
        false,
        false,
        true,
        None,
    ))
    .unwrap();
    assert_eq!(muted.process_frame([1.0, -1.0]), [0.0, 0.0]);

    let mut block = [[0.2, 0.4], [-0.6, 0.8], [0.0, 0.1]];
    let expected = block.map(|frame| right_only.process_frame(frame));
    right_only.process_block(&mut block);
    assert_eq!(block, expected);
    assert!(
        PreparedUtility::new(settings(
            MAX_UTILITY_GAIN,
            -1.0,
            4.0,
            ChannelMode::Stereo,
            true,
            true,
            false,
            false,
            None,
        ))
        .is_ok()
    );
}

#[test]
fn invalid_utility_settings_are_rejected() {
    let invalid = [
        (f32::NAN, 0.0, 1.0, None),
        (f32::INFINITY, 0.0, 1.0, None),
        (-0.1, 0.0, 1.0, None),
        (MAX_UTILITY_GAIN + 1.0, 0.0, 1.0, None),
        (1.0, f32::NAN, 1.0, None),
        (1.0, 1.1, 1.0, None),
        (1.0, 0.0, f32::INFINITY, None),
        (1.0, 0.0, -0.1, None),
        (1.0, 0.0, 4.1, None),
        (1.0, 0.0, 1.0, Some(f32::NAN)),
        (1.0, 0.0, 1.0, Some(-1.1)),
        (1.0, 0.0, 1.0, Some(1.1)),
    ];
    for (gain, balance, width, ms) in invalid {
        assert!(
            UtilitySettings::new(
                gain,
                balance,
                width,
                ChannelMode::Stereo,
                false,
                false,
                false,
                false,
                ms,
            )
            .is_err()
        );
    }
}

#[test]
fn utility_definition_owns_validated_portable_state_and_generic_metadata() {
    use hex_lattice::NativeDeviceRegistry;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterPresentation, ParameterValue};

    let definitions: [&dyn NativeDeviceDefinition; 1] = [&UTILITY_DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let state = registry.default_state(UTILITY_TYPE_ID).unwrap();
    assert_eq!(registry.migrate_state(&state).unwrap(), state);
    assert_eq!(
        registry
            .definition(UTILITY_TYPE_ID)
            .unwrap()
            .descriptors()
            .len(),
        14
    );
    for descriptor in UTILITY_DEFINITION.descriptors() {
        descriptor.validate_descriptor().unwrap();
    }
    assert_eq!(
        UTILITY_DEFINITION.descriptors()[0].presentation,
        ParameterPresentation::LinearAmplitudeDecibels
    );
    assert!(matches!(
        UTILITY_DEFINITION.descriptors()[0].kind,
        ParameterKind::Float {
            unit: Some("linear"),
            ..
        }
    ));
    assert!(
        UTILITY_DEFINITION.descriptors()[1..]
            .iter()
            .all(|descriptor| descriptor.presentation == ParameterPresentation::Linear)
    );

    let patched = registry
        .patch_parameter(&state, parameter::GAIN, ParameterValue::Float(2.0))
        .unwrap();
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&patched, parameter::GAIN)
            .unwrap(),
        ParameterValue::Float(2.0)
    );
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&patched, parameter::WIDTH)
            .unwrap(),
        ParameterValue::Float(1.0)
    );
    assert!(
        registry
            .patch_parameter(&patched, parameter::GAIN, ParameterValue::Float(f32::NAN))
            .is_err()
    );
    assert!(
        registry
            .patch_parameter(
                &patched,
                parameter::CHANNEL,
                ParameterValue::Choice(u32::MAX)
            )
            .is_err()
    );

    let malformed = NativeDeviceState::new(UTILITY_TYPE_ID, 1, vec![0; 18]).unwrap();
    assert!(registry.migrate_state(&malformed).is_err());
    let future = NativeDeviceState::new(UTILITY_TYPE_ID, 99, patched.payload().to_vec()).unwrap();
    assert!(registry.migrate_state(&future).is_err());
}

#[test]
fn utility_width_and_mid_side_availability_tracks_channel_mode_without_resetting_values() {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mut state = UTILITY_DEFINITION.default_state().unwrap();
    for (parameter, value) in [
        (parameter::WIDTH, ParameterValue::Float(2.5)),
        (parameter::MID_SIDE_ENABLED, ParameterValue::Bool(true)),
        (parameter::MID_SIDE_BALANCE, ParameterValue::Float(-0.4)),
    ] {
        state = UTILITY_DEFINITION
            .patch_parameter(&state, parameter, value)
            .unwrap();
    }
    let conditional = [
        parameter::WIDTH,
        parameter::MID_SIDE_ENABLED,
        parameter::MID_SIDE_BALANCE,
    ];

    for mode in [1, 2] {
        let left_or_right = UTILITY_DEFINITION
            .patch_parameter(&state, parameter::CHANNEL, ParameterValue::Choice(mode))
            .unwrap();
        for parameter in conditional {
            assert!(
                !UTILITY_DEFINITION
                    .parameter_enabled(&left_or_right, parameter)
                    .unwrap()
            );
        }
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&left_or_right, parameter::WIDTH)
                .unwrap(),
            ParameterValue::Float(2.5)
        );
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&left_or_right, parameter::MID_SIDE_ENABLED)
                .unwrap(),
            ParameterValue::Bool(true)
        );
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&left_or_right, parameter::MID_SIDE_BALANCE)
                .unwrap(),
            ParameterValue::Float(-0.4)
        );
    }

    for mode in [0, 3] {
        let stereo_or_swap = UTILITY_DEFINITION
            .patch_parameter(&state, parameter::CHANNEL, ParameterValue::Choice(mode))
            .unwrap();
        for parameter in conditional {
            assert!(
                UTILITY_DEFINITION
                    .parameter_enabled(&stereo_or_swap, parameter)
                    .unwrap()
            );
        }
    }

    assert!(
        UTILITY_DEFINITION
            .parameter_enabled(&state, ParameterId::from_static(99))
            .is_err()
    );
    let malformed = NativeDeviceState::new(UTILITY_TYPE_ID, 2, vec![0; 19]).unwrap();
    assert!(
        UTILITY_DEFINITION
            .parameter_enabled(&malformed, parameter::WIDTH)
            .is_err()
    );
}

#[test]
fn schema_one_utility_state_migrates_to_neutral_stateful_defaults() {
    use hex_lattice::NativeDeviceRegistry;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let definitions: [&dyn NativeDeviceDefinition; 1] = [&UTILITY_DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let schema_two = registry.default_state(UTILITY_TYPE_ID).unwrap();
    let schema_one =
        NativeDeviceState::new(UTILITY_TYPE_ID, 1, schema_two.payload()[..19].to_vec()).unwrap();
    let migrated = registry.migrate_state(&schema_one).unwrap();
    assert_eq!(migrated.state_version(), 2);
    assert_eq!(migrated.payload().len(), 26);
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&migrated, parameter::DC_REMOVE)
            .unwrap(),
        ParameterValue::Bool(false)
    );
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&migrated, parameter::BASS_MONO)
            .unwrap(),
        ParameterValue::Bool(false)
    );
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&migrated, parameter::BASS_MONO_CUTOFF)
            .unwrap(),
        ParameterValue::Float(200.0)
    );
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&migrated, parameter::BASS_MONO_AUDITION)
            .unwrap(),
        ParameterValue::Bool(false)
    );
}

#[test]
fn inactive_mid_side_balance_remains_validated_and_round_trips_independently() {
    use hex_lattice::NativeDeviceRegistry;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let definitions: [&dyn NativeDeviceDefinition; 1] = [&UTILITY_DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let mut state = registry.default_state(UTILITY_TYPE_ID).unwrap();

    state = registry
        .patch_parameter(
            &state,
            parameter::MID_SIDE_BALANCE,
            ParameterValue::Float(-0.75),
        )
        .unwrap();
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&state, parameter::MID_SIDE_ENABLED)
            .unwrap(),
        ParameterValue::Bool(false)
    );
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&state, parameter::MID_SIDE_BALANCE)
            .unwrap(),
        ParameterValue::Float(-0.75)
    );

    state = registry
        .patch_parameter(
            &state,
            parameter::MID_SIDE_ENABLED,
            ParameterValue::Bool(true),
        )
        .unwrap();
    state = registry
        .patch_parameter(
            &state,
            parameter::MID_SIDE_BALANCE,
            ParameterValue::Float(0.75),
        )
        .unwrap();
    state = registry
        .patch_parameter(
            &state,
            parameter::MID_SIDE_ENABLED,
            ParameterValue::Bool(false),
        )
        .unwrap();
    state = registry
        .patch_parameter(&state, parameter::GAIN, ParameterValue::Float(2.0))
        .unwrap();
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&state, parameter::MID_SIDE_BALANCE)
            .unwrap(),
        ParameterValue::Float(0.75)
    );
    state = registry
        .patch_parameter(
            &state,
            parameter::MID_SIDE_ENABLED,
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&state, parameter::MID_SIDE_BALANCE)
            .unwrap(),
        ParameterValue::Float(0.75)
    );

    let mut malformed_payload = registry
        .default_state(UTILITY_TYPE_ID)
        .unwrap()
        .payload()
        .to_vec();
    malformed_payload[15..19].copy_from_slice(&f32::NAN.to_le_bytes());
    let malformed = NativeDeviceState::new(UTILITY_TYPE_ID, 2, malformed_payload).unwrap();
    assert!(registry.migrate_state(&malformed).is_err());
}

#[test]
fn legacy_nineteen_byte_state_migrates_to_filter_defaults() {
    use hex_lattice::NativeDeviceRegistry;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let definitions: [&dyn NativeDeviceDefinition; 1] = [&UTILITY_DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let legacy = NativeDeviceState::new(
        UTILITY_TYPE_ID,
        1,
        UTILITY_DEFINITION.default_state().unwrap().payload()[..19].to_vec(),
    )
    .unwrap();
    let migrated = registry.migrate_state(&legacy).unwrap();
    assert_eq!(migrated.state_version(), 2);
    assert_eq!(migrated.payload().len(), 26);
    for (id, expected) in [
        (parameter::DC_REMOVE, ParameterValue::Bool(false)),
        (parameter::BASS_MONO, ParameterValue::Bool(false)),
        (parameter::BASS_MONO_CUTOFF, ParameterValue::Float(200.0)),
        (parameter::BASS_MONO_AUDITION, ParameterValue::Bool(false)),
    ] {
        assert_eq!(
            UTILITY_DEFINITION.read_parameter(&migrated, id).unwrap(),
            expected
        );
    }
}
