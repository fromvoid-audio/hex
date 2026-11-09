//! Authored banks must not reach an engine that would silently omit them.
use super::*;
use hex_lattice_contract::{DeviceTypeId, NativeDeviceMetadata, NativeDeviceState};
use hex_project::{Command, Created, DeviceModulation, Editor, ModulationSourceId, Modulator};

#[test]
fn arrangement_and_session_refuse_unknown_modulator_without_committing() {
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Device(id) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    assert!(validate_project(editor.project()).is_ok());
    let before = editor.project().snapshot();
    let refused = editor.execute_prepared(
        Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(
                vec![Modulator {
                    id: ModulationSourceId::new(before.next_id).unwrap(),
                    enabled: true,
                    state: NativeDeviceState::new(
                        DeviceTypeId::from_static("test.modulator"),
                        1,
                        vec![1],
                    )
                    .unwrap(),
                }],
                vec![],
            )
            .unwrap(),
        },
        |candidate| crate::compile::project(candidate, &Default::default()),
    );
    assert!(refused.is_err());
    assert_eq!(editor.project().snapshot(), before);
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(
                vec![Modulator {
                    id: ModulationSourceId::new(editor.project().snapshot().next_id).unwrap(),
                    enabled: true,
                    state: NativeDeviceState::new(
                        DeviceTypeId::from_static("test.modulator"),
                        1,
                        vec![1],
                    )
                    .unwrap(),
                }],
                vec![],
            )
            .unwrap(),
        })
        .unwrap();
    let project = editor.project();
    assert_eq!(
        crate::compile::project(project, &Default::default())
            .err()
            .unwrap()
            .0,
        "modulator state or rate is unsupported"
    );
    assert_eq!(
        crate::PreparedSessionInventory::prepare(
            project,
            std::sync::Arc::new(hex_devices::SineTable::new()),
            None
        )
        .err()
        .unwrap()
        .0,
        "modulator state or rate is unsupported"
    );
}

#[test]
fn saved_lfos_modulate_automation_per_sample_in_arrangement_session_and_sealed_playback() {
    use crate::{Audio, PlaybackAction, PlaybackCommand, PreparedRevision, Renderer};
    use hex_devices::{
        modulation::lfo::{Rate, Settings},
        utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, parameter},
    };
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceModulationRoute,
        ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
        ModulationRouteId,
    };
    use std::{collections::BTreeMap, sync::Arc};

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
    assert!(crate::project_device_modulation_target_supported(
        editor.project(),
        device,
        parameter::GAIN,
    ));
    assert!(!crate::project_device_modulation_target_supported(
        editor.project(),
        device,
        hex_lattice_contract::ParameterId::from_static(u32::MAX),
    ));
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: parameter::GAIN,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(2.0 / MAX_UTILITY_GAIN).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    let next = editor.project().snapshot().next_id;
    let source = |offset, rate, phase| Modulator {
        id: ModulationSourceId::new(next + offset).unwrap(),
        enabled: true,
        state: Settings { rate, phase }.encode().unwrap(),
    };
    let first = source(0, Rate::Hertz(12_000.0), 0.0);
    let second = source(1, Rate::Hertz(0.0), 0.25);
    let destination = ModulationDestination::Device {
        device_type: hex_devices::utility::UTILITY_TYPE_ID,
        target: hex_project::DeviceAutomationTarget::Parameter(parameter::GAIN),
        vst3_class: None,
    };
    let route = |offset, source, amount, range| DeviceModulationRoute {
        id: ModulationRouteId::new(next + offset).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination,
        amount: ModulationAmount::new(amount).unwrap(),
        range,
        enabled: true,
    };
    let routes = vec![
        route(
            2,
            first.id,
            1.0 / MAX_UTILITY_GAIN,
            ModulationRange::default(),
        ),
        route(
            3,
            second.id,
            -0.25 / MAX_UTILITY_GAIN,
            ModulationRange {
                start: ModulationAmount::new(0.0).unwrap(),
                end: ModulationAmount::new(1.0).unwrap(),
            },
        ),
    ];
    // Authored order is independent of the stable source IDs in the routes.
    let bank = DeviceModulation::new(vec![second, first], routes).unwrap();
    let audio = Audio::new(48_000, vec![[0.125, -0.25]; 8]).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    editor
        .execute_prepared(
            Command::ReplaceDeviceModulation {
                device,
                modulation: bank,
            },
            |project| crate::compile::project(project, &media),
        )
        .unwrap();
    let project = hex_project::Project::from_snapshot(editor.project().snapshot()).unwrap();
    let assert_pcm = |frames: [[f32; 2]; 8]| {
        for (index, frame) in frames.into_iter().enumerate() {
            let gain = 1.75 + [0.0, 1.0, 0.0, -1.0][index % 4];
            assert!(
                (frame[0] - 0.125 * gain).abs() < 1e-6,
                "left at {index}: {frame:?}"
            );
            assert!(
                (frame[1] + 0.25 * gain).abs() < 1e-6,
                "right at {index}: {frame:?}"
            );
        }
    };
    let graph = crate::compile::project(&project, &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    let mut pcm = [[0.0; 2]; 8];
    renderer.render(&mut pcm).unwrap();
    assert_pcm(pcm);

    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = crate::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    playback.render(&mut pcm[..3]).unwrap();
    playback.render(&mut pcm[3..]).unwrap();
    assert_pcm(pcm);

    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("sealed modulation playback requires the supported constructor layout");
        return;
    }
    let registry = hex_devices::native_registry().unwrap();
    let effects = crate::AudioCandidateEffects::new(&registry);
    let quote = effects.quote(&project, 1).unwrap();
    let limits = quote.storage_reservation(64).unwrap();
    let mut cell = crate::AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
    cell.copy_resident_source_on_control(asset, &audio).unwrap();
    effects
        .prepare(
            &mut cell,
            &project,
            11,
            crate::AudioSourceContext {
                project_generation: 2,
                editor_revision: 8,
            },
            None,
        )
        .unwrap();
    let summary = cell
        .owner_on_control()
        .unwrap()
        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows,
            root_rows: 1,
            max_retained_bytes: limits.max_prepared_data_bytes,
            max_construction_peak_bytes: limits.max_construction_data_bytes,
        })
        .unwrap();
    assert_eq!(summary.data.unknown_storage, 0);
    let owner = cell.owner_on_control().unwrap();
    owner
        .test_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    owner.test_render(&mut pcm[..5]).unwrap();
    owner.test_render(&mut pcm[5..]).unwrap();
    assert_pcm(pcm);
    cell.cleanup_on_control().unwrap();
}
