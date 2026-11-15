use super::*;
use hex_lattice::{
    NativeInstrumentDefinition, NativeInstrumentEvent, PreparedMutableInstrumentRuntime,
};
fn state() -> NativeDeviceState {
    let mut s = tests::legacy_state();
    for (id, v) in [
        (1, 1.0),
        (2, 1.0),
        (29, 1.0),
        (33, 1.0),
        (34, 1.0),
        (35, 1.0),
        (36, 0.0),
        (37, 0.001),
        (38, 0.01),
    ] {
        s = OPERATOR_METADATA
            .patch_parameter(&s, ParameterId::from_static(id), ParameterValue::Float(v))
            .unwrap();
    }
    OPERATOR_METADATA
        .patch_parameter(
            &s,
            ParameterId::from_static(20_001),
            ParameterValue::Bool(true),
        )
        .unwrap()
}
fn factory(state: &NativeDeviceState) -> Box<dyn PreparedMutableInstrumentRuntime> {
    OPERATOR_METADATA
        .prepare_instrument(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap()
}
#[test]
fn filter_retargets_reverse_and_bypass_keep_boundary_state_and_legacy_pcm() {
    let source = state();
    let original = factory(&source);
    let mut runtime = original.create_runtime();
    let on = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 69,
        velocity: 100,
    }];
    let mut prefix = [[0.0; 2]; 123];
    runtime
        .preflight_events(&on, prefix.len(), &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    runtime.process_block(&on, &mut prefix, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut age = 123;
    for frequency in [20.0, 20000.0, 333.0, 12000.0] {
        let changed = OPERATOR_METADATA
            .patch_parameter(
                &source,
                ParameterId::from_static(20_005),
                ParameterValue::Float(frequency),
            )
            .unwrap();
        let target = factory(&changed);
        let mut next = target.create_runtime();
        let hex_lattice::InstrumentHistoryRetargetPreparation::Prepared(proof) =
            target.prepare_history_retarget_target().unwrap()
        else {
            panic!("filter retarget proof")
        };
        assert!(next.can_retarget_history_from(runtime.as_ref(), proof.as_ref()));
        next.retarget_history_from(runtime.as_ref(), proof.as_ref());
        let mut expected = [[0.0; 2]; 1];
        let mut actual = expected;
        runtime.process_block(&[], &mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        next.process_block(&[], &mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(
            actual, expected,
            "first sample uses exact current ramp boundary"
        );
        let mut more = [[0.0; 2]; 16];
        next.process_block(&[], &mut more, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(more.iter().flatten().all(|x| x.is_finite()));
        age += 17;
        runtime = next;
    }
    let off = OPERATOR_METADATA
        .patch_parameter(
            &source,
            ParameterId::from_static(20_001),
            ParameterValue::Bool(false),
        )
        .unwrap();
    let target = factory(&off);
    let mut next = target.create_runtime();
    let hex_lattice::InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.prepare_history_retarget_target().unwrap()
    else {
        panic!("bypass proof")
    };
    next.retarget_history_from(runtime.as_ref(), proof.as_ref());
    let mut ramp = [[0.0; 2]; 240];
    next.process_block(&[], &mut ramp, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    age += 240;
    let settings = OperatorSettings::from_state(&off).unwrap();
    let configuration = settings.prepare_configuration(48000).unwrap();
    let note = configuration.prepare_note(69, 100).unwrap();
    for n in 0..128 {
        let mut x = [[0.0; 2]; 1];
        next.process_block(&[], &mut x, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let expected = configuration.sample(crate::shared_sine_table(), &note, age + n, u64::MAX);
        assert_eq!(x[0], [expected; 2]);
    }
}
#[test]
fn filter_output_gate_has_exact_finite_quiescence_and_reuse_resets_history() {
    let s = state();
    let f = factory(&s);
    let mut r = f.create_runtime();
    assert!(f.declared_twin_bytes() > 4096);
    let on = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 69,
        velocity: 127,
    }];
    r.preflight_events(&on, 64, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    r.process_block(
        &on,
        &mut [[0.0; 2]; 64],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let off = [NativeInstrumentEvent::NoteOff {
        frame_offset: 0,
        occurrence_id: 1,
    }];
    let frames = f.max_release_tail_frames() as usize;
    let mut release = vec![[0.0; 2]; frames];
    r.preflight_events(&off, frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    r.process_block(&off, &mut release, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(r.is_quiescent());
    let mut zero = [[0.0; 2]; 64];
    r.process_block(&[], &mut zero, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(zero, [[0.0; 2]; 64]);
    let mut fresh = f.create_runtime();
    let mut a = [[0.0; 2]; 64];
    let mut b = a;
    r.process_block(&on, &mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    fresh.process_block(&on, &mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);
}
#[test]
fn version_seven_migrates_disabled_without_consuming_reserved_formats_or_ids() {
    let current = OPERATOR_METADATA.default_state().unwrap();
    let old =
        NativeDeviceState::new(OPERATOR_TYPE_ID, 7, current.payload()[..364].to_vec()).unwrap();
    let migrated = OPERATOR_METADATA.migrate_state(&old).unwrap();
    assert_eq!(&migrated.payload()[..364], old.payload());
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&migrated, ParameterId::from_static(20_001))
            .unwrap(),
        ParameterValue::Bool(false)
    );
    for (range, default) in [(609..=612, 0.0), (613..=616, 100.0)] {
        for id in range {
            let id = ParameterId::from_static(id);
            assert!(OPERATOR_METADATA.descriptors().iter().any(|d| d.id == id));
            assert_eq!(
                OPERATOR_METADATA.read_parameter(&migrated, id).unwrap(),
                ParameterValue::Float(default)
            );
        }
    }
    let v8 =
        NativeDeviceState::new(OPERATOR_TYPE_ID, 8, current.payload()[..380].to_vec()).unwrap();
    let migrated_v8 = OPERATOR_METADATA.migrate_state(&v8).unwrap();
    assert_eq!(&migrated_v8.payload()[..380], v8.payload());
    for version in [9, 10, OPERATOR_STATE_VERSION + 1] {
        assert!(
            OperatorSettings::from_state(
                &NativeDeviceState::new(OPERATOR_TYPE_ID, version, migrated.payload().to_vec())
                    .unwrap()
            )
            .is_err()
        );
    }
    assert!(
        OPERATOR_METADATA
            .descriptors()
            .iter()
            .all(|d| d.id.value() <= 388
                || (600..=616).contains(&d.id.value())
                || (620..=639).contains(&d.id.value())
                || (513..=516).contains(&d.id.value())
                || d.id.value() >= 20001)
    );
}

#[test]
fn filter_level_carrier_and_feedback_edits_keep_actual_in_flight_boundary() {
    let mut s = state();
    s = OPERATOR_METADATA
        .patch_parameter(&s, ParameterId::from_static(78), ParameterValue::Float(0.4))
        .unwrap();
    let mut runtime = factory(&s).create_runtime();
    runtime.process_block(
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 69,
            velocity: 100,
        }],
        &mut [[0.0; 2]; 123],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    for (id, value) in [
        (2, ParameterValue::Float(0.3)),
        (29, ParameterValue::Float(0.2)),
        (78, ParameterValue::Float(0.7)),
        (513, ParameterValue::Bool(false)),
        (513, ParameterValue::Bool(true)),
        (2, ParameterValue::Float(0.8)),
        (29, ParameterValue::Float(0.9)),
        (78, ParameterValue::Float(0.1)),
    ] {
        s = OPERATOR_METADATA
            .patch_parameter(&s, ParameterId::from_static(id), value)
            .unwrap();
        let f = factory(&s);
        let mut next = f.create_runtime();
        let hex_lattice::InstrumentHistoryRetargetPreparation::Prepared(proof) =
            f.prepare_history_retarget_target().unwrap()
        else {
            panic!("proof")
        };
        assert!(next.can_retarget_history_from(runtime.as_ref(), proof.as_ref()));
        next.retarget_history_from(runtime.as_ref(), proof.as_ref());
        let mut expected = [[0.0; 2]; 1];
        let mut actual = expected;
        runtime.process_block(&[], &mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        next.process_block(&[], &mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(
            actual, expected,
            "parameter {id} must start at actual boundary"
        );
        next.process_block(
            &[],
            &mut [[0.0; 2]; 16],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        runtime = next;
    }
}

#[test]
fn activating_filter_during_feedback_ramp_keeps_its_actual_boundary() {
    let mut s = state();
    // Build an unarmed source from its legacy prefix.
    s = NativeDeviceState::new(OPERATOR_TYPE_ID, 7, s.payload()[..364].to_vec()).unwrap();
    s = OPERATOR_METADATA
        .patch_parameter(&s, ParameterId::from_static(78), ParameterValue::Float(0.4))
        .unwrap();
    let mut old = factory(&s).create_runtime();
    old.process_block(
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 69,
            velocity: 100,
        }],
        &mut [[0.0; 2]; 123],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let changed = OPERATOR_METADATA
        .patch_parameter(&s, ParameterId::from_static(2), ParameterValue::Float(0.3))
        .unwrap();
    let f = factory(&changed);
    let mut ramping = f.create_runtime();
    let hex_lattice::InstrumentHistoryRetargetPreparation::Prepared(proof) =
        f.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    assert!(ramping.can_retarget_history_from(old.as_ref(), proof.as_ref()));
    ramping.retarget_history_from(old.as_ref(), proof.as_ref());
    ramping.process_block(
        &[],
        &mut [[0.0; 2]; 17],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let enabled = OPERATOR_METADATA
        .patch_parameter(
            &changed,
            ParameterId::from_static(20_001),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let f = factory(&enabled);
    let mut filtered = f.create_runtime();
    let hex_lattice::InstrumentHistoryRetargetPreparation::Prepared(proof) =
        f.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    assert!(filtered.can_retarget_history_from(ramping.as_ref(), proof.as_ref()));
    filtered.retarget_history_from(ramping.as_ref(), proof.as_ref());
    let mut a = [[0.0; 2]; 1];
    let mut b = a;
    filtered.process_block(&[], &mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    ramping.process_block(&[], &mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);
}

#[test]
fn analytic_import_refuses_invalid_ids_gates_capacity_and_payload_without_mutation() {
    let s = state();
    let legacy = NativeDeviceState::new(OPERATOR_TYPE_ID, 7, s.payload()[..364].to_vec()).unwrap();
    let prepared = OPERATOR_METADATA
        .prepare_instrument(
            &legacy,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    let context = prepared.prepare_shared_analytic_context().unwrap().unwrap();
    let f = factory(&s);
    assert!(f.can_import_analytic_context(context.as_ref()));
    let valid = hex_lattice::NativeInstrumentAnalyticSeed {
        occurrence_id: 1,
        note: context.prepare_note(69, 100).unwrap(),
        age_frames: 123,
        gate_frame: None,
    };
    let mut runtime = f.create_runtime();
    assert!(runtime.can_import_analytic(&[valid]));
    let mut bad = valid;
    bad.occurrence_id = 0;
    assert!(!runtime.can_import_analytic(&[bad]));
    runtime.import_analytic(&[bad]);
    assert!(runtime.is_quiescent());
    bad = valid;
    bad.gate_frame = Some(124);
    assert!(!runtime.can_import_analytic(&[bad]));
    bad = valid;
    bad.age_frames = u64::MAX;
    assert!(!runtime.can_import_analytic(&[bad]));
    bad = valid;
    bad.note = hex_lattice::NativeInstrumentAnalyticSeed::default().note;
    assert!(!runtime.can_import_analytic(&[bad]));
    assert!(!runtime.can_import_analytic(&[valid, valid]));
    let too_many: Vec<_> = (1..=65)
        .map(|id| hex_lattice::NativeInstrumentAnalyticSeed {
            occurrence_id: id,
            ..valid
        })
        .collect();
    assert!(!runtime.can_import_analytic(&too_many));
    runtime.import_analytic(&[valid]);
    assert!(!runtime.is_quiescent());
    assert!(!runtime.can_import_analytic(&[valid]));
}

#[test]
fn speculative_filter_history_rolls_back_both_carriers_and_transition_clock() {
    let f = factory(&state());
    let limits = hex_lattice::NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 256,
        max_active_occurrences: 64,
        max_tail_frames: 48000 * 30,
    };
    let mut active = hex_lattice::NativeInstrumentRuntimePair::new(f.as_ref(), limits).unwrap();
    let mut reference = hex_lattice::NativeInstrumentRuntimePair::new(f.as_ref(), limits).unwrap();
    let on = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 69,
        velocity: 100,
    }];
    for pair in [&mut active, &mut reference] {
        pair.begin_block();
        pair.process_block(
            &on,
            &mut [[0.0; 2]; 123],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
        pair.commit_block();
    }
    active.begin_block();
    active
        .process_block(
            &[],
            &mut [[0.0; 2]; 128],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    active.rollback_block();
    let mut a = [[0.0; 2]; 256];
    let mut b = a;
    active.begin_block();
    active
        .process_block(&[], &mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    active.commit_block();
    reference.begin_block();
    reference
        .process_block(&[], &mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    reference.commit_block();
    assert_eq!(a, b);
}
