//! Observable provider PCM and refusal checks; no engine ownership assumptions.
use super::*;
use hex_lattice::{AnalyticContinuation, AnalyticContinuationError, AnalyticContinuationId};

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

fn fixture() -> NativeDeviceState {
    let mut state = tests::legacy_state();
    // A carrier, with B modulating A. No attack/decay obscures the level law.
    for (parameter, value) in [
        (29, 1.0),
        (30, 0.0),
        (31, 0.0),
        (32, 0.0),
        (17, 0.8),
        (2, 0.6),
        (5, 0.4),
    ] {
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                ParameterId::from_static(parameter),
                ParameterValue::Float(value),
            )
            .unwrap();
    }
    for operator in 1..=4 {
        for (field, value) in [
            (parameter::EnvelopeField::Initial, 1.0),
            (parameter::EnvelopeField::Peak, 1.0),
            (parameter::EnvelopeField::Sustain, 1.0),
            (parameter::EnvelopeField::Attack, 0.0),
            (parameter::EnvelopeField::Decay, 0.001),
        ] {
            state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    parameter::envelope(operator, field),
                    ParameterValue::Float(value),
                )
                .unwrap();
        }
    }
    state
}

#[test]
fn carrier_and_modulator_enable_pcm_matches_independently_prepared_level_law() {
    let state = fixture();
    for operator in [1, 2] {
        let source = context(&state);
        let disabled = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::enabled(operator),
                ParameterValue::Bool(false),
            )
            .unwrap();
        let target = context(&disabled);
        let provider = target.analytic_continuation().unwrap();
        let source_provider = source.analytic_continuation().unwrap();
        let source_id = AnalyticContinuationId::new(1).unwrap();
        let target_id = AnalyticContinuationId::new(2).unwrap();
        let start = provider
            .prepare_start(
                target_id,
                source_provider,
                source_id,
                &source_provider.endpoint(source_id),
            )
            .unwrap();
        let transition = AnalyticContinuation::new(target_id, start, 100, 240).unwrap();
        let note = source.prepare_note(60, 100).unwrap();
        for elapsed in [0_u32, 1, 60, 120, 239, 240] {
            let frame = provider
                .parameters_at(target_id, &transition, 100 + u64::from(elapsed))
                .unwrap();
            let original_level = if operator == 1 { 0.6 } else { 0.4 };
            let oracle_state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    parameter::oscillator_level(operator),
                    ParameterValue::Float(original_level * (1.0 - elapsed as f32 / 240.0)),
                )
                .unwrap();
            let oracle = context(&oracle_state);
            let oracle_note = oracle.prepare_note(60, 100).unwrap();
            for (age, gate) in [(517, u64::MAX), (800, 600)] {
                let actual = provider
                    .sample_with_parameters(target_id, &frame, &note, age, gate)
                    .unwrap();
                let expected = oracle.sample(&oracle_note, age, gate);
                assert!(
                    (actual[0] - expected[0]).abs() < 2e-6,
                    "operator {operator}, elapsed {elapsed}, age {age}: {actual:?} vs {expected:?}"
                );
            }
        }
    }
}

#[test]
fn authored_level_edit_refuses_even_when_disabled_endpoints_match() {
    let state = OPERATOR_METADATA
        .patch_parameter(
            &fixture(),
            parameter::enabled(1),
            ParameterValue::Bool(false),
        )
        .unwrap();
    let source = context(&state);
    let changed = OPERATOR_METADATA
        .patch_parameter(
            &state,
            parameter::oscillator_level(1),
            ParameterValue::Float(0.2),
        )
        .unwrap();
    let target = context(&changed);
    let source_id = AnalyticContinuationId::new(1).unwrap();
    let target_id = AnalyticContinuationId::new(2).unwrap();
    let source_provider = source.analytic_continuation().unwrap();
    assert_eq!(
        target.analytic_continuation().unwrap().prepare_start(
            target_id,
            source_provider,
            source_id,
            &source_provider.endpoint(source_id)
        ),
        Err(AnalyticContinuationError::IncompatibleSource)
    );
}
