use super::*;
use hex_lattice::NativeInstrumentRegistry;
use hex_lattice_contract::MAX_NATIVE_DEVICE_STATE_BYTES;

fn explicit_registry() -> NativeInstrumentRegistry<'static> {
    static DEFINITIONS: [&dyn NativeInstrumentDefinition; 1] = [&FM4_DEFINITION];
    NativeInstrumentRegistry::new(&DEFINITIONS).expect("valid standalone FM4 definition")
}

#[test]
fn definition_defaults_roundtrip_and_match_every_stable_descriptor() {
    let registry = explicit_registry();
    let state = registry
        .default_state(FM4_TYPE_ID)
        .expect("default FM4 state");
    assert_eq!(state.type_id(), FM4_TYPE_ID);
    assert_eq!(state.state_version(), FM4_STATE_VERSION);
    assert_eq!(state.payload().len(), PAYLOAD_BYTES);
    assert_eq!(FM4_DEFINITION.migrate_state(&state).unwrap(), state);
    assert_eq!(PARAMETERS.len(), EDITABLE_PARAMETER_COUNT);
    assert_eq!(PARAMETERS.len(), 32);

    for descriptor in PARAMETERS {
        assert!(![13, 18, 23, 28].contains(&descriptor.id.value()));
        assert_eq!(
            FM4_DEFINITION
                .read_parameter(&state, descriptor.id)
                .expect("read default parameter"),
            descriptor.default,
            "descriptor {} matches encoded default",
            descriptor.label
        );
    }
    for reserved in [13, 18, 23, 28] {
        assert!(
            PARAMETERS
                .iter()
                .all(|descriptor| descriptor.id.value() != reserved)
        );
        assert_eq!(
            FM4_DEFINITION.read_parameter(&state, ParameterId::from_static(reserved)),
            Err(DeviceContractError::UnknownParameter)
        );
    }

    let mut authored = state.payload().to_vec();
    authored[8..12].copy_from_slice(&(-0.0_f32).to_le_bytes());
    let authored = NativeDeviceState::new(FM4_TYPE_ID, FM4_STATE_VERSION, authored).unwrap();
    assert_eq!(FM4_DEFINITION.migrate_state(&authored).unwrap(), authored);
}

#[test]
fn editable_descriptor_ranges_patch_and_roundtrip_both_endpoints() {
    let original = FM4_DEFINITION.default_state().unwrap();
    for descriptor in PARAMETERS {
        let hex_lattice_contract::ParameterKind::Float { min, max, .. } = descriptor.kind else {
            panic!("FM4 parameters are floats");
        };
        for endpoint in [min, max] {
            let value = ParameterValue::Float(endpoint);
            let patched = FM4_DEFINITION
                .patch_parameter(&original, descriptor.id, value)
                .unwrap_or_else(|error| panic!("endpoint patch {}: {error:?}", descriptor.label));
            assert_eq!(
                FM4_DEFINITION.read_parameter(&patched, descriptor.id),
                Ok(value)
            );
            assert_eq!(FM4_DEFINITION.migrate_state(&patched).unwrap(), patched);
        }
    }
    assert_eq!(original, FM4_DEFINITION.default_state().unwrap());
}

#[test]
fn generic_parameter_patches_read_back_and_preserve_the_original_state() {
    let state = FM4_DEFINITION.default_state().unwrap();
    for descriptor in PARAMETERS {
        let value = match descriptor.id.value() {
            1..=12 => {
                let field = (descriptor.id.value() - 1) % 3;
                ParameterValue::Float(if field == 2 { 0.25 } else { 1.25 })
            }
            13..=28 => {
                let index = descriptor.id.value() - 13;
                let source = index / 4;
                let target = index % 4;
                if source == target {
                    ParameterValue::Float(0.0)
                } else {
                    ParameterValue::Float(0.125)
                }
            }
            29..=32 => ParameterValue::Float(0.25),
            33..=36 => ParameterValue::Float(match descriptor.id.value() {
                35 => 0.5,
                _ => 0.25,
            }),
            _ => unreachable!("static descriptor id"),
        };
        let patched = FM4_DEFINITION
            .patch_parameter(&state, descriptor.id, value)
            .unwrap_or_else(|error| {
                panic!("valid independent patch {}: {error:?}", descriptor.label)
            });
        assert_eq!(
            FM4_DEFINITION.read_parameter(&patched, descriptor.id),
            Ok(value)
        );
        assert_eq!(
            state.payload(),
            FM4_DEFINITION.default_state().unwrap().payload()
        );
    }
}

#[test]
fn malformed_types_versions_lengths_and_oversize_are_refused() {
    let wrong_type = NativeDeviceState::new(
        crate::utility::UTILITY_TYPE_ID,
        FM4_STATE_VERSION,
        vec![0; PAYLOAD_BYTES],
    )
    .unwrap();
    assert_eq!(
        FM4_DEFINITION.migrate_state(&wrong_type),
        Err(DeviceContractError::WrongDeviceType)
    );

    let future =
        NativeDeviceState::new(FM4_TYPE_ID, FM4_STATE_VERSION + 1, vec![0; PAYLOAD_BYTES]).unwrap();
    assert_eq!(
        FM4_DEFINITION.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );

    for length in [PAYLOAD_BYTES - 1, PAYLOAD_BYTES + 1] {
        let malformed =
            NativeDeviceState::new(FM4_TYPE_ID, FM4_STATE_VERSION, vec![0; length]).unwrap();
        assert_eq!(
            FM4_DEFINITION.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }

    assert_eq!(
        NativeDeviceState::new(
            FM4_TYPE_ID,
            FM4_STATE_VERSION,
            vec![0; MAX_NATIVE_DEVICE_STATE_BYTES + 1]
        ),
        Err(DeviceContractError::StateTooLarge)
    );

    let mut non_finite = FM4_DEFINITION.default_state().unwrap().payload().to_vec();
    non_finite[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
    let malformed = NativeDeviceState::new(FM4_TYPE_ID, FM4_STATE_VERSION, non_finite).unwrap();
    assert_eq!(
        FM4_DEFINITION.migrate_state(&malformed),
        Err(DeviceContractError::InvalidParameterValue)
    );
}

#[test]
fn cross_field_cycles_and_invalid_patches_leave_prior_bytes_unchanged() {
    let original = FM4_DEFINITION.default_state().unwrap();
    assert_eq!(parameter::modulation(1, 1).value(), 0);
    assert_eq!(
        FM4_DEFINITION.patch_parameter(
            &original,
            ParameterId::from_static(13),
            ParameterValue::Float(0.1),
        ),
        Err(DeviceContractError::UnknownParameter)
    );
    let mut authored_self_edge = original.payload().to_vec();
    authored_self_edge[12 * 4..13 * 4].copy_from_slice(&0.1_f32.to_le_bytes());
    let self_edge_state =
        NativeDeviceState::new(FM4_TYPE_ID, FM4_STATE_VERSION, authored_self_edge).unwrap();
    assert_eq!(
        FM4_DEFINITION.migrate_state(&self_edge_state),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert_eq!(
        FM4_DEFINITION.read_parameter(&self_edge_state, ParameterId::from_static(13)),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert_eq!(original, FM4_DEFINITION.default_state().unwrap());

    let first_edge = FM4_DEFINITION
        .patch_parameter(
            &original,
            parameter::modulation(1, 2),
            ParameterValue::Float(0.2),
        )
        .unwrap();
    let cyclic = FM4_DEFINITION.patch_parameter(
        &first_edge,
        parameter::modulation(2, 1),
        ParameterValue::Float(0.2),
    );
    assert_eq!(cyclic, Err(DeviceContractError::InvalidParameterValue));
    assert_eq!(
        FM4_DEFINITION.read_parameter(&first_edge, parameter::modulation(1, 2)),
        Ok(ParameterValue::Float(0.2))
    );
    assert_eq!(
        FM4_DEFINITION.patch_parameter(
            &original,
            parameter::ATTACK,
            ParameterValue::Float(f32::INFINITY)
        ),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert_eq!(
        FM4_DEFINITION.patch_parameter(
            &original,
            parameter::ATTACK,
            ParameterValue::Float(f32::NEG_INFINITY)
        ),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert_eq!(
        FM4_DEFINITION.patch_parameter(&original, parameter::ATTACK, ParameterValue::Bool(true)),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert_eq!(
        FM4_DEFINITION.patch_parameter(
            &original,
            ParameterId::from_static(999),
            ParameterValue::Float(0.0)
        ),
        Err(DeviceContractError::UnknownParameter)
    );
}

#[test]
fn explicit_definition_prepares_analytic_notes_matching_the_standalone_kernel() {
    let registry = explicit_registry();
    let state = registry.default_state(FM4_TYPE_ID).unwrap();
    let settings = Fm4Settings::from_state(&state).unwrap();
    let (oscillators, routing, envelope) = settings.kernel_parts().unwrap();
    let table = crate::SineTable::new();

    for sample_rate in [8_000, 44_100, 96_000, 192_000] {
        let prepared = registry
            .prepare_instrument(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .expect("control-side FM4 preparation");
        for pitch in [48, 60, 72] {
            for velocity in [1, 64, 127] {
                let expected =
                    PreparedFm4::new(sample_rate, pitch, velocity, oscillators, routing, envelope)
                        .expect("valid note configuration");
                let note = prepared
                    .prepare_note(pitch, velocity)
                    .expect("prepared analytic sampler");
                assert_eq!(note.release_frames(), expected.release_frames());
                let gate = u64::from(sample_rate / 20);
                for age in [
                    0,
                    1,
                    u64::from(sample_rate / 100),
                    gate.saturating_sub(1),
                    gate,
                    gate + 1,
                    gate + note.release_frames() - 1,
                    gate + note.release_frames(),
                ] {
                    let sample = expected.sample(&table, age, gate);
                    assert_eq!(
                        note.sample(age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                        [sample, sample]
                    );
                }
            }
        }
    }
    assert!(crate::shared_sine_table_if_prepared().is_some());
}

#[test]
fn definition_is_admitted_by_builtin_and_explicit_registries() {
    let registry = explicit_registry();
    assert!(registry.definition(FM4_TYPE_ID).is_some());
    let builtins = crate::native_instrument_registry().unwrap();
    assert!(builtins.definition(FM4_TYPE_ID).is_some());
    assert!(
        builtins
            .definition(crate::operator_state::OPERATOR_TYPE_ID)
            .is_some()
    );
    assert_eq!(builtins.definitions().len(), 4);
}
