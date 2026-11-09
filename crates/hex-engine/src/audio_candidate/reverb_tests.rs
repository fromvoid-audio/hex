//! Public quote plus actual original constructor DATA, including legacy migration.
use super::*;
use crate::{Audio, AudioSourceContext};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState};
use hex_project::{Command, Created, Editor, Project};
#[test]
fn reverb_public_quote_covers_each_real_ring_allocation_and_versioned_state_migration() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("reverb native constructor: unsupported layout, not exercised");
        return;
    }
    for legacy in [false, true] {
        let mut edit = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let Created::Imported { track, asset, .. } = edit
            .execute(Command::ImportAudioTrack {
                name: "room".into(),
                path: "original.wav".into(),
                sample_rate: 48000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("import");
        };
        let state = if legacy {
            NativeDeviceState::new(
                hex_devices::reverb::REVERB_TYPE_ID,
                1,
                [2.1_f32, 73.0]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect(),
            )
            .unwrap()
        } else {
            hex_devices::reverb::REVERB_DEFINITION
                .default_state()
                .unwrap()
        };
        edit.execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap();
        let Created::ReturnTrack(bus) = edit
            .execute(Command::AddReturnTrack {
                name: "room return".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        edit.execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state,
        })
        .unwrap();
        edit.execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.4,
        })
        .unwrap();
        AudioCandidateEffects::with_host_definitions(&[], |effects| {
            let quote = effects.quote(edit.project(), 1).unwrap();
            let limits = quote.storage_reservation(8 * 8).unwrap();
            let mut preparation = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
            preparation
                .copy_resident_source_on_control(
                    asset,
                    &Audio::new(48000, vec![[0.25, -0.1]; 8]).unwrap(),
                )
                .unwrap();
            effects
                .prepare(
                    &mut preparation,
                    edit.project(),
                    11,
                    AudioSourceContext {
                        project_generation: 3,
                        editor_revision: 7,
                    },
                    None,
                )
                .unwrap();
            let summary = preparation
                .owner_on_control()
                .unwrap()
                .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                    data_rows: quote.data_rows,
                    handle_rows: quote.handle_rows,
                    root_rows: 8,
                    max_retained_bytes: limits.max_prepared_data_bytes,
                    max_construction_peak_bytes: limits.max_construction_data_bytes,
                })
                .unwrap();
            assert_eq!(summary.data.unknown_storage, 0);
            assert!(
                summary.data.data_allocations
                    >= 4 * (1 + hex_devices::reverb::RUNTIME_HEAP_ALLOCATIONS)
            );
            assert!(summary.data.data_allocations <= quote.data_rows);
            assert!(summary.owned_runtime_bytes >= 4 * 240_000);
            preparation.cleanup_on_control().unwrap();
            assert!(!preparation.failed_incomplete());
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn reverb_modulation_follows_host_samples_in_the_96_khz_room() {
    use crate::{PlaybackAction, PlaybackCommand, Renderer};
    use hex_devices::{
        modulation::lfo::{Rate, Settings},
        reverb::{REVERB_DEFINITION as room, REVERB_TYPE_ID, ReverbDefinition, parameter as p},
    };
    use hex_lattice::{EMPTY_DEVICE_RESOURCES, OutputTail, PreparedNativeProcessor};
    use hex_lattice_contract::{ParameterId, ParameterValue};
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime,
        DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, ElementColor,
        ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
        ModulationRouteId, ModulationSourceId, Modulator,
    };
    use std::collections::BTreeMap;

    const RATE: u32 = 96_000;
    const FRAMES: usize = 1_409;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "room".into(),
            path: "room.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let state = room.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("effect fixture");
    };
    let tail = |ids| {
        ReverbDefinition::prepare_owned_room_for_parameters(&state, RATE, ids)
            .unwrap()
            .output_tail()
    };
    let OutputTail::Finite {
        max_tail_frames: base_tail,
    } = tail(&[])
    else {
        panic!("finite canonical room");
    };
    let OutputTail::Finite {
        max_tail_frames: moving_decay,
    } = tail(&[p::DECAY])
    else {
        panic!("finite modulated decay");
    };
    assert!(moving_decay > base_tail);
    assert_eq!(tail(&[p::FREEZE, p::CUT]), OutputTail::Unbounded);
    assert_eq!(
        room.read_parameter(&state, p::FREEZE).unwrap(),
        ParameterValue::Bool(false)
    );

    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Parameter(p::DRY_WET),
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let bank = |parameter: ParameterId| {
        DeviceModulation::new(
            vec![Modulator {
                id: source,
                color: ElementColor::default(),
                enabled: true,
                state: Settings {
                    rate: Rate::Hertz(12_000.0),
                    phase: 0.0,
                }
                .encode()
                .unwrap(),
            }],
            vec![DeviceModulationRoute {
                id: ModulationRouteId::new(next + 1).unwrap(),
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: REVERB_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(parameter),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(0.25).unwrap(),
                range: ModulationRange::default(),
                enabled: true,
            }],
        )
        .unwrap()
    };
    let dry: [[f32; 2]; FRAMES] = std::array::from_fn(|frame| {
        let signal = (frame as f32 * 0.27).sin() * 0.2;
        [signal, -0.75 * signal]
    });
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let project = editor.project();
    for id in [p::DECAY, p::DRY_WET, p::FREEZE, p::CUT] {
        assert!(crate::project_device_modulation_target_supported(
            project, device, id
        ));
    }
    let mut unsupported = Editor::new(project.clone(), 1).unwrap();
    unsupported
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank(p::PREDELAY),
        })
        .unwrap();
    assert!(crate::compile::project(unsupported.project(), &media).is_err());

    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank(p::DRY_WET),
        })
        .unwrap();
    let project = editor.project();
    assert_eq!(project.device(device).unwrap().state(), &state);
    let full_wet = room
        .patch_parameter(&state, p::DRY_WET, ParameterValue::Float(100.0))
        .unwrap();
    let mut room_only = dry;
    ReverbDefinition::prepare_owned_room(&full_wet, RATE)
        .unwrap()
        .create_runtime()
        .process_block(&mut room_only, &EMPTY_DEVICE_RESOURCES);
    assert!(room_only.iter().any(|&frame| frame != [0.0; 2]));
    let compare = |pcm: &[[f32; 2]; FRAMES]| {
        for (i, actual) in pcm.iter().enumerate() {
            let wave = (std::f32::consts::TAU * (i % 8) as f32 / 8.0).sin();
            let wet = 0.5 + 0.25 * wave;
            for (channel, &sample) in actual.iter().enumerate() {
                let expected = dry[i][channel] * (1.0 - wet) + room_only[i][channel] * wet;
                assert!(
                    (sample - expected).abs() < 1e-6,
                    "frame {i}: {sample} != {expected}"
                );
            }
        }
    };

    let graph = crate::compile::project(project, &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    let mut pcm = [[0.0; 2]; FRAMES];
    for block in pcm.chunks_mut(129) {
        renderer.render(block).unwrap();
    }
    compare(&pcm);

    if crate::audio_storage_constructor_layout_supported() {
        let registry = hex_devices::native_registry().unwrap();
        let effects = AudioCandidateEffects::new(&registry);
        let quote = effects.quote(project, 1).unwrap();
        let limits = quote.storage_reservation(FRAMES * 8).unwrap();
        let mut candidate = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
        candidate
            .copy_resident_source_on_control(asset, &audio)
            .unwrap();
        effects
            .prepare(
                &mut candidate,
                project,
                4,
                AudioSourceContext {
                    project_generation: 2,
                    editor_revision: 8,
                },
                None,
            )
            .unwrap();
        let owner = candidate.owner_on_control().unwrap();
        let summary = owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: quote.data_rows,
                handle_rows: quote.handle_rows,
                root_rows: 1,
                max_retained_bytes: limits.max_prepared_data_bytes,
                max_construction_peak_bytes: limits.max_construction_data_bytes,
            })
            .unwrap();
        assert_eq!(summary.data.unknown_storage, 0);
        owner
            .test_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        pcm.fill([0.0; 2]);
        for block in pcm.chunks_mut(43) {
            owner.test_render(block).unwrap();
        }
        compare(&pcm);
        candidate.cleanup_on_control().unwrap();
    }
}
