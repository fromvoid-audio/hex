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
    exercise_fm4_sample_routes(Fm4Targets::Carriers);
}

#[test]
fn fm4_oscillator_levels_follow_sample_routes_and_retry_without_allocating() {
    exercise_fm4_sample_routes(Fm4Targets::Levels);
}

#[test]
fn fm4_oscillator_phases_follow_sample_routes_and_retry_without_allocating() {
    exercise_fm4_sample_routes(Fm4Targets::Phases);
}

#[test]
fn fm4_existing_edges_follow_signed_sample_routes_and_retry_without_allocating() {
    exercise_fm4_sample_routes(Fm4Targets::Edges);
}

enum Fm4Targets {
    Carriers,
    Levels,
    Phases,
    Edges,
}

fn exercise_fm4_sample_routes(targets: Fm4Targets) {
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &TuningTable::default(),
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let (ids, route_scale) = match targets {
        Fm4Targets::Carriers => ([parameter::carrier(1), parameter::carrier(2)], 1.0),
        Fm4Targets::Levels => (
            [
                parameter::oscillator_level(1),
                parameter::oscillator_level(2),
            ],
            0.25,
        ),
        Fm4Targets::Phases => (
            [
                parameter::oscillator_phase(1),
                parameter::oscillator_phase(2),
            ],
            2.0,
        ),
        Fm4Targets::Edges => (
            [parameter::modulation(2, 1), parameter::modulation(3, 2)],
            0.125,
        ),
    };
    let patch = |state: &NativeDeviceState, weights: [f32; 2]| {
        let snapshot = DeviceParameterSnapshot::new(&[
            (ids[0], ParameterValue::Float(weights[0])),
            (ids[1], ParameterValue::Float(weights[1])),
        ])
        .unwrap();
        FM4_DEFINITION.patch_parameters(state, &snapshot).unwrap()
    };
    let mut original = FM4_DEFINITION.default_state().unwrap();
    if !matches!(targets, Fm4Targets::Carriers) {
        original = FM4_DEFINITION
            .patch_parameter(
                &original,
                parameter::modulation(2, 1),
                ParameterValue::Float(0.25),
            )
            .unwrap();
    }
    let base = if matches!(targets, Fm4Targets::Edges) {
        [0.25, -0.25]
    } else {
        [0.5; 2]
    };
    let state = patch(&original, base);
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
            matches!(
                descriptor.id.value(),
                2 | 3 | 5 | 6 | 8 | 9 | 11 | 12 | 29..=32
            ) || (!matches!(targets, Fm4Targets::Carriers)
                && descriptor.id == parameter::modulation(2, 1))
                || (matches!(targets, Fm4Targets::Edges)
                    && descriptor.id == parameter::modulation(3, 2))
        );
    }
    let declared = ids.map(|id| ModulationTarget {
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
        amount: [0.125, -0.25][index] * route_scale,
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
    let program = PreparedModulation::new(&declared, &routes, 1).unwrap();
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
    let (high, low) = if matches!(targets, Fm4Targets::Phases) {
        let max = f64::from(f32::from_bits(0x3f7f_ffff));
        let base = f64::from((0.5 / max) as f32);
        let phase = |sign: f64| {
            [0.25, -0.5].map(|depth| {
                let normalized = (base + depth * sign).clamp(0.0, 1.0) as f32;
                (f64::from(normalized) * max) as f32
            })
        };
        (note(phase(1.0)), note(phase(-1.0)))
    } else if matches!(targets, Fm4Targets::Edges) {
        (note([0.5, -0.75]), note([0.0, 0.25]))
    } else {
        (note([0.75, 0.0]), note([0.25, 1.0]))
    };
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
    for (id, value) in ids.into_iter().zip(base) {
        assert_eq!(
            FM4_DEFINITION.read_parameter(&state, id).unwrap(),
            ParameterValue::Float(value)
        );
    }
}

#[test]
fn fm4_saved_bank_renders_and_hands_off_through_the_callback_without_allocation() {
    use hex_audio::prepared;
    use hex_devices::{
        fm4::device::FM4_TYPE_ID,
        modulation::lfo::{Rate, Settings, Waveform},
    };
    use hex_engine::{
        PreparedRevision, SineTable,
        playback::{Playback, PlaybackCommand},
    };
    use hex_project::{
        BeatTime, Command, Created, DeviceAutomationTarget, DeviceModulation,
        DeviceModulationRoute, Editor, ModulationAmount, ModulationDestination, ModulationOutputId,
        ModulationRange, ModulationRouteId, ModulationSourceId, Modulator, Project,
    };
    use std::{collections::BTreeMap, sync::Arc};

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "fm4".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: FM4_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let length = BeatTime::from_quarters(4).unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: length,
            pitch: 60,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let slot = Modulator {
        id: source,
        color: Default::default(),
        enabled: true,
        state: Settings {
            rate: Rate::Hertz(12_000.0),
            phase: 0.0,
            waveform: Waveform::Square,
        }
        .encode()
        .unwrap(),
    };
    let mut route = DeviceModulationRoute {
        id: ModulationRouteId::new(next + 1).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Device {
            device_type: FM4_TYPE_ID,
            target: DeviceAutomationTarget::Parameter(parameter::carrier(1)),
            vst3_class: None,
        },
        amount: ModulationAmount::new(0.25).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    };
    let prepare = |editor: &mut Editor, route| {
        editor
            .execute(Command::ReplaceDeviceModulation {
                device,
                modulation: DeviceModulation::new(vec![slot.clone()], vec![route]).unwrap(),
            })
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        Playback::try_from_prepared_with_session(revision).unwrap()
    };
    let initial = prepare(&mut editor, route);
    route.amount = ModulationAmount::new(-0.4).unwrap();
    let changed = prepare(&mut editor, route);
    editor.execute(Command::RemoveTrack(track)).unwrap();
    let detached = Playback::try_from_prepared_with_session(Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    assert!(control.publish(initial).is_ok());
    control.command(PlaybackCommand::Play).unwrap();
    let mut frames = [[0.0; 2]; 131];
    let mut render = || {
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(black_box(&mut frames)).unwrap();
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
        assert!(frames.iter().flatten().any(|v| v.abs() > 0.0001));
    };
    render();
    assert!(control.publish(changed).is_ok());
    render();
    assert!(control.publish(detached).is_ok());
    render();
    assert!(control.status.rejected_revision().is_none());
    assert!(control.reclaim() > 0);
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

#[test]
fn saved_fm4_note_pressure_keeps_its_unclamped_source_value_during_detachment() {
    use hex_audio::prepared;
    use hex_devices::{
        fm4::device::FM4_TYPE_ID,
        modulation::{macro_value, note_pressure},
    };
    use hex_engine::{
        PreparedRevision, SineTable,
        computer_keyboard::{ComputerKeyboardInput, LiveNoteEvent},
        playback::{Playback, PlaybackCommand},
    };
    use hex_project::{
        Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor,
        ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
        ModulationRouteId, ModulationSourceId, Modulator, Project,
    };
    use std::{collections::BTreeMap, sync::Arc};

    const RATE: u32 = 48_000;
    const DETACH: u64 = 17;
    let context = DevicePreparationContext {
        sample_rate: RATE,
        tuning: &TuningTable::default(),
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let state = FM4_DEFINITION.default_state().unwrap();
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "expressive fm4".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let next = editor.project().next_identity();
    let manual = ModulationSourceId::new(next).unwrap();
    let pressure = ModulationSourceId::new(next + 1).unwrap();
    let slot = |id, state| Modulator {
        id,
        color: Default::default(),
        enabled: true,
        state,
    };
    let slots = vec![
        slot(
            manual,
            macro_value::Settings { value: 1.0 }.encode().unwrap(),
        ),
        slot(pressure, note_pressure::default_state().unwrap()),
    ];
    let target = parameter::carrier(1);
    let route = |id, source, amount, range| DeviceModulationRoute {
        id: ModulationRouteId::new(id).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Device {
            device_type: FM4_TYPE_ID,
            target: DeviceAutomationTarget::Parameter(target),
            vst3_class: None,
        },
        amount: ModulationAmount::new(amount).unwrap(),
        range,
        enabled: true,
    };
    let routes = [
        route(next + 2, manual, 0.125, ModulationRange::default()),
        route(
            next + 3,
            pressure,
            -0.25,
            ModulationRange {
                start: ModulationAmount::new(0.0).unwrap(),
                end: ModulationAmount::new(1.0).unwrap(),
            },
        ),
    ];
    let edit = |routes| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(slots.clone(), routes).unwrap(),
    };
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| {
        PreparedRevision::prepare(project, &BTreeMap::new(), Arc::clone(&table), None)
    };
    let (_, revision) = editor
        .execute_prepared(edit(routes.to_vec()), prepare)
        .unwrap();
    let authored = editor.project().clone();
    let mut invalid = routes;
    invalid[1].destination = ModulationDestination::Device {
        device_type: FM4_TYPE_ID,
        target: DeviceAutomationTarget::Parameter(parameter::RELEASE),
        vst3_class: None,
    };
    assert!(
        editor
            .execute_prepared(edit(invalid.to_vec()), prepare)
            .is_err()
    );
    assert_eq!(editor.project(), &authored);

    let mut playback = Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap();
    let input = Arc::new(ComputerKeyboardInput::default().with_live_notes().unwrap());
    playback
        .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
        .unwrap();
    let queue = input.live_notes().unwrap();
    let activation = queue.current_activation_on_control().unwrap();
    let (mut control, mut callback) = prepared::channel(RATE).unwrap();
    assert!(control.publish(playback).is_ok());
    control.command(PlaybackCommand::Play).unwrap();

    let on = |channel, velocity| LiveNoteEvent::NoteOn {
        channel,
        pitch: 60,
        velocity,
    };
    for event in [
        on(0, 100),
        on(1, 80),
        LiveNoteEvent::PolyPressure {
            channel: 0,
            pitch: 60,
            value: 127,
        },
    ] {
        queue.publish(activation, event).unwrap();
    }
    let mut actual = [[0.0; 2]; DETACH as usize];
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
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
    let note = |carrier, velocity| {
        let state = FM4_DEFINITION
            .patch_parameter(&state, target, ParameterValue::Float(carrier))
            .unwrap();
        FM4_DEFINITION
            .prepare_instrument(&state, &context)
            .unwrap()
            .prepare_note(60, velocity)
            .unwrap()
    };
    let first = [note(1.0, 100), note(0.75, 100)];
    let second = note(1.0, 80);
    let sum = |a: [f32; 2], b: [f32; 2]| [(f64::from(a[0]) + f64::from(b[0])) as f32; 2];
    let expected: [[f32; 2]; DETACH as usize] = std::array::from_fn(|index| {
        let a =
            first[usize::from(index >= 2)].sample(index as u64, u64::MAX, &EMPTY_DEVICE_RESOURCES);
        let b = index.checked_sub(1).map_or([0.0; 2], |age| {
            second.sample(age as u64, u64::MAX, &EMPTY_DEVICE_RESOURCES)
        });
        sum(a, b)
    });
    assert_eq!(actual, expected);

    editor.execute(Command::RemoveTrack(track)).unwrap();
    let detached =
        Playback::try_from_prepared_with_session(Arc::new(prepare(editor.project()).unwrap()))
            .unwrap();
    assert!(control.publish(detached).is_ok());
    let mut tail = [[0.0; 2]; 31];
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut tail).unwrap();
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
    for (index, frame) in tail.into_iter().enumerate() {
        let age = DETACH + index as u64;
        let expected = sum(
            first[1].sample(age, DETACH, &EMPTY_DEVICE_RESOURCES),
            second.sample(age - 1, DETACH - 1, &EMPTY_DEVICE_RESOURCES),
        );
        assert_eq!(frame, expected, "detached frame {index}");
    }
    assert!(control.status.rejected_revision().is_none());
}
