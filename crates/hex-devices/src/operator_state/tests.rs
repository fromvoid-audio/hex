use super::*;
use crate::SineTable;
use parameter::EnvelopeField;

fn patch(state: &NativeDeviceState, id: ParameterId, value: f32) -> NativeDeviceState {
    OPERATOR_METADATA
        .patch_parameter(state, id, ParameterValue::Float(value))
        .expect("valid patch")
}

#[test]
fn opaque_state_and_all_editable_parameters_round_trip_exactly() {
    let original = OPERATOR_METADATA.default_state().expect("defaults");
    assert_eq!(original.payload().len(), 224);
    assert_eq!(OPERATOR_METADATA.descriptors().len(), 52);
    assert_eq!(
        OPERATOR_METADATA
            .migrate_state(&original)
            .expect("migration"),
        original
    );
    let mut ids = std::collections::BTreeSet::new();
    for descriptor in OPERATOR_METADATA.descriptors() {
        assert!(ids.insert(descriptor.id.value()));
        assert_eq!(descriptor.label.to_lowercase(), descriptor.label);
        descriptor
            .validate(descriptor.default)
            .expect("valid default");
        let ParameterKind::Float { min, max, .. } = descriptor.kind else {
            panic!("float parameter")
        };
        let value = min + (max - min) * 0.375;
        let changed = patch(&original, descriptor.id, value);
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&changed, descriptor.id)
                .expect("read"),
            ParameterValue::Float(value)
        );
        assert_eq!(
            OperatorSettings::from_state(&changed)
                .expect("decode")
                .to_state()
                .expect("encode"),
            changed
        );
        for invalid in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            min - 1.0,
            max + 1.0,
        ] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&original, descriptor.id, ParameterValue::Float(invalid))
                    .is_err()
            );
        }
        for invalid in [ParameterValue::Bool(true), ParameterValue::Choice(1)] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&original, descriptor.id, invalid)
                    .is_err()
            );
        }
    }
    assert_eq!(
        OperatorSettings::defaults().to_state().expect("defaults"),
        original
    );
}

#[test]
fn malformed_state_reserved_feedback_and_cycles_are_refused_atomically() {
    let original = OPERATOR_METADATA.default_state().expect("defaults");
    for (type_id, version, payload) in [
        (
            crate::fm4::device::FM4_TYPE_ID,
            1,
            original.payload().to_vec(),
        ),
        (OPERATOR_TYPE_ID, 2, original.payload().to_vec()),
        (OPERATOR_TYPE_ID, 1, original.payload()[..220].to_vec()),
        (OPERATOR_TYPE_ID, 1, vec![0; 228]),
    ] {
        let state = NativeDeviceState::new(type_id, version, payload).expect("opaque state");
        assert!(OperatorSettings::from_state(&state).is_err());
        assert!(OPERATOR_METADATA.migrate_state(&state).is_err());
    }
    for operator in 0..4 {
        for value in [1.0_f32, f32::NAN, f32::INFINITY] {
            let mut bytes = original.payload().to_vec();
            let offset = (12 + operator * 4 + operator) * 4;
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                OperatorSettings::from_state(
                    &NativeDeviceState::new(OPERATOR_TYPE_ID, 1, bytes).expect("state")
                )
                .is_err()
            );
        }
    }
    let one_edge = patch(&original, parameter::modulation(1, 2), 0.5);
    assert!(
        OPERATOR_METADATA
            .patch_parameter(
                &one_edge,
                parameter::modulation(2, 1),
                ParameterValue::Float(0.5)
            )
            .is_err()
    );
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&one_edge, parameter::modulation(2, 1))
            .expect("unchanged"),
        ParameterValue::Float(0.0)
    );
    for id in [
        ParameterId::from_static(0),
        ParameterId::from_static(13),
        ParameterId::from_static(57),
        parameter::envelope(5, EnvelopeField::Attack),
    ] {
        assert!(OPERATOR_METADATA.read_parameter(&original, id).is_err());
        assert!(
            OPERATOR_METADATA
                .patch_parameter(&original, id, ParameterValue::Float(0.0))
                .is_err()
        );
    }
    let negative_zero = patch(
        &original,
        parameter::envelope(1, EnvelopeField::Initial),
        -0.0,
    );
    assert_eq!(
        OperatorSettings::from_state(&negative_zero)
            .expect("decode")
            .to_state()
            .expect("encode")
            .payload(),
        negative_zero.payload()
    );
}

#[test]
fn control_fields_drive_each_operator_against_an_independent_pcm_oracle() {
    let table = SineTable::new();
    for operator in 1..=4 {
        let mut state = OPERATOR_METADATA.default_state().expect("defaults");
        for carrier in 1..=4 {
            state = patch(
                &state,
                parameter::carrier(carrier),
                if carrier == operator { 1.0 } else { 0.0 },
            );
        }
        for (id, value) in [
            (parameter::oscillator_ratio(operator), 2.0),
            (parameter::oscillator_level(operator), 0.5),
            (parameter::oscillator_phase(operator), 0.125),
            (parameter::envelope(operator, EnvelopeField::Initial), 0.2),
            (parameter::envelope(operator, EnvelopeField::Peak), 0.9),
            (parameter::envelope(operator, EnvelopeField::Sustain), 0.3),
            (parameter::envelope(operator, EnvelopeField::Attack), 0.004),
            (parameter::envelope(operator, EnvelopeField::Decay), 0.008),
            (parameter::envelope(operator, EnvelopeField::Release), 0.006),
        ] {
            state = patch(&state, id, value);
        }
        let note = OperatorSettings::from_state(&state)
            .expect("decode")
            .prepare_note(8000, 60, 100)
            .expect("prepare");
        let held = |age: u64| {
            let initial = f64::from(0.2_f32);
            let peak = f64::from(0.9_f32);
            let sustain = f64::from(0.3_f32);
            if age < 32 {
                initial + (peak - initial) * age as f64 / 32.0
            } else if age < 96 {
                sustain + (peak - sustain) * 1e-5_f64.powf((age - 32) as f64 / 64.0)
            } else {
                sustain
            }
        };
        for gate in [16, 64, 120] {
            for age in 0..300 {
                let amplitude = if age < gate {
                    held(age)
                } else if age - gate < 48 {
                    held(gate) * 1e-5_f64.powf((age - gate) as f64 / 48.0)
                } else {
                    0.0
                };
                let turns = 440.0 * 2.0_f64.powf(-9.0 / 12.0) * 2.0 * age as f64 / 8000.0 + 0.125;
                let expected =
                    (turns * std::f64::consts::TAU).sin() * 0.5 * amplitude * 100.0 / 127.0;
                let actual = f64::from(note.sample(&table, age, gate));
                assert!(
                    (actual - expected).abs() < 2e-6,
                    "operator {operator}, age {age}, gate {gate}: {actual} != {expected}"
                );
            }
        }
    }
}

#[test]
fn routing_field_direction_and_carrier_sign_match_independent_modulation() {
    let table = SineTable::new();
    // Reverse operator order catches an accidental target-major routing codec.
    for (source, target) in [(1, 4), (4, 1), (2, 3), (3, 2)] {
        let mut state = OPERATOR_METADATA.default_state().expect("defaults");
        for operator in 1..=4 {
            state = patch(
                &state,
                parameter::carrier(operator),
                if operator == target { -0.75 } else { 0.0 },
            );
            for (field, value) in [
                (EnvelopeField::Attack, 0.0),
                (EnvelopeField::Decay, 0.0),
                (
                    EnvelopeField::Sustain,
                    if operator == source { 0.6 } else { 0.3 },
                ),
            ] {
                state = patch(&state, parameter::envelope(operator, field), value);
            }
        }
        state = patch(&state, parameter::oscillator_ratio(source), 1.5);
        state = patch(&state, parameter::oscillator_ratio(target), 0.5);
        state = patch(&state, parameter::oscillator_level(source), 0.4);
        state = patch(&state, parameter::oscillator_level(target), 0.8);
        state = patch(&state, parameter::modulation(source, target), -0.25);
        let note = OperatorSettings::from_state(&state)
            .expect("decode")
            .prepare_note(8000, 69, 127)
            .expect("prepare");
        for age in 0..1000 {
            let modulator = (std::f64::consts::TAU * 440.0 * 1.5 * age as f64 / 8000.0).sin()
                * f64::from(0.4_f32)
                * f64::from(0.6_f32);
            let turns = 440.0 * 0.5 * age as f64 / 8000.0 - 0.25 * modulator;
            let expected = (std::f64::consts::TAU * turns).sin()
                * f64::from(0.8_f32)
                * f64::from(0.3_f32)
                * -0.75;
            assert!(
                (f64::from(note.sample(&table, age, 1000)) - expected).abs() < 2e-6,
                "route {source}->{target}, age {age}"
            );
        }
    }
}

#[test]
fn standalone_preparation_rejects_invalid_note_inputs() {
    let settings = OperatorSettings::defaults();
    for (rate, pitch, velocity) in [
        (0, 60, 100),
        (7999, 60, 100),
        (192001, 60, 100),
        (48000, 128, 100),
        (48000, 60, 0),
        (48000, 60, 128),
    ] {
        assert!(settings.prepare_note(rate, pitch, velocity).is_err());
    }
}
