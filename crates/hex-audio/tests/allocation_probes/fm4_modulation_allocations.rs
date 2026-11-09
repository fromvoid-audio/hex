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
