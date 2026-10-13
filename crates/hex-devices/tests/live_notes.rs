use hex_lattice::{
    NoteGate, PreparedAnalyticNote, PreparedLiveAnalyticNote, PreparedNativeInstrument,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId};

#[test]
fn live_gate_is_additive_and_unsupported_provider_refuses() {
    assert_eq!(std::mem::size_of::<PreparedAnalyticNote>(), 344);
    assert_eq!(std::mem::size_of::<PreparedLiveAnalyticNote>(), 344);
    assert!(!std::mem::needs_drop::<PreparedLiveAnalyticNote>());
    struct Unsupported;
    impl PreparedNativeInstrument for Unsupported {
        fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
            Err(DeviceContractError::UnsupportedProcessingClass)
        }
    }
    assert!(matches!(
        Unsupported.prepare_live_note(60, 100),
        Err(DeviceContractError::UnsupportedProcessingClass)
    ));
}

#[test]
fn sine_and_fm4_released_pcm_is_exactly_authored_and_held_has_no_deadline() {
    let registry = hex_devices::native_instrument_registry().unwrap();
    for name in ["hex.instrument.sine", "hex.instrument.fm4"] {
        let id = DeviceTypeId::from_static(name);
        let state = registry.definition(id).unwrap().default_state().unwrap();
        let prepared = registry.prepare_instrument(&state, 48_000).unwrap();
        for pitch in 60..=71 {
            let authored = prepared.prepare_note(pitch, 100).unwrap();
            let live = prepared.prepare_live_note(pitch, 100).unwrap();
            for gate in [0, 1, 17, 1000] {
                for age in 0..gate + authored.release_frames() + 1 {
                    assert_eq!(
                        live.sample(
                            age,
                            NoteGate::Released {
                                at_age_frames: gate
                            }
                        ),
                        authored.sample(age, gate),
                        "{name} pitch={pitch} age={age} gate={gate}"
                    );
                    if age < gate {
                        assert_eq!(live.sample(age, NoteGate::Held), authored.sample(age, gate));
                    }
                }
            }
            for age in [u64::MAX - 1, u64::MAX] {
                assert!(
                    live.sample(age, NoteGate::Held)
                        .iter()
                        .all(|sample| sample.is_finite()),
                    "no invented gate horizon at maximum age"
                );
            }
        }
    }
}

#[test]
fn held_sine_matches_independent_wrapping_phase_attack_oracle() {
    let registry = hex_devices::native_instrument_registry().unwrap();
    let id = DeviceTypeId::from_static("hex.instrument.sine");
    let state = registry.definition(id).unwrap().default_state().unwrap();
    let prepared = registry.prepare_instrument(&state, 48_000).unwrap();
    let live = prepared.prepare_live_note(69, 100).unwrap();
    let step = (440.0 / 48_000.0 * 18_446_744_073_709_551_616.0_f64).round() as u64;
    let velocity = 100.0_f32 / 127.0 * 0.1;
    for age in [0, 1, 17, 239, 240, 241, 1234, u64::MAX - 1, u64::MAX] {
        let phase = step.wrapping_mul(age);
        let angle = std::f64::consts::TAU * phase as f64 / 18_446_744_073_709_551_616.0;
        let expected = angle.sin() as f32 * (age as f32 / 240.0).min(1.0) * velocity;
        let actual = live.sample(age, NoteGate::Held);
        assert!(
            (actual[0] - expected).abs() < 3.0e-6,
            "age={age} {actual:?} != {expected}"
        );
        assert_eq!(actual[0], actual[1]);
    }
}

#[test]
fn operator_shared_live_gate_preserves_released_law_and_mutable_class_refuses_fallback() {
    use hex_lattice_contract::{ParameterId, ParameterValue};
    let registry = hex_devices::native_instrument_registry().unwrap();
    let id = DeviceTypeId::from_static("hex.instrument.operator");
    let definition = registry.definition(id).unwrap();
    let state = definition.default_state().unwrap();
    let prepared = registry.prepare_instrument(&state, 48_000).unwrap();
    let context = prepared.prepare_shared_analytic_context().unwrap().unwrap();
    assert!(context.supports_live_gate());
    let note = context.prepare_note(60, 100).unwrap();
    for gate in [0, 17, 1000] {
        for age in 0..gate + note.release_frames() + 1 {
            assert_eq!(
                context
                    .sample_live(
                        &note,
                        age,
                        NoteGate::Released {
                            at_age_frames: gate
                        }
                    )
                    .unwrap(),
                context.sample(&note, age, gate)
            );
            if age < gate {
                assert_eq!(
                    context.sample_live(&note, age, NoteGate::Held).unwrap(),
                    context.sample(&note, age, gate)
                );
            }
        }
    }
    assert!(
        context
            .sample_live(&note, u64::MAX, NoteGate::Held)
            .unwrap()
            .iter()
            .all(|sample| sample.is_finite())
    );
    for parameter in [78, 82] {
        let value = if parameter == 78 {
            ParameterValue::Float(0.4)
        } else {
            ParameterValue::Bool(true)
        };
        let state = definition
            .patch_parameter(&state, ParameterId::from_static(parameter), value)
            .unwrap();
        let prepared = registry.prepare_instrument(&state, 48_000).unwrap();
        assert!(
            prepared
                .prepare_shared_analytic_context()
                .unwrap()
                .is_none()
        );
        assert!(prepared.prepare_live_note(60, 100).is_err());
        assert!(prepared.prepare_mutable_runtime().unwrap().is_some());
        prepared.validate_event_note(60, 100).unwrap();
    }
}

#[test]
fn operator_open_gate_enable_continuation_keeps_phase_and_finite_gate_law() {
    use hex_lattice::{
        ANALYTIC_CONTINUATION_WORDS, AnalyticContinuationId, AnalyticParameterFrame,
    };
    use hex_lattice_contract::{ParameterId, ParameterValue};
    let registry = hex_devices::native_instrument_registry().unwrap();
    let id = DeviceTypeId::from_static("hex.instrument.operator");
    let definition = registry.definition(id).unwrap();
    let state = definition.default_state().unwrap();
    let prepared = registry.prepare_instrument(&state, 48_000).unwrap();
    let context = prepared.prepare_shared_analytic_context().unwrap().unwrap();
    let target = AnalyticContinuationId::new(1).unwrap();
    let provider = context.analytic_continuation().unwrap();
    let note = context.prepare_note(60, 100).unwrap();
    let endpoint = provider.endpoint(target);
    for age in [0, 17, 1000, u64::MAX] {
        assert_eq!(
            provider
                .sample_live_with_parameters(target, &endpoint, &note, age, NoteGate::Held)
                .unwrap(),
            context.sample_live(&note, age, NoteGate::Held).unwrap()
        );
    }
    for gate in [0, 17, 1000] {
        for age in [gate, gate + 1, gate + note.release_frames()] {
            assert_eq!(
                provider
                    .sample_live_with_parameters(
                        target,
                        &endpoint,
                        &note,
                        age,
                        NoteGate::Released {
                            at_age_frames: gate
                        }
                    )
                    .unwrap(),
                provider
                    .sample_with_parameters(target, &endpoint, &note, age, gate)
                    .unwrap()
            );
        }
    }
    let mut words = *endpoint.words();
    words[0] = 0.0_f64.to_bits();
    let off = AnalyticParameterFrame::new(target, words);
    assert_eq!(
        provider
            .sample_live_with_parameters(target, &off, &note, 1000, NoteGate::Held)
            .unwrap(),
        [0.0; 2]
    );
    words[ANALYTIC_CONTINUATION_WORDS - 1] = 1;
    assert!(
        provider
            .sample_live_with_parameters(
                target,
                &AnalyticParameterFrame::new(target, words),
                &note,
                1000,
                NoteGate::Held
            )
            .is_err()
    );
    let state = definition
        .patch_parameter(
            &state,
            ParameterId::from_static(513),
            ParameterValue::Bool(false),
        )
        .unwrap();
    let context = registry
        .prepare_instrument(&state, 48_000)
        .unwrap()
        .prepare_shared_analytic_context()
        .unwrap()
        .unwrap();
    let provider = context.analytic_continuation().unwrap();
    assert_eq!(
        provider
            .sample_live_with_parameters(
                target,
                &provider.endpoint(target),
                &note,
                1000,
                NoteGate::Held
            )
            .unwrap(),
        [0.0; 2]
    );
}
