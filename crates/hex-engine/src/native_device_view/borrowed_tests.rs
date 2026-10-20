use super::*;

#[test]
fn borrowed_controls_match_current_views_without_owner_birth() {
    for class in [NativeDeviceClass::Effect, NativeDeviceClass::Instrument] {
        let catalog = native_device_catalog_for_class(class).unwrap();
        for entry in catalog {
            let state = crate::default_native_device_state_for_class(class, entry.type_id).unwrap();
            let expected = native_device_view_for_class(class, &state).unwrap();
            let measured = allocation_counter::measure(|| {
                let read = NativeDeviceRead::on_control(class, &state).unwrap();
                assert_eq!(read.name(), expected.name);
                assert_eq!(read.parameter_count(), expected.parameters.len());
                for (index, expected) in expected.parameters.iter().enumerate() {
                    let actual = read.parameter_at(index).unwrap();
                    assert_eq!(actual.descriptor, expected.descriptor);
                    assert_eq!(actual.semantics, expected.semantics);
                    assert_eq!(actual.value, expected.value);
                    assert_eq!(actual.enabled, expected.enabled);
                    assert_eq!(
                        read.parameter(actual.descriptor.id).unwrap().value,
                        actual.value
                    );
                }
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.count_current,
                    measured.bytes_total
                ),
                (0, 0, 0),
                "{}",
                entry.name
            );
            let wrong = match class {
                NativeDeviceClass::Effect => NativeDeviceClass::Instrument,
                NativeDeviceClass::Instrument => NativeDeviceClass::Effect,
            };
            assert!(NativeDeviceRead::on_control(wrong, &state).is_err());
            let malformed =
                NativeDeviceState::new(entry.type_id, state.state_version(), vec![255]).unwrap();
            let future =
                NativeDeviceState::new(entry.type_id, u32::MAX, state.payload().to_vec()).unwrap();
            assert!(NativeDeviceRead::on_control(class, &malformed).is_err());
            assert!(NativeDeviceRead::on_control(class, &future).is_err());
        }
    }
}

#[test]
fn borrowed_controls_preserve_dynamic_eq_and_conditional_utility() {
    use hex_devices::eq::{EqState, Node, NodeId};
    let mut authored = EqState::default();
    for index in 1..=4 {
        authored
            .add(Node::new(NodeId::new(index).unwrap(), index as f32 * 100.0, 3.0, 0).unwrap())
            .unwrap();
    }
    let state = authored.encode().unwrap();
    let expected = native_device_view_for_class(NativeDeviceClass::Effect, &state).unwrap();
    let measured = allocation_counter::measure(|| {
        let read = NativeDeviceRead::on_control(NativeDeviceClass::Effect, &state).unwrap();
        assert_eq!(read.parameter_count(), expected.parameters.len());
        for (index, expected) in expected.parameters.iter().enumerate() {
            let actual = read.parameter_at(index).unwrap();
            assert_eq!(
                (
                    actual.descriptor,
                    actual.semantics,
                    actual.value,
                    actual.enabled
                ),
                (
                    expected.descriptor,
                    expected.semantics,
                    expected.value,
                    expected.enabled
                )
            );
        }
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    let utility =
        crate::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID).unwrap();
    let channel = native_device_view(&utility)
        .unwrap()
        .parameters
        .into_iter()
        .find(|p| p.descriptor.label == "channel")
        .unwrap();
    let left = crate::patch_native_device_parameter(
        &utility,
        channel.descriptor.id,
        ParameterValue::Choice(1),
    )
    .unwrap();
    let read = NativeDeviceRead::on_control(NativeDeviceClass::Effect, &left).unwrap();
    for index in 0..read.parameter_count() {
        let parameter = read.parameter_at(index).unwrap();
        if matches!(
            parameter.descriptor.label,
            "width" | "mid-side" | "mid-side balance"
        ) {
            assert!(!parameter.enabled);
        }
    }
}

#[test]
fn borrowed_pending_curves_share_actual_patch_law_without_allocating() {
    let saturator =
        crate::default_native_device_state(hex_devices::saturator::SATURATOR_TYPE_ID).unwrap();
    let compressor =
        crate::default_native_device_state(hex_devices::compressor::COMPRESSOR_TYPE_ID).unwrap();
    for (state, overrides) in [
        (
            &saturator,
            [
                (1, ParameterValue::Float(12.0)),
                (2, ParameterValue::Float(-3.0)),
                (3, ParameterValue::Float(0.75)),
                (4, ParameterValue::Choice(1)),
            ],
        ),
        (
            &compressor,
            [
                (1, ParameterValue::Float(-24.0)),
                (2, ParameterValue::Float(4.0)),
                (5, ParameterValue::Float(6.0)),
                (6, ParameterValue::Float(3.0)),
            ],
        ),
    ] {
        let overrides = overrides.map(|(id, value)| (ParameterId::new(id).unwrap(), value));
        let mut expected_state = state.clone();
        for (id, value) in overrides {
            expected_state =
                crate::patch_native_device_parameter(&expected_state, id, value).unwrap();
        }
        let expected_s = if state.type_id() == saturator.type_id() {
            Some(crate::native_saturator_transfer_curve(&expected_state).unwrap())
        } else {
            None
        };
        let expected_c = if state.type_id() == compressor.type_id() {
            Some(crate::native_compressor_transfer_curve(&expected_state).unwrap())
        } else {
            None
        };
        let measured = allocation_counter::measure(|| {
            let read = NativeDeviceRead::on_control(NativeDeviceClass::Effect, state).unwrap();
            if let Some(expected) = &expected_s {
                assert_eq!(
                    &read.saturator_transfer_curve(&overrides).unwrap(),
                    expected
                );
            }
            if let Some(expected) = &expected_c {
                assert_eq!(
                    &read.compressor_transfer_curve(&overrides).unwrap(),
                    expected
                );
            }
            assert!(read.saturator_transfer_curve(&[overrides[0]; 2]).is_err());
            assert!(read.compressor_transfer_curve(&[overrides[0]; 5]).is_err());
        });
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        assert_eq!(
            state.payload(),
            if expected_s.is_some() {
                saturator.payload()
            } else {
                compressor.payload()
            }
        );
    }
}

#[test]
fn borrowed_eq_response_matches_prepared_coefficients_with_effective_pending_edits() {
    use hex_devices::eq::{EqState, NodeId, NodeParameter};
    use hex_devices::eq8::parameter as legacy;
    let eq8 = crate::default_native_device_state(hex_devices::eq8::EQ8_TYPE_ID).unwrap();
    // Imported shelves retain the historical S law until an actual shape/Q edit.
    let shelf = crate::patch_native_device_parameter(
        &eq8,
        legacy::enabled(3, 0),
        ParameterValue::Bool(true),
    )
    .unwrap();
    let shelf =
        crate::patch_native_device_parameter(&shelf, legacy::kind(3, 0), ParameterValue::Choice(1))
            .unwrap();
    let flexible = EqState::upgrade_legacy(&shelf, 1)
        .unwrap()
        .encode()
        .unwrap();
    let first_mid = NodeId::new(25).unwrap();
    let cases = [
        (
            shelf,
            [
                (legacy::MODE, ParameterValue::Choice(2)),
                (legacy::gain(3, 0), ParameterValue::Float(9.0)),
                (legacy::frequency(3, 0), ParameterValue::Float(500.0)),
                (legacy::OUTPUT_GAIN, ParameterValue::Float(0.75)),
            ],
        ),
        (
            flexible,
            [
                (hex_devices::eq::MODE, ParameterValue::Choice(2)),
                (
                    first_mid.parameter(NodeParameter::Gain).unwrap(),
                    ParameterValue::Float(9.0),
                ),
                (
                    first_mid.parameter(NodeParameter::Frequency).unwrap(),
                    ParameterValue::Float(500.0),
                ),
                (hex_devices::eq::OUTPUT_GAIN, ParameterValue::Float(0.75)),
            ],
        ),
    ];
    for (state, overrides) in cases {
        let mut effective = state.clone();
        for (id, value) in overrides {
            effective = crate::patch_native_device_parameter(&effective, id, value).unwrap();
        }
        for rate in [8_000, 44_100, 96_000] {
            let frequencies = [20.0, 500.0, 1000.0, rate as f32 * 0.45];
            let original = native_effect_frequency_response(&state, rate, &frequencies)
                .unwrap()
                .unwrap();
            let expected = native_effect_frequency_response(&effective, rate, &frequencies)
                .unwrap()
                .unwrap();
            let measured = allocation_counter::measure(|| {
                let read = NativeDeviceRead::on_control(NativeDeviceClass::Effect, &state).unwrap();
                for (pending, expected) in [(&[][..], &original), (&overrides[..], &expected)] {
                    let response = read.eq_response(rate, pending).unwrap();
                    for (frequency, expected) in frequencies.into_iter().zip(expected) {
                        assert_eq!(response.at_hz(frequency).unwrap(), *expected);
                    }
                    for invalid in [0.0, -1.0, f32::NAN, rate as f32 * 0.5] {
                        assert!(response.at_hz(invalid).is_none());
                    }
                }
                assert!(read.eq_response(0, &[]).is_err());
                assert!(read.eq_response(rate, &[overrides[0]; 2]).is_err());
                assert!(
                    read.eq_response(
                        rate,
                        &[(ParameterId::from_static(999), ParameterValue::Float(1.0))]
                    )
                    .is_err()
                );
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.count_current,
                    measured.bytes_total
                ),
                (0, 0, 0)
            );
        }
    }
}

#[test]
fn borrowed_current_main_compressor_preserves_mix_and_detector() {
    let initial =
        crate::default_native_device_state(hex_devices::compressor::COMPRESSOR_TYPE_ID).unwrap();
    let state = crate::patch_native_device_parameter(
        &initial,
        ParameterId::new(8).unwrap(),
        ParameterValue::Choice(1),
    )
    .unwrap();
    assert_eq!(
        NativeDeviceRead::on_control(NativeDeviceClass::Effect, &state)
            .unwrap()
            .parameter(ParameterId::new(8).unwrap())
            .unwrap()
            .value,
        ParameterValue::Choice(1)
    );
    let overrides = [(1, -24.0), (2, 4.0), (5, 6.0), (6, 3.0), (7, 0.25)]
        .map(|(id, value)| (ParameterId::new(id).unwrap(), ParameterValue::Float(value)));
    let mut expected_state = state.clone();
    for (id, value) in overrides {
        expected_state = crate::patch_native_device_parameter(&expected_state, id, value).unwrap();
    }
    assert_eq!(
        NativeDeviceRead::on_control(NativeDeviceClass::Effect, &expected_state)
            .unwrap()
            .parameter(ParameterId::new(7).unwrap())
            .unwrap()
            .value,
        ParameterValue::Float(0.25)
    );
    let expected = crate::native_compressor_transfer_curve(&expected_state).unwrap();
    let read = NativeDeviceRead::on_control(NativeDeviceClass::Effect, &state).unwrap();
    assert_ne!(
        read.compressor_transfer_curve(&overrides[..4]).unwrap(),
        expected
    );
    let measured = allocation_counter::measure(|| {
        assert_eq!(
            read.compressor_transfer_curve(&overrides).unwrap(),
            expected
        );
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let mut duplicate = overrides;
    duplicate[4] = overrides[0];
    assert!(read.compressor_transfer_curve(&duplicate).is_err());
    let mut foreign = overrides;
    foreign[4].0 = ParameterId::new(99).unwrap();
    assert!(read.compressor_transfer_curve(&foreign).is_err());
}
