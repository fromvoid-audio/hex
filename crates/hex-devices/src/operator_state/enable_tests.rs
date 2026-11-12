use super::*;

#[test]
fn enable_migration_preserves_each_valid_prefix_and_defaults_every_oscillator_on() {
    let current = tests::legacy_state();
    assert_eq!(current.payload().len(), PAYLOAD_BYTES);
    assert_eq!(PARAMETER_COUNT, 432);
    for (version, length) in [
        (1, 224),
        (2, 240),
        (3, 288),
        (4, 304),
        (5, 320),
        (6, 320),
        (7, 364),
        (8, 380),
    ] {
        let old = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            version,
            current.payload()[..length].to_vec(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&old).unwrap();
        assert_eq!(migrated.state_version(), OPERATOR_STATE_VERSION);
        assert_eq!(&migrated.payload()[..length], old.payload());
        for operator in 1..=4 {
            assert_eq!(
                OPERATOR_METADATA.read_parameter(&migrated, parameter::enabled(operator)),
                Ok(ParameterValue::Bool(true))
            );
        }
    }
    // Non-default LFO data is part of the preserved v7 prefix too.
    let with_lfo = OPERATOR_METADATA
        .patch_parameter(&current, parameter::LFO_RATE, ParameterValue::Float(4.25))
        .unwrap();
    let old =
        NativeDeviceState::new(OPERATOR_TYPE_ID, 7, with_lfo.payload()[..364].to_vec()).unwrap();
    let migrated = OPERATOR_METADATA.migrate_state(&old).unwrap();
    assert_eq!(&migrated.payload()[..364], old.payload());
    assert_eq!(
        OPERATOR_METADATA.read_parameter(&migrated, parameter::LFO_RATE),
        Ok(ParameterValue::Float(4.25))
    );
}

#[test]
fn enable_boolean_edit_is_atomic_and_keeps_the_complete_authored_prefix() {
    let original = tests::legacy_state();
    let mut changed = original.clone();
    for operator in 1..=4 {
        changed = OPERATOR_METADATA
            .patch_parameter(
                &changed,
                parameter::enabled(operator),
                ParameterValue::Bool(false),
            )
            .unwrap();
        assert_eq!(&changed.payload()[..364], &original.payload()[..364]);
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&changed, parameter::enabled(operator)),
            Ok(ParameterValue::Bool(false))
        );
        assert_eq!(
            OperatorSettings::from_state(&changed)
                .unwrap()
                .to_state()
                .unwrap(),
            changed
        );
        for value in [ParameterValue::Float(0.0), ParameterValue::Choice(0)] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&changed, parameter::enabled(operator), value)
                    .is_err()
            );
        }
    }
    for operator in 1..=4 {
        changed = OPERATOR_METADATA
            .patch_parameter(
                &changed,
                parameter::enabled(operator),
                ParameterValue::Bool(true),
            )
            .unwrap();
    }
    assert_eq!(changed, original);
    assert!(parameter::enabled(0).validate().is_err());
    assert!(parameter::enabled(5).validate().is_err());
}

#[test]
fn enable_suffix_refuses_nonbinary_flags_wrong_lengths_and_legacy_layout_aliases() {
    let original = tests::legacy_state();
    for operator in 0..4 {
        for word in [2_u32, u32::MAX, f32::NAN.to_bits()] {
            let mut bytes = original.payload()[..380].to_vec();
            bytes[364 + operator * 4..368 + operator * 4].copy_from_slice(&word.to_le_bytes());
            let invalid =
                NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, bytes).unwrap();
            assert!(OPERATOR_METADATA.migrate_state(&invalid).is_err());
        }
    }
    for (version, length) in [(8, 364), (8, 379), (8, 381), (7, 380), (6, 364), (9, 380)] {
        let mut bytes = original.payload().to_vec();
        bytes.resize(length, 0);
        assert!(
            OperatorSettings::from_state(
                &NativeDeviceState::new(OPERATOR_TYPE_ID, version, bytes).unwrap()
            )
            .is_err()
        );
    }
    // Strict v6/v7 durations must not acquire a moving legacy clamp on v8.
    for (version, length) in [(6, 320), (7, 364), (8, 380)] {
        let mut bytes = original.payload()[..length].to_vec();
        bytes[35 * 4..36 * 4].copy_from_slice(&21.0_f32.to_le_bytes());
        assert!(
            OperatorSettings::from_state(
                &NativeDeviceState::new(OPERATOR_TYPE_ID, version, bytes).unwrap()
            )
            .is_err()
        );
    }
    assert_eq!(original, tests::legacy_state());
}
