use super::*;

fn patch(state: &NativeDeviceState, id: ParameterId, value: ParameterValue) -> NativeDeviceState {
    OPERATOR_METADATA
        .patch_parameter(state, id, value)
        .expect("valid tuning patch")
}

#[test]
fn version_two_migration_preserves_waveform_prefix_and_legacy_pcm() {
    let mut current = OPERATOR_METADATA.default_state().expect("default");
    current = patch(&current, parameter::waveform(2), ParameterValue::Choice(2));
    let legacy = NativeDeviceState::new(
        OPERATOR_TYPE_ID,
        2,
        current.payload()[..WAVEFORM_PAYLOAD_BYTES].to_vec(),
    )
    .expect("version two");
    let migrated = OPERATOR_METADATA.migrate_state(&legacy).expect("migration");
    assert_eq!(
        &migrated.payload()[..WAVEFORM_PAYLOAD_BYTES],
        legacy.payload()
    );
    assert_eq!(migrated, current);
    let table = SineTable::new();
    let old = OperatorSettings::from_state(&legacy)
        .expect("old")
        .prepare_note(48_000, 60, 100)
        .expect("old note");
    let new = OperatorSettings::from_state(&migrated)
        .expect("new")
        .prepare_note(48_000, 60, 100)
        .expect("new note");
    for age in 0..8192 {
        assert_eq!(
            old.sample(&table, age, 6000).to_bits(),
            new.sample(&table, age, 6000).to_bits()
        );
    }
}

#[test]
fn registered_fixed_notes_ignore_pitch_and_generic_state_roundtrips() {
    let mut state = OPERATOR_METADATA.default_state().expect("default");
    for operator in 1..=4 {
        state = patch(
            &state,
            parameter::fixed_frequency(operator),
            ParameterValue::Float(200.0 + operator as f32),
        );
        state = patch(
            &state,
            parameter::fixed_multiplier(operator),
            ParameterValue::Float(2.0),
        );
        state = patch(
            &state,
            parameter::fixed(operator),
            ParameterValue::Bool(true),
        );
    }
    let decoded = OperatorSettings::from_state(&state).expect("settings");
    assert_eq!(decoded.to_state().expect("encode"), state);
    let instrument = OPERATOR_METADATA
        .prepare_instrument(&state, 48_000)
        .expect("instrument");
    let context = instrument
        .prepare_shared_analytic_context()
        .expect("context")
        .expect("shared class");
    let low = context.prepare_note(0, 100).expect("low");
    let high = context.prepare_note(127, 100).expect("high");
    for age in 0..8192 {
        assert_eq!(
            context.sample(&low, age, 6000),
            context.sample(&high, age, 6000)
        );
    }
    let ratio = patch(&state, parameter::fixed(1), ParameterValue::Bool(false));
    assert_ne!(ratio, state);
    let context = OPERATOR_METADATA
        .prepare_instrument(&ratio, 48_000)
        .expect("mixed")
        .prepare_shared_analytic_context()
        .expect("context")
        .expect("shared");
    let low = context.prepare_note(48, 100).expect("low");
    let high = context.prepare_note(60, 100).expect("high");
    assert!(
        (1000..4000).any(|age| context.sample(&low, age, 6000) != context.sample(&high, age, 6000))
    );
}

#[test]
fn malformed_tuning_is_refused_even_when_fixed_mode_is_disabled() {
    let original = OPERATOR_METADATA.default_state().expect("default");
    for operator in 0..4 {
        for (field, value) in [
            (0, 2_u32),
            (4, f32::NAN.to_bits()),
            (4, 0.0_f32.to_bits()),
            (8, f32::INFINITY.to_bits()),
            (8, 0.0_f32.to_bits()),
        ] {
            let mut bytes = original.payload().to_vec();
            let offset = WAVEFORM_PAYLOAD_BYTES + operator * 12 + field;
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            let malformed =
                NativeDeviceState::new(OPERATOR_TYPE_ID, 3, bytes).expect("opaque data");
            assert!(OperatorSettings::from_state(&malformed).is_err());
            assert!(OPERATOR_METADATA.migrate_state(&malformed).is_err());
        }
    }
    for size in [WAVEFORM_PAYLOAD_BYTES, PAYLOAD_BYTES - 1, PAYLOAD_BYTES + 1] {
        let malformed =
            NativeDeviceState::new(OPERATOR_TYPE_ID, 3, vec![0; size]).expect("opaque data");
        assert!(OperatorSettings::from_state(&malformed).is_err());
    }
    assert_eq!(
        original,
        OPERATOR_METADATA
            .default_state()
            .expect("unchanged defaults")
    );
}
