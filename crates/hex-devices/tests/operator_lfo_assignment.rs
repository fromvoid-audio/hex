//! Independent legacy PCM oracle for the four existing pitch-assignment flags.
mod support;
use hex_devices::operator_state::OPERATOR_METADATA;
use hex_lattice::{InstrumentHistoryRetargetPreparation, NativeInstrumentEvent};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterValue};

fn patch(state: &NativeDeviceState, id: u32, value: ParameterValue) -> NativeDeviceState {
    OPERATOR_METADATA
        .patch_parameter(state, ParameterId::from_static(id), value)
        .unwrap()
}

fn state(operator: u32, fixed: bool, selected: bool, amount: f32) -> NativeDeviceState {
    let mut state = support::legacy_operator_state();
    for i in 0..4 {
        state = patch(
            &state,
            29 + i,
            ParameterValue::Float(if i == operator { 1.0 } else { 0.0 }),
        );
        state = patch(
            &state,
            89 + i,
            ParameterValue::Bool(selected && i == operator),
        );
    }
    for (id, value) in [
        (1 + operator * 3, 1.0),
        (2 + operator * 3, 1.0),
        (33 + operator * 6, 1.0),
        (34 + operator * 6, 1.0),
        (35 + operator * 6, 1.0),
        (36 + operator * 6, 0.0),
        (37 + operator * 6, 0.001),
        (38 + operator * 6, 0.005),
        (63 + operator * 3, 330.0),
        (85, 3.0),
        (87, amount),
    ] {
        state = patch(&state, id, ParameterValue::Float(value));
    }
    state = patch(&state, 62 + operator * 3, ParameterValue::Bool(fixed));
    patch(&state, 82, ParameterValue::Bool(true))
}

fn check(
    operator: u32,
    fixed: bool,
    selected: bool,
    amount: f32,
    edit_at: Option<usize>,
    partition: usize,
) {
    let registry = hex_devices::native_instrument_registry().unwrap();
    let original = state(operator, fixed, selected, amount);
    let prepared = registry
        .prepare_instrument(
            &original,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
    let mut runtime = factory.create_runtime();
    let mut phase = 0.0_f64;
    let mut at = 0;
    while at < 1100 {
        if edit_at == Some(at) {
            let target = patch(&original, 89 + operator, ParameterValue::Bool(!selected));
            let target = registry
                .prepare_instrument(
                    &target,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: 48_000,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    },
                )
                .unwrap();
            let target = target.prepare_mutable_runtime().unwrap().unwrap();
            let InstrumentHistoryRetargetPreparation::Prepared(proof) =
                target.prepare_history_retarget_target().unwrap()
            else {
                panic!("existing LFO target proof")
            };
            let mut next = target.create_runtime();
            assert!(next.can_retarget_history_from(runtime.as_ref(), proof.as_ref()));
            next.retarget_history_from(runtime.as_ref(), proof.as_ref());
            runtime = next;
        }
        let mut end = (at + partition).min(1100);
        if let Some(edit) = edit_at.filter(|edit| *edit > at) {
            end = end.min(edit);
        }
        let mut events = Vec::new();
        if at == 0 {
            events.push(NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 69,
                velocity: 127,
            });
        }
        if (at..end).contains(&768) {
            events.push(NativeInstrumentEvent::NoteOff {
                frame_offset: 768 - at,
                occurrence_id: 1,
            });
        }
        let mut pcm = vec![[0.0; 2]; end - at];
        runtime
            .preflight_events(&events, pcm.len(), &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        runtime.process_block(&events, &mut pcm, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        for (offset, actual) in pcm.iter().enumerate() {
            let age = at + offset;
            let envelope = if age < 768 {
                1.0
            } else if age < 1008 {
                0.00001_f64.powf((age - 768) as f64 / 240.0)
            } else {
                0.0
            };
            let expected = (phase.sin() * envelope) as f32;
            assert!(
                (actual[0] - expected).abs() < 3.0e-6,
                "op={operator} fixed={fixed} selected={selected} amount={amount} edit={edit_at:?} partition={partition} age={age}: {actual:?} != {expected}"
            );
            assert_eq!(actual[0], actual[1]);
            let selected = if edit_at.is_some_and(|edit| age >= edit) {
                !selected
            } else {
                selected
            };
            let multiplier = if selected {
                ((std::f64::consts::TAU * age as f64 * 3.0 / 48_000.0).sin() * f64::from(amount)
                    / 100.0)
                    .exp2()
            } else {
                1.0
            };
            phase +=
                std::f64::consts::TAU * (if fixed { 330.0 } else { 440.0 }) / 48_000.0 * multiplier;
        }
        at = end;
    }
}

#[test]
fn every_destination_zero_all_off_ratio_fixed_and_held_released_retarget_match_independent_pcm() {
    for operator in 0..4 {
        for fixed in [false, true] {
            for (selected, amount, edit) in [
                (true, 100.0, None),
                (false, 100.0, None),
                (true, 0.0, None),
                (true, 100.0, Some(512)),
                (false, 100.0, Some(900)),
            ] {
                for partition in [1, 17, 128, 256] {
                    check(operator, fixed, selected, amount, edit, partition);
                }
            }
        }
    }
}
