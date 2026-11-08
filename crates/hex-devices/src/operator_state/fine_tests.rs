use super::*;

#[test]
fn fine_offsets_have_stable_domains_and_round_trip_independently() {
    let mut state = OperatorSettings::defaults().to_state().unwrap();
    for operator in 1..=4 {
        let id = parameter::fine_offset(operator);
        assert_eq!(id.value(), 608 + operator);
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&state, id).unwrap(),
            ParameterValue::Float(0.0)
        );
        for value in [-1000.0, -0.25, 0.0, 0.5, 1000.0] {
            state = OPERATOR_METADATA
                .patch_parameter(&state, id, ParameterValue::Float(value))
                .unwrap();
            let restored = OperatorSettings::from_state(&state)
                .unwrap()
                .to_state()
                .unwrap();
            assert_eq!(
                OPERATOR_METADATA.read_parameter(&restored, id).unwrap(),
                ParameterValue::Float(value)
            );
        }
        for value in [f32::NAN, f32::INFINITY, -1000.01, 1000.01] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&state, id, ParameterValue::Float(value))
                    .is_err()
            );
        }
    }
    assert_eq!(parameter::fine_offset(0).value(), 0);
    assert_eq!(parameter::fine_offset(5).value(), 0);
    assert_eq!(
        OperatorSettings::from_state(&state).unwrap().fine.values(),
        [1000.0; 4]
    );
}

#[test]
fn version_fifteen_preserves_global_modulation_prefix_and_exact_zero_offset_sound() {
    let mut state = OperatorSettings::defaults().to_state().unwrap();
    for (id, value) in [
        (601, ParameterValue::Choice(3)),
        (605, ParameterValue::Float(17.0)),
        (620, ParameterValue::Choice(1)),
    ] {
        state = OPERATOR_METADATA
            .patch_parameter(&state, ParameterId::from_static(id), value)
            .unwrap();
    }
    let legacy = NativeDeviceState::new(
        OPERATOR_TYPE_ID,
        15,
        state.payload()[..MODULATION_PAYLOAD_BYTES].to_vec(),
    )
    .unwrap();
    let migrated = OPERATOR_METADATA.migrate_state(&legacy).unwrap();
    assert_eq!(migrated, state);
    assert_eq!(
        &migrated.payload()[..MODULATION_PAYLOAD_BYTES],
        legacy.payload()
    );
    let old = OperatorSettings::from_state(&legacy)
        .unwrap()
        .prepare_configuration(48_000)
        .unwrap();
    let new = OperatorSettings::from_state(&migrated)
        .unwrap()
        .prepare_configuration(48_000)
        .unwrap();
    let table = SineTable::new();
    for pitch in [0, 60, 69, 127] {
        let a = old.prepare_note(pitch, 100).unwrap();
        let b = new.prepare_note(pitch, 100).unwrap();
        assert_eq!(a, b);
        for age in [0, 1, 97, 1024, 8192] {
            assert_eq!(
                old.sample(&table, &a, age, 6000).to_bits(),
                new.sample(&table, &b, age, 6000).to_bits()
            );
        }
    }
}

#[test]
fn fine_suffix_rejects_malformed_lengths_and_values() {
    let state = OperatorSettings::defaults().to_state().unwrap();
    for length in [
        MODULATION_PAYLOAD_BYTES,
        PAYLOAD_BYTES - 1,
        PAYLOAD_BYTES + 1,
    ] {
        let mut bytes = state.payload().to_vec();
        bytes.resize(length, 0);
        let malformed =
            NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, bytes).unwrap();
        assert!(OperatorSettings::from_state(&malformed).is_err());
    }
    for operator in 0..4 {
        for value in [f32::NAN, f32::NEG_INFINITY, -1000.01, 1000.01] {
            let mut bytes = state.payload().to_vec();
            let offset = MODULATION_PAYLOAD_BYTES + operator * 4;
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            let malformed =
                NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, bytes).unwrap();
            assert!(OperatorSettings::from_state(&malformed).is_err());
        }
    }
}

#[test]
fn fine_parameter_changes_prepared_frequency_after_ratio_and_fixed_multiplier() {
    for fixed in [false, true] {
        let mut state = OperatorSettings::defaults().to_state().unwrap();
        for (id, value) in [
            (parameter::oscillator_ratio(1), ParameterValue::Float(2.0)),
            (parameter::fixed_frequency(1), ParameterValue::Float(440.0)),
            (parameter::fixed_multiplier(1), ParameterValue::Float(2.0)),
            (parameter::fixed(1), ParameterValue::Bool(fixed)),
            (parameter::fine_offset(1), ParameterValue::Float(-100.0)),
        ] {
            state = OPERATOR_METADATA
                .patch_parameter(&state, id, value)
                .unwrap();
        }
        let actual = OperatorSettings::from_state(&state)
            .unwrap()
            .prepare_configuration(48_000)
            .unwrap();
        let mut reference = state.clone();
        for (id, value) in [
            (parameter::fine_offset(1), ParameterValue::Float(0.0)),
            (parameter::fixed_frequency(1), ParameterValue::Float(780.0)),
            (parameter::fixed_multiplier(1), ParameterValue::Float(1.0)),
            (parameter::fixed(1), ParameterValue::Bool(true)),
        ] {
            reference = OPERATOR_METADATA
                .patch_parameter(&reference, id, value)
                .unwrap();
        }
        let expected = OperatorSettings::from_state(&reference)
            .unwrap()
            .prepare_configuration(48_000)
            .unwrap();
        let a = actual.prepare_note(69, 127).unwrap();
        let b = expected.prepare_note(69, 127).unwrap();
        let table = SineTable::new();
        for age in [1, 97, 1024, 8192] {
            assert_eq!(
                actual.sample(&table, &a, age, 6000).to_bits(),
                expected.sample(&table, &b, age, 6000).to_bits()
            );
        }
    }
}
