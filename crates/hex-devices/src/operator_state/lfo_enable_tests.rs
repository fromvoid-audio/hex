//! Enable changes preserve integrated phases and apply scalar levels inside FM.
use super::*;
use crate::operator_state::{OPERATOR_METADATA, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

fn state(feedback: f32) -> hex_lattice_contract::NativeDeviceState {
    let mut state = OPERATOR_METADATA.default_state().unwrap();
    for (id, value) in [
        (2, 0.6),
        (5, 0.4),
        (17, 0.8),
        (29, 1.0),
        (33, 1.0),
        (34, 1.0),
        (35, 1.0),
        (36, 0.0),
        (37, 0.001),
        (38, 0.03),
        (39, 1.0),
        (40, 1.0),
        (41, 1.0),
        (42, 0.0),
        (43, 0.001),
        (44, 0.03),
        (79, feedback),
        (87, 30.0),
    ] {
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                hex_lattice_contract::ParameterId::from_static(id),
                ParameterValue::Float(value),
            )
            .unwrap();
    }
    OPERATOR_METADATA
        .patch_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(82),
            ParameterValue::Bool(true),
        )
        .unwrap()
}

fn factory(state: &hex_lattice_contract::NativeDeviceState) -> OperatorLfoFactory {
    let settings = OperatorSettings::from_state(state).unwrap();
    prepare_factory(
        &settings,
        settings.prepare_configuration(48_000).unwrap(),
        48_000,
        crate::shared_sine_table(),
    )
    .unwrap()
    .unwrap()
}

fn runtime(factory: OperatorLfoFactory) -> Runtime {
    Runtime {
        factory,
        voices: [Voice::default(); VOICES],
        phase: Phase::default(),
        enable_ramp: None,
        released: 0,
    }
}

fn enabled(
    state: &hex_lattice_contract::NativeDeviceState,
    operator: u32,
    enabled: bool,
) -> hex_lattice_contract::NativeDeviceState {
    OPERATOR_METADATA
        .patch_parameter(
            state,
            parameter::enabled(operator),
            ParameterValue::Bool(enabled),
        )
        .unwrap()
}

fn start(runtime: &mut Runtime) {
    runtime.process_block(
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 69,
            velocity: 100,
        }],
        &mut [[0.0; 2]; 80],
    );
}

#[test]
fn lfo_enable_levels_match_prepared_carrier_modulator_oracle_with_feedback_and_release() {
    for feedback in [0.0, 0.4] {
        for operator in [1, 2] {
            let original = state(feedback);
            let mut source = runtime(factory(&original));
            start(&mut source);
            source.process_block(
                &[NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 1,
                }],
                &mut [[0.0; 2]; 3],
            );
            let mut target = runtime(factory(&enabled(&original, operator, false)));
            assert!(!target.can_copy_history_from(&source));
            let proof = Retarget(target.factory.configuration.configuration_id());
            assert!(target.can_retarget_history_from(&source, &proof));
            target.retarget_history_from(&source, &proof);
            assert_eq!(target.voices[0].phases, source.voices[0].phases);
            assert_eq!(target.voices[0].previous, source.voices[0].previous);
            assert_eq!(target.voices[0].age, source.voices[0].age);
            assert_eq!(target.voices[0].gate, source.voices[0].gate);
            for elapsed in 0..=240 {
                let amount = elapsed as f32 / 240.0;
                let authored = if operator == 1 { 0.6 } else { 0.4 };
                let oracle_state = OPERATOR_METADATA
                    .patch_parameter(
                        &original,
                        parameter::oscillator_level(operator),
                        ParameterValue::Float(authored * (1.0 - amount)),
                    )
                    .unwrap();
                let oracle = factory(&oracle_state);
                let voice = target.voices[0];
                let expected = oracle
                    .configuration
                    .sample_lfo(
                        oracle.table,
                        &voice.note,
                        voice.age,
                        voice.gate,
                        &voice.phases,
                        oracle.feedback,
                        voice.previous,
                    )
                    .0;
                let mut actual = [[0.0; 2]; 1];
                target.process_block(&[], &mut actual);
                assert!(
                    (actual[0][0] - expected).abs() < 3e-6,
                    "feedback {feedback}, operator {operator}, elapsed {elapsed}: {:?} vs {expected}",
                    actual[0]
                );
            }
            assert!(target.enable_ramp.is_none());
        }
    }
}

#[test]
fn lfo_reversal_copy_new_onsets_partition_and_pair_budget_preserve_current_boundary() {
    let original = state(0.4);
    let mut source = runtime(factory(&original));
    start(&mut source);
    let mut off = runtime(factory(&enabled(&original, 2, false)));
    off.retarget_history_from(
        &source,
        &Retarget(off.factory.configuration.configuration_id()),
    );
    off.process_block(&[], &mut [[0.0; 2]; 60]);
    let boundary = off.current_levels();
    let mut back = runtime(factory(&original));
    back.retarget_history_from(
        &off,
        &Retarget(back.factory.configuration.configuration_id()),
    );
    assert_eq!(back.current_levels(), boundary);
    let mut copy = runtime(back.factory.clone());
    assert!(copy.can_copy_history_from(&back));
    copy.copy_history_from(&back);
    assert_eq!(copy.current_levels(), boundary);
    assert_eq!(copy.enable_ramp.unwrap().elapsed, 0);
    eprintln!(
        "enable mutable storage: runtime={} twins={} cap=8192 retarget={} voices={}",
        size_of::<Runtime>(),
        size_of::<Runtime>() * 2,
        size_of::<Retarget>(),
        VOICES,
    );
    assert!(
        size_of::<Runtime>() * 2 <= 8192,
        "pair {}",
        size_of::<Runtime>() * 2
    );
    let event = NativeInstrumentEvent::NoteOn {
        frame_offset: 17,
        occurrence_id: 2,
        pitch: 72,
        velocity: 80,
    };
    let mut together = [[0.0; 2]; 300];
    back.process_block(&[event], &mut together);
    let mut divided = [[0.0; 2]; 300];
    for (at, frame) in divided.iter_mut().enumerate() {
        let events = if at == 17 {
            vec![NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 72,
                velocity: 80,
            }]
        } else {
            Vec::new()
        };
        copy.process_block(&events, std::slice::from_mut(frame));
    }
    assert_eq!(together, divided);
    assert!(back.enable_ramp.is_none());
    back.reset();
    assert!(back.enable_ramp.is_none());
    assert!(back.is_quiescent());
}
