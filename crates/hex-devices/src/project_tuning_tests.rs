use super::*;
use hex_lattice::TuningTable;

fn table(hz: f64) -> TuningTable {
    TuningTable::new([hz; 128]).unwrap()
}
fn instrument(kind: DeviceTypeId, table: &TuningTable) -> Box<dyn PreparedNativeInstrument> {
    let registry = native_instrument_registry().unwrap();
    let state = registry.default_state(kind).unwrap();
    registry
        .prepare_instrument(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: table,
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
}
#[test]
fn neutral_preparation_preserves_all_builtin_pcm() {
    let registry = native_instrument_registry().unwrap();
    for kind in [
        SINE_INSTRUMENT_TYPE_ID,
        fm4::device::FM4_TYPE_ID,
        operator_state::OPERATOR_TYPE_ID,
    ] {
        let state = registry.default_state(kind).unwrap();
        let old = registry
            .prepare_instrument(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap();
        let new = registry
            .prepare_instrument(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap();
        let old_context = old.prepare_shared_analytic_context().unwrap();
        let new_context = new.prepare_shared_analytic_context().unwrap();
        if let (Some(old), Some(new)) = (old_context, new_context) {
            let a = old.prepare_note(60, 100).unwrap();
            let b = new.prepare_note(60, 100).unwrap();
            for age in 0..2048 {
                assert_eq!(
                    old.sample(&a, age, 777, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                    new.sample(&b, age, 777, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                );
            }
        } else if let (Some(old), Some(new)) = (
            old.prepare_mutable_runtime().unwrap(),
            new.prepare_mutable_runtime().unwrap(),
        ) {
            let limits = hex_lattice::NativeInstrumentRuntimeLimits {
                max_pair_bytes: old.declared_twin_bytes() * 2,
                max_events_per_block: old.max_events_per_block(),
                max_active_occurrences: old.max_active_occurrences(),
                max_tail_frames: old.max_release_tail_frames(),
            };
            let mut a =
                hex_lattice::NativeInstrumentRuntimePair::new(old.as_ref(), limits).unwrap();
            let mut b =
                hex_lattice::NativeInstrumentRuntimePair::new(new.as_ref(), limits).unwrap();
            let events = [
                hex_lattice::NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 100,
                },
                hex_lattice::NativeInstrumentEvent::NoteOff {
                    frame_offset: 777,
                    occurrence_id: 1,
                },
            ];
            let mut x = [[0.0; 2]; 2048];
            let mut y = x;
            a.begin_block();
            b.begin_block();
            a.process_block(&events, &mut x, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .unwrap();
            b.process_block(&events, &mut y, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .unwrap();
            assert_eq!(x, y);
        } else {
            let a = old.prepare_note(60, 100).unwrap();
            let b = new.prepare_note(60, 100).unwrap();
            for age in 0..2048 {
                assert_eq!(
                    a.sample(age, 777, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                    b.sample(age, 777, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                );
            }
        }
    }
}
#[test]
fn sine_custom_pitch_and_rapid_held_or_released_edits_follow_independent_phase_oracle() {
    let a = instrument(SINE_INSTRUMENT_TYPE_ID, &table(240.0))
        .prepare_note(69, 127)
        .unwrap();
    let b = instrument(SINE_INSTRUMENT_TYPE_ID, &table(333.0))
        .prepare_note(69, 127)
        .unwrap();
    let c = instrument(SINE_INSTRUMENT_TYPE_ID, &table(196.0))
        .prepare_note(69, 127)
        .unwrap();
    let step = |hz: f64| (hz / 48000.0 * PHASE_SCALE).round() as u64;
    let b = b.continue_from(&a, 777).unwrap();
    let c = c.continue_from(&b, 913).unwrap();
    let offset = step(240.0)
        .wrapping_sub(step(333.0))
        .wrapping_mul(777)
        .wrapping_add(step(333.0).wrapping_sub(step(196.0)).wrapping_mul(913));
    let lookup = SineTable::new();
    for gate in [800, u64::MAX] {
        for age in 913..1300 {
            let amplitude = if age < gate {
                1.0
            } else {
                1.0 - (age - gate) as f32 / 2400.0
            };
            let expected = lookup.sample_phase(step(196.0).wrapping_mul(age).wrapping_add(offset))
                * amplitude
                * 0.1;
            assert_eq!(
                c.sample(age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                [expected; 2]
            );
        }
    }
    assert_eq!(
        a.sample(777, 800, &hex_lattice::EMPTY_DEVICE_RESOURCES),
        b.sample(777, 800, &hex_lattice::EMPTY_DEVICE_RESOURCES)
    );
    assert_eq!(
        b.sample(913, 800, &hex_lattice::EMPTY_DEVICE_RESOURCES),
        c.sample(913, 800, &hex_lattice::EMPTY_DEVICE_RESOURCES)
    );
}
#[test]
fn fm4_continuation_preserves_all_oscillator_phases_and_refuses_other_law_changes() {
    let registry = native_instrument_registry().unwrap();
    let state = registry.default_state(fm4::device::FM4_TYPE_ID).unwrap();
    let prepare = |state: &NativeDeviceState, hz| {
        registry
            .prepare_instrument(
                state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &table(hz),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .prepare_note(60, 100)
            .unwrap()
    };
    let a = prepare(&state, 240.0);
    let b = prepare(&state, 333.0);
    let continued = b.continue_from(&a, 777).unwrap();
    for gate in [700, 9000] {
        assert_eq!(
            a.sample(777, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES),
            continued.sample(777, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        );
    }
    let changed = registry
        .patch_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(3),
            hex_lattice_contract::ParameterValue::Float(0.25),
        )
        .unwrap();
    assert!(prepare(&changed, 333.0).continue_from(&a, 777).is_err());
    let mut source = [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS];
    source[..4].fill(1);
    source[32] = 0x03020100;
    let mut target = source;
    target[32] = 0x03020200;
    assert!(fm4::continue_fm4_note(&source, &target, 1).is_err());
}
