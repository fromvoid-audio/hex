//! Control-side payload sharing; no runtime/context-owner interning.
use super::*;
use hex_lattice::AnalyticContinuationId;

fn context(state: &NativeDeviceState) -> Box<dyn PreparedSharedAnalyticContext> {
    OPERATOR_METADATA
        .prepare_instrument(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .prepare_shared_analytic_context()
        .unwrap()
        .unwrap()
}

fn operator(context: &dyn PreparedSharedAnalyticContext) -> &PreparedOperatorContext {
    context
        .analytic_continuation()
        .unwrap()
        .as_any()
        .downcast_ref()
        .unwrap()
}

fn exact_pcm_and_provider(
    actual: &dyn PreparedSharedAnalyticContext,
    expected: &dyn PreparedSharedAnalyticContext,
) {
    let id = AnalyticContinuationId::new(123).unwrap();
    let a = actual.analytic_continuation().unwrap();
    let b = expected.analytic_continuation().unwrap();
    assert_eq!(a.format(), b.format());
    assert_eq!(a.endpoint(id), b.endpoint(id));
    let start = a.prepare_start(id, b, id, &b.endpoint(id)).unwrap();
    assert_eq!(start, a.endpoint(id));
    let mut nonzero = false;
    for (pitch, velocity) in [(0, 1), (60, 100), (127, 127)] {
        let (note, oracle) = match (
            actual.prepare_note(pitch, velocity),
            expected.prepare_note(pitch, velocity),
        ) {
            (Ok(note), Ok(oracle)) => (note, oracle),
            (Err(observed), Err(wanted)) => {
                assert_eq!(observed, wanted);
                continue;
            }
            _ => panic!("note admission changed"),
        };
        a.validate_note(&note).unwrap();
        for age in [0, 1, 517, 800, 24_001] {
            for gate in [u64::MAX, 600] {
                let observed =
                    actual.sample(&note, age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                nonzero |= observed[0] != 0.0;
                let wanted =
                    expected.sample(&oracle, age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                assert_eq!(observed.map(f32::to_bits), wanted.map(f32::to_bits));
                assert_eq!(
                    a.sample_with_parameters(
                        id,
                        &start,
                        &note,
                        age,
                        gate,
                        &hex_lattice::EMPTY_DEVICE_RESOURCES
                    )
                    .unwrap()
                    .map(f32::to_bits),
                    observed.map(f32::to_bits)
                );
            }
        }
    }
    assert!(nonzero, "PCM comparison must include sounding output");
}

#[test]
fn canonical_v12_reuses_the_actual_input_payload_owner_and_keeps_it_alive() {
    let input = tests::legacy_state();
    let original = input.payload().as_ptr();
    let bytes = input.payload().to_vec();
    let prepared = context(&input);
    let authored = &operator(prepared.as_ref()).authored_state;
    assert_eq!(authored.state_version(), OPERATOR_STATE_VERSION);
    assert_eq!(authored.payload().len(), PAYLOAD_BYTES); // Arc slice, no retained Vec slack.
    assert!(std::ptr::eq(authored.payload(), input.payload()));
    assert_eq!(prepared.declared_context_bytes(), 3056); // Includes host gains and asset-list metadata.
    drop(input);
    assert_eq!(
        operator(prepared.as_ref())
            .authored_state
            .payload()
            .as_ptr(),
        original
    );
    assert_eq!(operator(prepared.as_ref()).authored_state.payload(), bytes);
    let note = prepared.prepare_note(60, 100).unwrap();
    assert!(
        prepared.sample(&note, 517, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES)[0].is_finite()
    );
}

#[test]
fn exact_bytes_retain_signed_zero_disabled_oscillator_and_inactive_lfo_history() {
    let mut input = tests::legacy_state();
    for (parameter, value) in [
        (
            parameter::amplitude_velocity(1),
            ParameterValue::Float(-0.0),
        ),
        (parameter::enabled(3), ParameterValue::Bool(false)),
        (parameter::LFO_RATE, ParameterValue::Float(4.25)),
        (parameter::LFO_AMOUNT, ParameterValue::Float(23.0)),
        (parameter::LFO_RETRIGGER, ParameterValue::Bool(false)),
    ] {
        input = OPERATOR_METADATA
            .patch_parameter(&input, parameter, value)
            .unwrap();
    }
    let prepared = context(&input); // LFO remains disabled, authored values survive.
    let authored = &operator(prepared.as_ref()).authored_state;
    assert!(std::ptr::eq(authored.payload(), input.payload()));
    assert_eq!(authored.payload(), input.payload());
    assert_eq!(&authored.payload()[288..292], &(-0.0f32).to_le_bytes());
    assert_eq!(
        OPERATOR_METADATA.read_parameter(authored, parameter::enabled(3)),
        Ok(ParameterValue::Bool(false))
    );
    assert_eq!(
        OPERATOR_METADATA.read_parameter(authored, parameter::LFO_RATE),
        Ok(ParameterValue::Float(4.25))
    );
    let independently_encoded = OperatorSettings::from_state(&input)
        .unwrap()
        .to_state()
        .unwrap();
    assert!(!std::ptr::eq(
        independently_encoded.payload(),
        input.payload()
    ));
    exact_pcm_and_provider(prepared.as_ref(), context(&independently_encoded).as_ref());
}

#[test]
fn equal_content_on_unrelated_input_owners_does_not_intern_contexts_or_payloads() {
    let input = tests::legacy_state();
    let other = NativeDeviceState::new(
        OPERATOR_TYPE_ID,
        OPERATOR_STATE_VERSION,
        input.payload().to_vec(),
    )
    .unwrap();
    assert_eq!(input, other);
    assert!(!std::ptr::eq(input.payload(), other.payload()));
    let first = context(&input);
    let second = context(&other);
    assert!(std::ptr::eq(
        operator(first.as_ref()).authored_state.payload(),
        input.payload()
    ));
    assert!(std::ptr::eq(
        operator(second.as_ref()).authored_state.payload(),
        other.payload()
    ));
    assert_ne!(
        operator(first.as_ref()).configuration.configuration_id(),
        operator(second.as_ref()).configuration.configuration_id()
    );
    exact_pcm_and_provider(first.as_ref(), second.as_ref());
}

#[test]
fn every_legacy_version_keeps_required_canonical_allocation_and_exact_pcm() {
    let current = tests::legacy_state();
    for (version, len) in [
        (1, 224),
        (2, 240),
        (3, 288),
        (4, 304),
        (5, 320),
        (6, 320),
        (7, 364),
        (8, 380),
        (11, 416),
    ] {
        let input =
            NativeDeviceState::new(OPERATOR_TYPE_ID, version, current.payload()[..len].to_vec())
                .unwrap();
        let canonical = OperatorSettings::from_state(&input)
            .unwrap()
            .to_state()
            .unwrap();
        let prepared = context(&input);
        let authored = &operator(prepared.as_ref()).authored_state;
        assert_eq!(authored, &canonical);
        assert_eq!(authored.state_version(), OPERATOR_STATE_VERSION);
        assert_eq!(authored.payload().len(), PAYLOAD_BYTES);
        assert!(!std::ptr::eq(authored.payload(), input.payload()));
        exact_pcm_and_provider(prepared.as_ref(), context(&canonical).as_ref());
    }
}

#[test]
fn legacy_duration_normalization_keeps_new_bytes_and_input_unchanged() {
    let current = tests::legacy_state();
    for (version, len) in [(1, 224), (2, 240), (3, 288), (4, 304), (5, 320)] {
        let mut bytes = current.payload()[..len].to_vec();
        bytes[35 * 4..36 * 4].copy_from_slice(&120.0f32.to_le_bytes());
        let input = NativeDeviceState::new(OPERATOR_TYPE_ID, version, bytes.clone()).unwrap();
        let canonical = OperatorSettings::from_state(&input)
            .unwrap()
            .to_state()
            .unwrap();
        assert_ne!(&canonical.payload()[35 * 4..36 * 4], &bytes[35 * 4..36 * 4]);
        let prepared = context(&input);
        let authored = &operator(prepared.as_ref()).authored_state;
        assert_eq!(authored, &canonical);
        assert!(!std::ptr::eq(authored.payload(), input.payload()));
        assert_eq!(input.payload(), bytes);
        exact_pcm_and_provider(prepared.as_ref(), context(&canonical).as_ref());
    }
}

#[test]
fn malformed_versions_lengths_flags_and_values_keep_decoder_refusals() {
    let current = tests::legacy_state();
    let mut cases = Vec::new();
    for (version, len) in [
        (8, 379),
        (8, 381),
        (7, 380),
        (6, 364),
        (9, 380),
        (u32::MAX, 380),
    ] {
        let mut bytes = current.payload().to_vec();
        bytes.resize(len, 0);
        cases.push(NativeDeviceState::new(OPERATOR_TYPE_ID, version, bytes).unwrap());
    }
    for (offset, word) in [
        (364, 2u32),
        (0, f32::NAN.to_bits()),
        (35 * 4, 21.0f32.to_bits()),
    ] {
        let mut bytes = current.payload().to_vec();
        bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        cases.push(NativeDeviceState::new(OPERATOR_TYPE_ID, 8, bytes).unwrap());
    }
    cases.push(
        NativeDeviceState::new(
            crate::SINE_INSTRUMENT_TYPE_ID,
            8,
            current.payload().to_vec(),
        )
        .unwrap(),
    );
    for input in cases {
        let error = OperatorSettings::from_state(&input).unwrap_err();
        assert_eq!(
            OPERATOR_METADATA
                .prepare_instrument(
                    &input,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: 48_000,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES
                    }
                )
                .err(),
            Some(error)
        );
    }
}
