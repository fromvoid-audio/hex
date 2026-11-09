use crate::{
    Audio, AudioCandidateEffects, AudioCandidatePreparation, AudioSourceContext,
    AudioStorageLedgerReservation, PlaybackAction, PlaybackCommand, PreparedRevision,
    playback::Playback,
};
use hex_devices::{
    SineTable,
    modulation::macro_value::Settings,
    utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, UTILITY_TYPE_ID, parameter},
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn macro_value_updates_active_sample_frames_and_matches_the_sealed_graph() {
    let mut editor = Editor::new(crate::audio_preparation::candidate_test_project(), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: base.clone(),
        })
        .unwrap()
    else {
        panic!("utility fixture");
    };
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let routes: Vec<_> = [
        (parameter::GAIN, 1.0 / MAX_UTILITY_GAIN),
        (parameter::INVERT_LEFT, 1.0),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (parameter, amount))| DeviceModulationRoute {
        id: ModulationRouteId::new(next + index as u64 + 1).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Device {
            device_type: UTILITY_TYPE_ID,
            target: DeviceAutomationTarget::Parameter(parameter),
            vst3_class: None,
        },
        amount: ModulationAmount::new(amount).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    })
    .collect();
    let replace = |value| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(
            vec![Modulator {
                color: Default::default(),
                id: source,
                enabled: true,
                state: Settings { value }.encode().unwrap(),
            }],
            routes.clone(),
        )
        .unwrap(),
    };
    let audio = Audio::new(48_000, vec![[0.125, -0.25]; 8]).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let prepare = |project: &hex_project::Project| {
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
    };
    let compare = |samples: &[[f32; 2]], gain: f32, polarity: f32| {
        for frame in samples {
            assert!(
                (frame[0] - 0.125 * gain * polarity).abs() < 1e-6,
                "{frame:?}"
            );
            assert!((frame[1] + 0.25 * gain).abs() < 1e-6, "{frame:?}");
        }
    };
    let (_, initial) = editor.execute_prepared(replace(0.25), prepare).unwrap();
    let mut playback = Playback::try_from_prepared_with_session(Arc::new(initial)).unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut output = [[0.0; 2]; 8];
    playback.render(&mut output[..3]).unwrap();
    compare(&output[..3], 1.25, 1.0);

    let (_, updated) = editor.execute_prepared(replace(1.0), prepare).unwrap();
    assert_eq!(editor.project().device(device).unwrap().state(), &base);
    let mut successor = Playback::try_from_prepared_with_session(Arc::new(updated)).unwrap();
    successor.try_inherit_runtime_from(&mut playback).unwrap();
    successor.render(&mut output[3..]).unwrap();
    compare(&output[3..], 2.0, -1.0);

    if render_sealed(editor.project(), asset, &audio, &mut output) {
        compare(&output, 2.0, -1.0);
    }
}

#[test]
fn saved_button_switches_signed_gain_after_live_handoff() {
    use hex_devices::modulation::button;

    let mut editor = Editor::new(crate::audio_preparation::candidate_test_project(), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: base.clone(),
        })
        .unwrap()
    else {
        panic!("utility fixture");
    };
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let route = DeviceModulationRoute {
        id: ModulationRouteId::new(next + 1).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Device {
            device_type: UTILITY_TYPE_ID,
            target: DeviceAutomationTarget::Parameter(parameter::GAIN),
            vst3_class: None,
        },
        amount: ModulationAmount::new(-0.5 / MAX_UTILITY_GAIN).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    };
    let replace = |on, route| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(
            vec![Modulator {
                color: Default::default(),
                id: source,
                enabled: true,
                state: button::encode(on).unwrap(),
            }],
            vec![route],
        )
        .unwrap(),
    };
    let dry = [0.125, -0.25];
    let audio = Audio::new(48_000, vec![dry; 8]).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let prepare = |project: &hex_project::Project| {
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
    };
    let (_, initial) = editor
        .execute_prepared(replace(false, route), prepare)
        .unwrap();
    let mut playback = Playback::try_from_prepared_with_session(Arc::new(initial)).unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut output = [[0.0; 2]; 8];
    playback.render(&mut output[..3]).unwrap();
    assert_eq!(output[..3], [dry; 3]);

    let prior = editor.project().clone();
    let invalid = DeviceModulationRoute {
        range: ModulationRange {
            start: ModulationAmount::new(0.0).unwrap(),
            end: ModulationAmount::new(1.0).unwrap(),
        },
        ..route
    };
    assert!(
        editor
            .execute_prepared(replace(false, invalid), prepare)
            .is_err()
    );
    assert_eq!(*editor.project(), prior);

    let (_, updated) = editor
        .execute_prepared(replace(true, route), prepare)
        .unwrap();
    let current = editor.project().device(device).unwrap();
    assert_eq!(current.state(), &base);
    assert_eq!(current.modulation().routes(), [route]);
    let mut successor = Playback::try_from_prepared_with_session(Arc::new(updated)).unwrap();
    successor.try_inherit_runtime_from(&mut playback).unwrap();
    successor.render(&mut output[3..]).unwrap();
    assert_eq!(output[3..], [dry.map(|sample| sample * 0.5); 5]);
    if render_sealed(editor.project(), asset, &audio, &mut output) {
        assert_eq!(output, [dry.map(|sample| sample * 0.5); 8]);
    }
}

fn render_sealed(
    project: &hex_project::Project,
    asset: hex_project::AssetId,
    audio: &Audio,
    output: &mut [[f32; 2]],
) -> bool {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("sealed modulation playback requires the supported constructor layout");
        return false;
    }
    let registry = hex_devices::native_registry().unwrap();
    let effects = AudioCandidateEffects::new(&registry);
    let quote = effects.quote(project, 1).unwrap();
    let limits = quote.storage_reservation(64).unwrap();
    let mut candidate = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
    candidate
        .copy_resident_source_on_control(asset, audio)
        .unwrap();
    effects
        .prepare(
            &mut candidate,
            project,
            3,
            AudioSourceContext {
                project_generation: 2,
                editor_revision: 8,
            },
            None,
        )
        .unwrap();
    let owner = candidate.owner_on_control().unwrap();
    assert_eq!(
        owner
            .seal_constructor_on_control(AudioStorageLedgerReservation {
                data_rows: quote.data_rows,
                handle_rows: quote.handle_rows,
                root_rows: 1,
                max_retained_bytes: limits.max_prepared_data_bytes,
                max_construction_peak_bytes: limits.max_construction_data_bytes,
            })
            .unwrap()
            .data
            .unknown_storage,
        0,
    );
    owner
        .test_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    owner.test_render(&mut output[..5]).unwrap();
    owner.test_render(&mut output[5..]).unwrap();
    candidate.cleanup_on_control().unwrap();
    true
}

#[test]
fn saved_source_routes_deliver_sample_rate_pcm_and_preserve_authored_macro_values() {
    use hex_devices::modulation::{lfo, macro_value};
    let mut editor = Editor::new(crate::audio_preparation::candidate_test_project(), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("utility fixture");
    };
    let next = editor.project().next_identity();
    let source_id = |offset| ModulationSourceId::new(next + offset).unwrap();
    let macro_slot = |offset, value| Modulator {
        color: Default::default(),
        id: source_id(offset),
        enabled: true,
        state: Settings { value }.encode().unwrap(),
    };
    let sources = vec![
        macro_slot(0, 0.25),
        macro_slot(3, 0.5),
        macro_slot(1, 0.25),
        Modulator {
            color: Default::default(),
            id: source_id(2),
            enabled: true,
            state: lfo::Settings {
                rate: lfo::Rate::Hertz(12_000.0),
                phase: 0.0,
                ..lfo::Settings::default()
            }
            .encode()
            .unwrap(),
        },
    ];
    let route = |offset, source, destination, amount| DeviceModulationRoute {
        id: ModulationRouteId::new(next + 4 + offset).unwrap(),
        source: source_id(source),
        output: ModulationOutputId::MAIN,
        destination,
        amount: ModulationAmount::new(amount).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    };
    let macro_target = |index, parameter| ModulationDestination::Modulator {
        source: source_id(index),
        module_type: macro_value::TYPE_ID,
        parameter,
    };
    let mut routes = vec![
        route(
            0,
            0,
            ModulationDestination::Device {
                device_type: UTILITY_TYPE_ID,
                target: DeviceAutomationTarget::Parameter(parameter::GAIN),
                vst3_class: None,
            },
            1.0 / MAX_UTILITY_GAIN,
        ),
        route(1, 1, macro_target(0, macro_value::VALUE_PARAMETER), 1.0),
        route(2, 2, macro_target(1, macro_value::VALUE_PARAMETER), 0.25),
        route(3, 3, macro_target(1, macro_value::VALUE_PARAMETER), 0.25),
    ];
    let replacement = |routes| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(sources.clone(), routes).unwrap(),
    };
    let audio = Audio::new(48_000, vec![[0.125, -0.25]; 8]).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let prepare = |project: &hex_project::Project| {
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
    };
    let (_, revision) = editor
        .execute_prepared(replacement(routes.clone()), prepare)
        .unwrap();
    let mut playback = Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap();
    playback.apply(PlaybackCommand::Play);
    let mut pcm = [[0.0; 2]; 8];
    let compare = |output: &[[f32; 2]], modulation: &[f32]| {
        for (index, frame) in output.iter().enumerate() {
            let gain = 1.0 + modulation[index % modulation.len()];
            assert!((frame[0] - 0.125 * gain).abs() < 1e-6, "{frame:?}");
            assert!((frame[1] + 0.25 * gain).abs() < 1e-6, "{frame:?}");
        }
    };
    let expected = [0.625, 0.875, 0.625, 0.375];
    for block in pcm.chunks_mut(3) {
        playback.render(block).unwrap();
    }
    compare(&pcm, &expected);
    if render_sealed(editor.project(), asset, &audio, &mut pcm) {
        compare(&pcm, &expected);
    }

    let original = editor.project().snapshot();
    let mut invalid = routes.clone();
    invalid[2].destination = macro_target(1, hex_lattice_contract::ParameterId::from_static(2));
    assert!(
        editor
            .execute_prepared(replacement(invalid), prepare)
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), original);

    routes[2].enabled = false;
    let (_, revision) = editor
        .execute_prepared(replacement(routes), prepare)
        .unwrap();
    let mut playback = Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap();
    playback.apply(PlaybackCommand::Play);
    playback.render(&mut pcm).unwrap();
    compare(&pcm, &[0.625]);
    assert_eq!(
        editor
            .project()
            .device(device)
            .unwrap()
            .modulation()
            .sources(),
        &sources
    );
}

#[test]
fn saved_lfo_rate_route_doubles_the_free_and_synced_phase_steps_in_both_graphs() {
    use hex_devices::modulation::{lfo, macro_value};
    let mut editor = Editor::new(crate::audio_preparation::candidate_test_project(), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let state = UTILITY_DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("utility fixture");
    };
    let next = editor.project().next_identity();
    let manual_id = ModulationSourceId::new(next).unwrap();
    let lfo_id = ModulationSourceId::new(next + 1).unwrap();
    let manual = Modulator {
        color: Default::default(),
        id: manual_id,
        enabled: true,
        state: macro_value::Settings { value: 0.25 }.encode().unwrap(),
    };
    let oscillator = |rate| Modulator {
        color: Default::default(),
        id: lfo_id,
        enabled: true,
        state: lfo::Settings {
            rate,
            phase: 0.0,
            waveform: lfo::Waveform::RisingRamp,
        }
        .encode()
        .unwrap(),
    };
    let route = |offset, source, destination, amount| DeviceModulationRoute {
        id: ModulationRouteId::new(next + offset).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination,
        amount: ModulationAmount::new(amount).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    };
    let routes = vec![
        route(
            2,
            manual_id,
            ModulationDestination::Modulator {
                source: lfo_id,
                module_type: lfo::TYPE_ID,
                parameter: lfo::RATE_PARAMETER,
            },
            1.0,
        ),
        route(
            3,
            lfo_id,
            ModulationDestination::Device {
                device_type: UTILITY_TYPE_ID,
                target: DeviceAutomationTarget::Parameter(parameter::GAIN),
                vst3_class: None,
            },
            0.5 / MAX_UTILITY_GAIN,
        ),
    ];
    let command = |rate| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(vec![manual.clone(), oscillator(rate)], routes.clone())
            .unwrap(),
    };
    let audio = Audio::new(48_000, vec![[0.125, -0.25]; 8]).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let prepare = |project: &hex_project::Project| {
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
    };
    let beat = hex_project::BeatTime::from_ratio(1, 6_000).unwrap();
    assert_eq!(editor.project().tempo_map().beat_to_frame(beat).unwrap(), 4);
    editor
        .execute(Command::SetTempoPoint {
            beat,
            tempo: hex_project::Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();

    // At +1 octave, the free 6 kHz oscillator advances a quarter
    // cycle per sample. The synced oscillator also advances a quarter
    // cycle until the tempo halves at frame four, then an eighth.
    for (rate, gains) in [
        (
            lfo::Rate::Hertz(6_000.0),
            [0.5, 0.75, 1.0, 1.25, 0.5, 0.75, 1.0, 1.25],
        ),
        (
            lfo::Rate::BeatsPerCycle(1.0 / 3000.0),
            [0.5, 0.75, 1.0, 1.25, 0.5, 0.625, 0.75, 0.875],
        ),
    ] {
        let (_, revision) = editor.execute_prepared(command(rate), prepare).unwrap();
        let mut playback = Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut pcm = [[0.0; 2]; 8];
        playback.render(&mut pcm[..3]).unwrap();
        playback.render(&mut pcm[3..]).unwrap();
        let compare = |pcm: &[[f32; 2]; 8]| {
            for (index, (frame, gain)) in pcm.iter().zip(gains).enumerate() {
                assert!(
                    (frame[0] - 0.125 * gain).abs() < 1e-6,
                    "{rate:?} frame {index}: {frame:?}"
                );
                assert!(
                    (frame[1] + 0.25 * gain).abs() < 1e-6,
                    "{rate:?} frame {index}: {frame:?}"
                );
            }
        };
        compare(&pcm);
        if render_sealed(editor.project(), asset, &audio, &mut pcm) {
            compare(&pcm);
        }
        let authored = editor.project().device(device).unwrap();
        assert_eq!(authored.state(), &state);
        assert_eq!(authored.modulation().routes(), routes);
        assert_eq!(authored.modulation().sources()[1], oscillator(rate));
    }
}

#[test]
fn macro_four_routes_each_output_and_applies_an_incoming_signal_per_sample() {
    use hex_devices::modulation::{lfo, macro_four};

    let mut editor = Editor::new(crate::audio_preparation::candidate_test_project(), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("utility fixture");
    };
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let upstream = ModulationSourceId::new(next + 1).unwrap();
    let mut routes: Vec<_> = [
        (parameter::GAIN, 1.0 / MAX_UTILITY_GAIN),
        (parameter::INVERT_LEFT, 1.0),
        (parameter::INVERT_RIGHT, 1.0),
        (parameter::GAIN, 0.5 / MAX_UTILITY_GAIN),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (target, amount))| DeviceModulationRoute {
        id: ModulationRouteId::new(next + index as u64 + 2).unwrap(),
        source,
        output: ModulationOutputId::new(index as u32 + 1).unwrap(),
        destination: ModulationDestination::Device {
            device_type: UTILITY_TYPE_ID,
            target: DeviceAutomationTarget::Parameter(target),
            vst3_class: None,
        },
        amount: ModulationAmount::new(amount).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    })
    .collect();
    routes.push(DeviceModulationRoute {
        id: ModulationRouteId::new(next + 6).unwrap(),
        source: upstream,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Modulator {
            source,
            module_type: macro_four::TYPE_ID,
            parameter: macro_four::VALUE_PARAMETERS[3],
        },
        amount: ModulationAmount::new(0.25).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    });
    let macro_slot = |id, values| Modulator {
        color: Default::default(),
        id: ModulationSourceId::new(id).unwrap(),
        enabled: true,
        state: macro_four::Settings { values }.encode().unwrap(),
    };
    let replace = |values, routes| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(
            vec![
                macro_slot(next, values),
                Modulator {
                    color: Default::default(),
                    id: upstream,
                    enabled: true,
                    state: lfo::Settings {
                        rate: lfo::Rate::Hertz(12_000.0),
                        phase: 0.0,
                        waveform: lfo::Waveform::Square,
                    }
                    .encode()
                    .unwrap(),
                },
            ],
            routes,
        )
        .unwrap(),
    };
    let dry = [0.125, -0.25];
    let audio = Audio::new(48_000, vec![dry; 8]).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let prepare = |project: &hex_project::Project| {
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
    };
    let initial_values = [0.25, 1.0, 0.0, 0.5];
    let (_, initial) = editor
        .execute_prepared(replace(initial_values, routes.clone()), prepare)
        .unwrap();
    let mut playback = Playback::try_from_prepared_with_session(Arc::new(initial)).unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let compare = |frames: &[[f32; 2]], gains: &[f32], polarity: [f32; 2]| {
        assert_eq!(frames.len(), gains.len());
        for (&frame, &gain) in frames.iter().zip(gains) {
            assert!(
                (frame[0] - dry[0] * gain * polarity[0]).abs() < 1e-6,
                "{frame:?}"
            );
            assert!(
                (frame[1] - dry[1] * gain * polarity[1]).abs() < 1e-6,
                "{frame:?}"
            );
        }
    };
    let mut output = [[0.0; 2]; 8];
    playback.render(&mut output[..3]).unwrap();
    compare(&output[..3], &[1.625, 1.625, 1.375], [-1.0, 1.0]);

    let prior = editor.project().clone();
    let mut invalid = routes.clone();
    invalid[0].output = ModulationOutputId::new(5).unwrap();
    assert!(
        editor
            .execute_prepared(replace(initial_values, invalid), prepare)
            .is_err()
    );
    let over_budget = Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(
            (prior.next_identity()..prior.next_identity() + 9)
                .map(|id| macro_slot(id, initial_values))
                .collect(),
            Vec::new(),
        )
        .unwrap(),
    };
    assert!(editor.execute_prepared(over_budget, prepare).is_err());
    assert_eq!(*editor.project(), prior);

    let updated_values = [0.125, 0.0, 1.0, 0.5];
    let (_, updated) = editor
        .execute_prepared(replace(updated_values, routes), prepare)
        .unwrap();
    let mut successor = Playback::try_from_prepared_with_session(Arc::new(updated)).unwrap();
    successor.try_inherit_runtime_from(&mut playback).unwrap();
    successor.render(&mut output[3..]).unwrap();
    compare(&output[3..], &[1.25, 1.5, 1.5, 1.25, 1.25], [1.0, -1.0]);
    if render_sealed(editor.project(), asset, &audio, &mut output) {
        compare(
            &output,
            &[1.5, 1.5, 1.25, 1.25, 1.5, 1.5, 1.25, 1.25],
            [1.0, -1.0],
        );
    }
}

#[test]
fn saved_math_inputs_drive_native_gain_in_ordinary_and_closed_graphs() {
    use hex_devices::modulation::{lfo, math};

    let mut editor = Editor::new(crate::audio_preparation::candidate_test_project(), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: base.clone(),
        })
        .unwrap()
    else {
        panic!("utility");
    };
    let next = editor.project().next_identity();
    let identity = |offset| ModulationSourceId::new(next + offset).unwrap();
    let source = identity(1);
    let square = |offset, phase| Modulator {
        id: identity(offset),
        color: Default::default(),
        enabled: true,
        state: lfo::Settings {
            rate: lfo::Rate::Hertz(12_000.0),
            phase,
            waveform: lfo::Waveform::Square,
        }
        .encode()
        .unwrap(),
    };
    let slots = vec![
        square(0, 0.0),
        Modulator {
            id: source,
            color: Default::default(),
            enabled: true,
            state: math::Settings {
                operation: math::Operation::Subtract,
                inputs: [0.0, 0.25],
            }
            .encode()
            .unwrap(),
        },
        square(2, 0.25),
    ];
    let route = |id, source, destination, amount| DeviceModulationRoute {
        id: ModulationRouteId::new(next + id).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination,
        amount: ModulationAmount::new(amount).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    };
    let input = |parameter| ModulationDestination::Modulator {
        source,
        module_type: math::TYPE_ID,
        parameter,
    };
    let routes = [
        route(3, identity(2), input(math::INPUT_B), 0.125),
        route(4, identity(0), input(math::INPUT_A), 0.25),
        route(
            5,
            source,
            ModulationDestination::Device {
                device_type: UTILITY_TYPE_ID,
                target: DeviceAutomationTarget::Parameter(parameter::GAIN),
                vst3_class: None,
            },
            0.25 / MAX_UTILITY_GAIN,
        ),
    ];
    let edit = |routes| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(slots.clone(), routes).unwrap(),
    };
    let dry = [0.125, -0.25];
    let audio = Audio::new(48_000, vec![dry; 8]).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let prepare = |project: &hex_project::Project| {
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
    };
    let (_, revision) = editor
        .execute_prepared(edit(routes.to_vec()), prepare)
        .unwrap();
    let stable = editor.project().clone();
    let mut invalid = routes;
    invalid[0].destination = input(hex_lattice_contract::ParameterId::from_static(3));
    assert!(
        editor
            .execute_prepared(edit(invalid.to_vec()), prepare)
            .is_err()
    );
    assert_eq!(*editor.project(), stable);
    assert_eq!(editor.project().device(device).unwrap().state(), &base);
    let mut playback = Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut output = [[0.0; 2]; 8];
    playback.render(&mut output[..3]).unwrap();
    playback.render(&mut output[3..]).unwrap();
    let gains = [1.0, 1.125, 0.875, 0.75];
    let check = |output: &[[f32; 2]]| {
        for (&frame, &gain) in output.iter().zip(gains.iter().cycle()) {
            for (&actual, expected) in frame.iter().zip(dry.map(|sample| sample * gain)) {
                assert!((actual - expected).abs() < 1e-6, "{frame:?}");
            }
        }
    };
    check(&output);
    if render_sealed(editor.project(), asset, &audio, &mut output) {
        check(&output);
    }
}

#[test]
fn saved_buttons_route_two_latches_independently_after_live_handoff() {
    use hex_devices::modulation::buttons;

    let mut editor = Editor::new(crate::audio_preparation::candidate_test_project(), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: base.clone(),
        })
        .unwrap()
    else {
        panic!("utility");
    };
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let routes = [
        (parameter::GAIN, 0.5 / MAX_UTILITY_GAIN),
        (parameter::INVERT_RIGHT, 1.0),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (parameter, amount))| DeviceModulationRoute {
        id: ModulationRouteId::new(next + index as u64 + 1).unwrap(),
        source,
        output: ModulationOutputId::new(index as u32 + 1).unwrap(),
        destination: ModulationDestination::Device {
            device_type: UTILITY_TYPE_ID,
            target: DeviceAutomationTarget::Parameter(parameter),
            vst3_class: None,
        },
        amount: ModulationAmount::new(amount).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    })
    .collect::<Vec<_>>();
    let edit = |on, routes| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(
            vec![Modulator {
                id: source,
                color: Default::default(),
                enabled: true,
                state: buttons::Settings { on }.encode().unwrap(),
            }],
            routes,
        )
        .unwrap(),
    };
    let dry = [0.125, -0.25];
    let audio = Audio::new(48_000, vec![dry; 8]).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let prepare = |project: &hex_project::Project| {
        PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None)
    };
    let (_, initial) = editor
        .execute_prepared(edit([false; 2], routes.clone()), prepare)
        .unwrap();
    let mut playback = Playback::try_from_prepared_with_session(Arc::new(initial)).unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut output = [[0.0; 2]; 8];
    playback.render(&mut output[..2]).unwrap();
    assert_eq!(output[..2], [dry; 2]);

    let stable = editor.project().clone();
    let mut invalid = routes.clone();
    invalid[1].output = ModulationOutputId::new(3).unwrap();
    assert!(
        editor
            .execute_prepared(edit([true, false], invalid), prepare)
            .is_err()
    );
    let mut invalid = routes.clone();
    invalid[0].range.start = ModulationAmount::new(0.0).unwrap();
    assert!(
        editor
            .execute_prepared(edit([true, false], invalid), prepare)
            .is_err()
    );
    assert_eq!(*editor.project(), stable);

    for (window, on, frame) in [
        (2..5, [true, false], [0.1875, -0.375]),
        (5..8, [false, true], [0.125, 0.25]),
    ] {
        let (_, next) = editor
            .execute_prepared(edit(on, routes.clone()), prepare)
            .unwrap();
        let mut successor = Playback::try_from_prepared_with_session(Arc::new(next)).unwrap();
        successor.try_inherit_runtime_from(&mut playback).unwrap();
        successor.render(&mut output[window.clone()]).unwrap();
        assert!(output[window].iter().all(|&actual| actual == frame));
        playback = successor;
    }
    let device = editor.project().device(device).unwrap();
    assert_eq!(device.state(), &base);
    assert_eq!(device.modulation().routes(), routes);
    if render_sealed(editor.project(), asset, &audio, &mut output) {
        assert_eq!(output, [[0.125, 0.25]; 8]);
    }
}
