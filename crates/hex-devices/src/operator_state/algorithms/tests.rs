use super::*;
use crate::operator_state::{OPERATOR_METADATA, OperatorSettings, parameter};
use hex_lattice_contract::NativeDeviceMetadata;

#[test]
fn every_algorithm_is_distinct_acyclic_and_round_trips_without_a_payload_suffix() {
    let original = OPERATOR_METADATA.default_state().unwrap();
    let mut seen = Vec::new();
    for (index, algorithm) in CATALOG.iter().enumerate() {
        assert!(
            algorithm
                .edges
                .iter()
                .all(|&(source, target)| source < 4 && source > target)
        );
        assert!(!algorithm.carriers.is_empty());
        for source in 0..4 {
            assert!(
                algorithm.carriers.contains(&source)
                    || algorithm.edges.iter().any(|&(s, _)| s == source)
            );
            assert_eq!(
                algorithm.carriers.contains(&source),
                !algorithm.edges.iter().any(|&(s, _)| s == source)
            );
        }
        let fields = algorithm.routing_fields();
        assert!(!seen.contains(&fields));
        seen.push(fields);
        let changed = OPERATOR_METADATA
            .patch_parameter(
                &original,
                parameter::ALGORITHM,
                ParameterValue::Choice(index as u32 + 1),
            )
            .unwrap();
        assert_eq!(changed.state_version(), original.state_version());
        assert_eq!(changed.payload().len(), original.payload().len());
        assert_eq!(&changed.payload()[..48], &original.payload()[..48]);
        assert_eq!(&changed.payload()[128..], &original.payload()[128..]);
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&changed, parameter::ALGORITHM)
                .unwrap(),
            ParameterValue::Choice(index as u32 + 1)
        );
        assert_eq!(
            OperatorSettings::from_state(&changed)
                .unwrap()
                .to_state()
                .unwrap(),
            changed
        );
    }
}

#[test]
fn custom_legacy_routing_is_preserved_and_old_preset_choices_keep_their_meaning() {
    let original = OPERATOR_METADATA.default_state().unwrap();
    let custom = OPERATOR_METADATA
        .patch_parameter(
            &original,
            parameter::modulation(4, 1),
            ParameterValue::Float(-0.75),
        )
        .unwrap();
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&custom, parameter::ALGORITHM)
            .unwrap(),
        ParameterValue::Choice(0)
    );
    assert_eq!(
        OPERATOR_METADATA
            .patch_parameter(&custom, parameter::ALGORITHM, ParameterValue::Choice(0))
            .unwrap(),
        custom
    );
    for (old_choice, new_choice) in [(1, 1), (2, 11), (3, 8)] {
        let old = OPERATOR_METADATA
            .patch_parameter(
                &custom,
                parameter::routing_preset(),
                ParameterValue::Choice(old_choice),
            )
            .unwrap();
        let new = OPERATOR_METADATA
            .patch_parameter(
                &custom,
                parameter::ALGORITHM,
                ParameterValue::Choice(new_choice),
            )
            .unwrap();
        let old_settings = OperatorSettings::from_state(&old).unwrap();
        let new_settings = OperatorSettings::from_state(&new).unwrap();
        assert_eq!(old_settings.fields[12..28], new_settings.fields[12..28]);
        let expected_carriers = match old_choice {
            2 => [0.25; 4],
            3 => [0.5, 0.0, 0.5, 0.0],
            _ => [1.0, 0.0, 0.0, 0.0],
        };
        assert_eq!(old_settings.fields[28..32], expected_carriers);
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&old, parameter::routing_preset())
                .unwrap(),
            ParameterValue::Choice(old_choice)
        );
    }
    for invalid in [
        ParameterValue::Choice(12),
        ParameterValue::Float(1.0),
        ParameterValue::Bool(true),
    ] {
        assert!(
            OPERATOR_METADATA
                .patch_parameter(&custom, parameter::ALGORITHM, invalid)
                .is_err()
        );
    }
}

#[test]
fn serial_default_levels_make_b_the_modulation_amount_without_a_matrix_edit() {
    let original = OPERATOR_METADATA.default_state().unwrap();
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&original, parameter::ALGORITHM)
            .unwrap(),
        ParameterValue::Choice(1)
    );
    for operator in 2..=4 {
        assert_eq!(
            OPERATOR_METADATA
                .read_parameter(&original, parameter::oscillator_level(operator))
                .unwrap(),
            ParameterValue::Float(0.0)
        );
    }
    let render = |state: &hex_lattice_contract::NativeDeviceState| {
        let note = OperatorSettings::from_state(state)
            .unwrap()
            .prepare_note(48_000, 69, 100)
            .unwrap();
        (0..512)
            .map(|age| note.sample(crate::shared_sine_table(), age, 4096))
            .collect::<Vec<_>>()
    };
    let dry = render(&original);
    for choice in 1..=11 {
        let selected = OPERATOR_METADATA
            .patch_parameter(
                &original,
                parameter::ALGORITHM,
                ParameterValue::Choice(choice),
            )
            .unwrap();
        assert_eq!(
            dry,
            render(&selected),
            "a-only gain stays constant across algorithm {choice}"
        );
    }
    assert!(dry.iter().any(|sample| sample.abs() > 0.01));
    let disconnected = OPERATOR_METADATA
        .patch_parameter(
            &original,
            parameter::modulation(2, 1),
            ParameterValue::Float(0.0),
        )
        .unwrap();
    assert_eq!(dry, render(&disconnected), "zero-level b contributes no FM");
    let raised_b = OPERATOR_METADATA
        .patch_parameter(
            &original,
            parameter::oscillator_level(2),
            ParameterValue::Float(0.75),
        )
        .unwrap();
    assert_ne!(dry, render(&raised_b), "raising b modulates audible a");
    let muted_a = OPERATOR_METADATA
        .patch_parameter(
            &raised_b,
            parameter::oscillator_level(1),
            ParameterValue::Float(0.0),
        )
        .unwrap();
    assert!(
        render(&muted_a).iter().all(|sample| *sample == 0.0),
        "b remains a modulator, not an added carrier"
    );
}
