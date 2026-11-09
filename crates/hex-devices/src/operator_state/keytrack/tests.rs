use super::*;

fn patch(state: &NativeDeviceState, operator: u32, value: f32) -> NativeDeviceState {
    OPERATOR_METADATA
        .patch_parameter(state, parameter(operator), ParameterValue::Float(value))
        .unwrap()
}

#[test]
fn independent_amounts_have_stable_ids_domains_and_current_roundtrips() {
    let mut state = OperatorSettings::defaults().to_state().unwrap();
    assert_eq!(state.state_version(), 17);
    assert_eq!(state.payload().len(), 1760);
    for i in 1..=4 {
        let id = crate::operator_state::parameter::pitch_keytrack(i);
        assert_eq!(id, parameter(i));
        assert_eq!(id.value(), 612 + i);
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&state, id).unwrap(),
            ParameterValue::Float(100.0)
        );
        for value in [0.0, 0.01, 25.0, 50.0, 99.99, 100.0] {
            state = patch(&state, i, value);
            assert_eq!(
                OPERATOR_METADATA.read_parameter(&state, id).unwrap(),
                ParameterValue::Float(value)
            );
            assert_eq!(
                OperatorSettings::from_state(&state)
                    .unwrap()
                    .to_state()
                    .unwrap(),
                state
            );
        }
        state = patch(&state, i, i as f32 * 20.0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 100.01] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&state, id, ParameterValue::Float(bad))
                    .is_err()
            );
        }
        assert!(
            OPERATOR_METADATA
                .patch_parameter(&state, id, ParameterValue::Bool(true))
                .is_err()
        );
    }
    assert_eq!(
        OperatorSettings::from_state(&state)
            .unwrap()
            .pitch_keytrack
            .values(),
        [20.0, 40.0, 60.0, 80.0]
    );
    assert_eq!(parameter(0).value(), 0);
    assert_eq!(parameter(5).value(), 0);
}

#[test]
fn old_fixed_toggles_and_new_amounts_keep_only_the_target_endpoint_coherent() {
    let mut state = OperatorSettings::defaults().to_state().unwrap();
    for i in 1..=4 {
        state = patch(&state, i, 25.0);
    }
    for i in 1..=4 {
        let fixed = crate::operator_state::parameter::fixed(i);
        for enabled in [true, false] {
            state = OPERATOR_METADATA
                .patch_parameter(&state, fixed, ParameterValue::Bool(enabled))
                .unwrap();
            assert_eq!(
                OPERATOR_METADATA
                    .read_parameter(&state, parameter(i))
                    .unwrap(),
                ParameterValue::Float(if enabled { 0.0 } else { 100.0 })
            );
            for neighbor in i + 1..=4 {
                assert_eq!(
                    OPERATOR_METADATA
                        .read_parameter(&state, parameter(neighbor))
                        .unwrap(),
                    ParameterValue::Float(25.0)
                );
            }
        }
        for amount in [0.0, 25.0, 100.0] {
            state = patch(&state, i, amount);
            assert_eq!(
                OPERATOR_METADATA.read_parameter(&state, fixed).unwrap(),
                ParameterValue::Bool(amount == 0.0)
            );
        }
    }
    state = patch(&state, 1, 50.0);
    let prefix = state.payload()[..FINE_PAYLOAD_BYTES].to_vec();
    for id in [
        crate::operator_state::parameter::fixed_frequency(1),
        crate::operator_state::parameter::fixed_multiplier(1),
    ] {
        state = OPERATOR_METADATA
            .patch_parameter(&state, id, ParameterValue::Float(2.0))
            .unwrap();
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&state, parameter(1))
                .unwrap(),
            ParameterValue::Float(50.0)
        );
    }
    assert_ne!(&state.payload()[..FINE_PAYLOAD_BYTES], prefix);
}

#[test]
fn every_supported_historical_prefix_migrates_to_matching_pitch_endpoints() {
    let mut current = crate::operator_state::tests::legacy_state();
    current = patch(&current, 1, 0.0);
    current = patch(&current, 3, 0.0);
    for (version, length) in [
        (1, 224),
        (2, 240),
        (3, 288),
        (4, 304),
        (5, 320),
        (6, 320),
        (7, 364),
        (8, 380),
        (11, 416),
        (12, 1488),
        (13, 1488),
        (14, 1616),
        (15, 1728),
        (16, 1744),
    ] {
        let old = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            version,
            current.payload()[..length].to_vec(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&old).unwrap();
        assert_eq!(migrated.state_version(), 17);
        assert_eq!(&migrated.payload()[..length], old.payload());
        let expected = if version < 3 {
            [100.0; 4]
        } else {
            [0.0, 100.0, 0.0, 100.0]
        };
        assert_eq!(
            OperatorSettings::from_state(&migrated)
                .unwrap()
                .pitch_keytrack
                .values(),
            expected
        );
        assert_eq!(
            OPERATOR_METADATA.migrate_state(&migrated).unwrap(),
            migrated
        );
    }
}

#[test]
fn version_sixteen_preserves_fine_global_and_modulation_values_and_pcm() {
    let mut current = OperatorSettings::defaults().to_state().unwrap();
    for (id, value) in [
        (
            crate::operator_state::parameter::fixed(2),
            ParameterValue::Bool(true),
        ),
        (
            crate::operator_state::parameter::fine_offset(1),
            ParameterValue::Float(-37.0),
        ),
        (ParameterId::from_static(605), ParameterValue::Float(17.0)),
        (ParameterId::from_static(620), ParameterValue::Choice(1)),
    ] {
        current = OPERATOR_METADATA
            .patch_parameter(&current, id, value)
            .unwrap();
    }
    let old = NativeDeviceState::new(
        OPERATOR_TYPE_ID,
        16,
        current.payload()[..FINE_PAYLOAD_BYTES].to_vec(),
    )
    .unwrap();
    let migrated = OPERATOR_METADATA.migrate_state(&old).unwrap();
    assert_eq!(migrated, current);
    let a = OperatorSettings::from_state(&old)
        .unwrap()
        .prepare_configuration(48000)
        .unwrap();
    let b = OperatorSettings::from_state(&migrated)
        .unwrap()
        .prepare_configuration(48000)
        .unwrap();
    let table = SineTable::new();
    for pitch in [0, 57, 69, 81, 127] {
        let na = a.prepare_note(pitch, 100).unwrap();
        let nb = b.prepare_note(pitch, 100).unwrap();
        assert_eq!(na, nb);
        for age in [0, 1, 97, 1024, 8192] {
            assert_eq!(
                a.sample(&table, &na, age, 6000).to_bits(),
                b.sample(&table, &nb, age, 6000).to_bits()
            );
        }
    }
}

#[test]
fn malformed_keytrack_suffix_and_inconsistent_fixed_flags_are_rejected() {
    let current = OperatorSettings::defaults().to_state().unwrap();
    for length in [FINE_PAYLOAD_BYTES, PAYLOAD_BYTES - 1, PAYLOAD_BYTES + 1] {
        let mut bytes = current.payload().to_vec();
        bytes.resize(length, 0);
        let bad = NativeDeviceState::new(OPERATOR_TYPE_ID, 17, bytes).unwrap();
        assert!(OperatorSettings::from_state(&bad).is_err());
    }
    for i in 0..4 {
        for bad in [f32::NAN, f32::NEG_INFINITY, -0.1, 100.1, 0.0] {
            let mut bytes = current.payload().to_vec();
            let at = FINE_PAYLOAD_BYTES + i * 4;
            bytes[at..at + 4].copy_from_slice(&bad.to_le_bytes());
            assert!(
                OperatorSettings::from_state(
                    &NativeDeviceState::new(OPERATOR_TYPE_ID, 17, bytes).unwrap()
                )
                .is_err()
            );
        }
    }
    assert!(
        OperatorSettings::from_state(
            &NativeDeviceState::new(OPERATOR_TYPE_ID, 18, current.payload().to_vec()).unwrap()
        )
        .is_err()
    );
}

#[test]
fn saved_half_tracking_changes_one_oscillator_pitch_without_changing_neighbors() {
    let mut original = OperatorSettings::defaults().to_state().unwrap();
    for field in [
        crate::operator_state::parameter::EnvelopeField::Initial,
        crate::operator_state::parameter::EnvelopeField::Peak,
        crate::operator_state::parameter::EnvelopeField::Sustain,
    ] {
        original = OPERATOR_METADATA
            .patch_parameter(
                &original,
                crate::operator_state::parameter::envelope(1, field),
                ParameterValue::Float(1.0),
            )
            .unwrap();
    }
    let changed = patch(&original, 1, 50.0);
    assert_eq!(
        &original.payload()[..FINE_PAYLOAD_BYTES],
        &changed.payload()[..FINE_PAYLOAD_BYTES]
    );
    let settings = OperatorSettings::from_state(&changed).unwrap();
    let config = settings.prepare_configuration(48000).unwrap();
    let table = SineTable::new();
    // Default anchor440Hz, ratio1: the played880Hz note becomes sqrt(440*880).
    let hz = (440.0_f64 * 880.0).sqrt();
    let step = (hz / 48000.0 * crate::PHASE_SCALE).round() as u64;
    let note = config.prepare_note(81, 127).unwrap();
    for age in 0..128 {
        let expected = table.sample_phase(step.wrapping_mul(age));
        assert!((config.sample(&table, &note, age, u64::MAX) - expected).abs() < 1e-7);
    }
}
