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
        id: source_id(offset),
        enabled: true,
        state: Settings { value }.encode().unwrap(),
    };
    let sources = vec![
        macro_slot(0, 0.25),
        macro_slot(3, 0.5),
        macro_slot(1, 0.25),
        Modulator {
            id: source_id(2),
            enabled: true,
            state: lfo::Settings {
                rate: lfo::Rate::Hertz(12_000.0),
                phase: 0.0,
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
