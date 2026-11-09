//! Exercise the real FM4 mutable pair with the same borrowed compositor used by effects.
use hex_devices::fm4::device::{FM4_DEFINITION, parameter};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, LfoWaveform, ModulationRoute,
    ModulationSourceId, ModulationTarget, NativeInstrumentDefinition, NativeInstrumentEvent,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, PreparedModulation,
    PreparedModulationSource, PreparedModulationState, TuningTable,
};
use hex_lattice_contract::{
    DeviceParameterSnapshot, NativeDeviceMetadata, NativeDeviceState, ParameterValue,
    validated_parameter_semantics,
};
use std::hint::black_box;

#[test]
fn fm4_carriers_follow_sample_routes_and_retry_without_allocating() {
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &TuningTable::default(),
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let ids = [parameter::carrier(1), parameter::carrier(2)];
    let patch = |state: &NativeDeviceState, weights: [f32; 2]| {
        let snapshot = DeviceParameterSnapshot::new(&[
            (ids[0], ParameterValue::Float(weights[0])),
            (ids[1], ParameterValue::Float(weights[1])),
        ])
        .unwrap();
        FM4_DEFINITION.patch_parameters(state, &snapshot).unwrap()
    };
    let original = FM4_DEFINITION.default_state().unwrap();
    let state = patch(&original, [0.5; 2]);
    let factory = FM4_DEFINITION
        .prepare_carrier_runtime(&state, &context)
        .unwrap();
    for descriptor in FM4_DEFINITION.descriptors() {
        let metadata =
            validated_parameter_semantics(&FM4_DEFINITION, &state, descriptor.id).unwrap();
        let prepared = factory.parameter_modulation_semantics(descriptor.id);
        assert_eq!(prepared, metadata);
        assert_eq!(
            prepared.is_some(),
            (29..=32).contains(&descriptor.id.value())
        );
    }
    let targets = ids.map(|id| ModulationTarget {
        descriptor: *factory
            .parameter_automation_descriptors()
            .iter()
            .find(|d| d.id == id)
            .unwrap(),
        semantics: factory.parameter_modulation_semantics(id).unwrap(),
        base: FM4_DEFINITION.read_parameter(&state, id).unwrap(),
    });
    let routes = std::array::from_fn::<_, 2, _>(|index| ModulationRoute {
        source: 0,
        target: ids[index],
        amount: [0.125, -0.25][index],
        range: [-1.0, 1.0],
    });
    let source = PreparedModulationSource::free_with_waveform(
        ModulationSourceId::new(1).unwrap(),
        48_000.0,
        12_000.0,
        0.0,
        LfoWaveform::Square,
    )
    .unwrap();
    let program = PreparedModulation::new(&targets, &routes, 1).unwrap();
    let mut modulation = PreparedModulationState::new(vec![source], program, 32)
        .unwrap()
        .prepare()
        .unwrap();
    let mut pair = NativeInstrumentRuntimePair::new(
        factory.as_ref(),
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 64,
            max_tail_frames: 48_000 * 120,
        },
    )
    .unwrap();
    let note = |weights: [f32; 2]| {
        let authored = patch(&state, weights);
        FM4_DEFINITION
            .prepare_instrument(&authored, &context)
            .unwrap()
            .prepare_note(60, 127)
            .unwrap()
    };
    let high = note([0.75, 0.0]);
    let low = note([0.25, 1.0]);
    let expected: [[f32; 2]; 41] = std::array::from_fn(|index| {
        let oracle = if index % 4 < 2 { &high } else { &low };
        oracle.sample(index as u64, 41, &EMPTY_DEVICE_RESOURCES)
    });
    assert!(expected.iter().any(|f| f[0].abs() > 0.0001));
    let on = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 127,
    }];
    let mut output = [[0.0; 2]; 41];
    let counts = allocation_counter::measure(|| {
        for (partitions, commit) in [([3, 17, 41], false), ([5, 27, 41], true)] {
            pair.begin_block();
            modulation.begin();
            let mut start = 0;
            for end in partitions {
                let events = if start == 0 { &on[..] } else { &[] };
                let frames = &mut output[start..end];
                modulation
                    .render_chunk(frames.len(), None, |values| {
                        pair.process_block_with_clock_and_automation(
                            events,
                            frames,
                            None,
                            &[],
                            values,
                            &EMPTY_DEVICE_RESOURCES,
                        )
                        .unwrap();
                        Ok(())
                    })
                    .unwrap();
                start = end;
            }
            assert_eq!(black_box(output), expected);
            if commit {
                pair.commit_block();
                modulation.commit().unwrap();
            } else {
                pair.rollback_block();
                modulation.rollback();
                output.fill([0.0; 2]);
            }
        }
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    for id in ids {
        assert_eq!(
            FM4_DEFINITION.read_parameter(&state, id).unwrap(),
            ParameterValue::Float(0.5)
        );
    }
}

#[test]
fn fm4_pressure_modulates_only_its_voice_and_preserves_release_and_retry() {
    use hex_lattice::{NativeInstrumentRuntimeError, PreparedPressureRoute};
    use std::num::NonZeroU64;

    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &TuningTable::default(),
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let parameter = parameter::carrier(1);
    let state = FM4_DEFINITION.default_state().unwrap();
    let semantics = validated_parameter_semantics(&FM4_DEFINITION, &state, parameter)
        .unwrap()
        .unwrap();
    let descriptor = *FM4_DEFINITION
        .descriptors()
        .iter()
        .find(|d| d.id == parameter)
        .unwrap();
    let route = PreparedPressureRoute::new(descriptor, semantics, -0.25, [0.0, 1.0]).unwrap();
    // The source-global result temporarily exceeds its maximum. The voice
    // contribution is added before that final clamp.
    assert_eq!(
        route
            .compose(ParameterValue::Float(1.0), Some(1.125), 127)
            .unwrap(),
        ParameterValue::Float(0.75)
    );
    let factory = FM4_DEFINITION
        .prepare_pressure_runtime(&state, &context, route)
        .unwrap();
    let make = || {
        NativeInstrumentRuntimePair::new(
            factory.as_ref(),
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 259,
                max_active_occurrences: 64,
                max_tail_frames: 48_000 * 120,
            },
        )
        .unwrap()
    };
    let mut pair = make();
    let mut successor = make();
    let note = |carrier, velocity| {
        let state = FM4_DEFINITION
            .patch_parameter(&state, parameter, ParameterValue::Float(carrier))
            .unwrap();
        FM4_DEFINITION
            .prepare_instrument(&state, &context)
            .unwrap()
            .prepare_note(60, velocity)
            .unwrap()
    };
    let first = [note(1.0, 127), note(0.5, 127)];
    let second = [note(1.0, 80), note(0.5, 80)];
    let expected: [[f32; 2]; 41] = std::array::from_fn(|frame| {
        let a = first[usize::from((4..9).contains(&frame))].sample(
            frame as u64,
            u64::MAX,
            &EMPTY_DEVICE_RESOURCES,
        )[0];
        let b =
            second[usize::from(frame >= 7)].sample(frame as u64, 10, &EMPTY_DEVICE_RESOURCES)[0];
        [(f64::from(a) + f64::from(b)) as f32; 2]
    });
    assert!(expected.iter().any(|f| f[0].abs() > 0.001));
    let on = |id, velocity| NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: id,
        pitch: 60,
        velocity,
    };
    let pressure = |frame_offset, id, value| NativeInstrumentEvent::NotePressure {
        frame_offset,
        occurrence_id: NonZeroU64::new(id).unwrap(),
        value,
    };
    let events = [
        on(1, 127),
        on(2, 80),
        pressure(4, 1, 127),
        pressure(7, 2, 127),
        pressure(9, 1, 0),
        NativeInstrumentEvent::NoteOff {
            frame_offset: 10,
            occurrence_id: 2,
        },
    ];
    let global = PreparedModulation::new(
        &[ModulationTarget {
            descriptor,
            semantics,
            base: ParameterValue::Float(1.0),
        }],
        &[ModulationRoute {
            source: 0,
            target: parameter,
            amount: 0.125,
            range: [-1.0, 1.0],
        }],
        1,
    )
    .unwrap();
    let macro_source =
        PreparedModulationSource::macro_value(ModulationSourceId::new(1).unwrap(), 48_000, 1.0)
            .unwrap();
    let mut global = PreparedModulationState::new(vec![macro_source], global, 41)
        .unwrap()
        .prepare()
        .unwrap();
    let combined = note(0.75, 127);
    let mut frames = [[0.0; 2]; 41];
    let counts = allocation_counter::measure(|| {
        pair.begin_block();
        pair.process_block(&events, &mut frames, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        assert_eq!(frames, expected);
        pair.rollback_block();
        frames.fill([0.0; 2]);

        pair.begin_block();
        pair.process_block(&events[..3], &mut frames[..6], &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        let shifted = [
            pressure(1, 2, 127),
            pressure(3, 1, 0),
            NativeInstrumentEvent::NoteOff {
                frame_offset: 4,
                occurrence_id: 2,
            },
        ];
        pair.process_block(&shifted, &mut frames[6..17], &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        pair.commit_block();
        successor.inherit_from(&pair).unwrap();
        successor.begin_block();
        for id in [2, 99] {
            assert_eq!(
                successor.process_block(
                    &[pressure(0, id, 127)],
                    &mut frames[17..],
                    &EMPTY_DEVICE_RESOURCES
                ),
                Err(NativeInstrumentRuntimeError::InvalidEventBatch)
            );
            assert_eq!(frames[17..], [[0.0; 2]; 24]);
        }
        successor
            .process_block(&[], &mut frames[17..], &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        successor.commit_block();
        assert_eq!(frames, expected);
        successor.reset();
        frames.fill([0.0; 2]);
        successor.begin_block();
        successor
            .process_block(&[on(1, 127)], &mut frames, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        for (age, frame) in frames.iter().enumerate() {
            assert_eq!(
                *frame,
                first[0].sample(age as u64, u64::MAX, &EMPTY_DEVICE_RESOURCES)
            );
        }
        successor.rollback_block();
        successor.reset();
        frames.fill([0.0; 2]);
        successor.begin_block();
        global.begin();
        global
            .render_chunk(frames.len(), None, |values| {
                successor
                    .process_block_with_clock_and_automation(
                        &[on(1, 127), pressure(0, 1, 127)],
                        &mut frames,
                        None,
                        &[],
                        values,
                        &EMPTY_DEVICE_RESOURCES,
                    )
                    .unwrap();
                Ok(())
            })
            .unwrap();
        for (age, frame) in frames.iter().enumerate() {
            assert_eq!(
                *frame,
                combined.sample(age as u64, u64::MAX, &EMPTY_DEVICE_RESOURCES)
            );
        }
        global.commit().unwrap();
        successor.commit_block();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
