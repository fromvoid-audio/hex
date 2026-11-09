//! The same typed host gain route through ordinary and sealed effect graphs.
use super::*;
use crate::{Audio, AudioSourceContext, PlaybackAction, PlaybackCommand, Renderer};
use hex_devices::{
    modulation::{
        envelope_follower,
        lfo::{Rate, Settings, Waveform},
    },
    utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, UTILITY_TYPE_ID, parameter},
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    AssetId, AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor, ElementColor,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator, Project,
};
use std::collections::BTreeMap;

#[test]
fn host_output_modulation_uses_output_automation_and_preserves_bypass() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 269;
    const STEP: usize = 129;
    const DEPTH: f32 = -0.08;
    let dry = [0.125, -0.25];
    let audio = Audio::new(RATE, vec![dry; FRAMES]).unwrap();
    for provider_route in [false, true] {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "output trim".into(),
                path: "output-trim.wav".into(),
                sample_rate: RATE,
                frames: FRAMES as u64,
            })
            .unwrap()
        else {
            panic!("audio fixture");
        };
        let state = UTILITY_DEFINITION
            .default_state()
            .unwrap()
            .with_output_gain_millidb(1750)
            .unwrap();
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
        let gain = DeviceAutomationTarget::HostOutputGain;
        let change = editor
            .project()
            .tempo_map()
            .frame_to_beat(STEP as u64)
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target: gain,
                points: [(BeatTime::ZERO, 0.65), (change, 0.75)]
                    .map(|(beat, value)| AutomationPoint {
                        beat,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    })
                    .to_vec(),
            })
            .unwrap();
        let next = editor.project().next_identity();
        let source = ModulationSourceId::new(next).unwrap();
        let route = |index, target, amount| DeviceModulationRoute {
            id: ModulationRouteId::new(next + index).unwrap(),
            source,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type: UTILITY_TYPE_ID,
                target,
                vst3_class: None,
            },
            amount: ModulationAmount::new(amount).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        };
        let mut routes = vec![route(1, gain, DEPTH)];
        if provider_route {
            routes.push(route(
                2,
                DeviceAutomationTarget::Parameter(parameter::GAIN),
                0.25 / MAX_UTILITY_GAIN,
            ));
        }
        let bank = DeviceModulation::new(
            vec![Modulator {
                id: source,
                enabled: true,
                color: ElementColor::default(),
                state: Settings {
                    rate: Rate::Hertz(12_000.0),
                    waveform: Waveform::Square,
                    phase: 0.0,
                }
                .encode()
                .unwrap(),
            }],
            routes,
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceModulation {
                device,
                modulation: bank.clone(),
            })
            .unwrap();
        for bypassed in [false, true] {
            editor
                .execute(Command::SetDeviceBypass { device, bypassed })
                .unwrap();
            let project = editor.project();
            let saved = project.device(device).unwrap();
            assert_eq!(saved.state(), &state);
            assert_eq!(saved.modulation(), &bank);
            let compare = |pcm: &[[f32; 2]; FRAMES]| {
                for (index, frame) in pcm.iter().enumerate() {
                    let wave = if index % 4 < 2 { 1.0 } else { -1.0 };
                    let base = if index < STEP { 0.65_f32 } else { 0.75 };
                    let normalized = (f64::from(base) + f64::from(DEPTH) * wave) as f32;
                    let db = (84_000.0 * f64::from(normalized) - 60_000.0).round() as f32 / 1000.0;
                    let provider = if provider_route {
                        1.0 + 0.25 * wave as f32
                    } else {
                        1.0
                    };
                    let output = if bypassed {
                        1.0
                    } else {
                        provider * 10.0_f32.powf(db / 20.0)
                    };
                    for (sample, input) in frame.iter().zip(dry) {
                        let expected = input * output;
                        assert!(
                            (sample - expected).abs() < 1e-6,
                            "frame {index}: {sample} != {expected}"
                        );
                    }
                }
            };

            compare_native_and_sealed_pcm(project, asset, &audio, compare);
        }
    }
}

fn compare_native_and_sealed_pcm<const FRAMES: usize>(
    project: &Project,
    asset: AssetId,
    audio: &Audio,
    compare: impl Fn(&[[f32; 2]; FRAMES]),
) {
    let media = BTreeMap::from([(asset, audio.clone())]);
    let graph = crate::compile::project(project, &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    let mut pcm = [[0.0; 2]; FRAMES];
    for block in pcm.chunks_mut(137) {
        renderer.render(block).unwrap();
    }
    compare(&pcm);

    if !crate::audio_storage_constructor_layout_supported() {
        return;
    }
    let registry = hex_devices::native_registry().unwrap();
    let effects = AudioCandidateEffects::new(&registry);
    let quote = effects.quote(project, 1).unwrap();
    let limits = quote.storage_reservation(FRAMES * 8).unwrap();
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
    for block in pcm.chunks_mut(41) {
        owner.test_render(block).unwrap();
    }
    compare(&pcm);
    candidate.cleanup_on_control().unwrap();
}

#[test]
fn host_input_modulation_follows_the_unmodulated_matrix_and_shares_output_sources() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 269;
    const STEP: usize = 129;
    const INPUT_DEPTH: f32 = 0.07;
    const OUTPUT_DEPTH: f32 = -0.025;
    let signal: [[f32; 2]; FRAMES] = std::array::from_fn(|frame| {
        if frame % 3 == 0 {
            [0.1, -0.2]
        } else {
            [-0.2, 0.075]
        }
    });
    let audio = Audio::new(RATE, signal.to_vec()).unwrap();
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "input trim".into(),
            path: "input-trim.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::GAIN,
            ParameterValue::Float(2.0),
        )
        .unwrap()
        .with_input_gain_millidb(-5500)
        .unwrap()
        .with_output_gain_millidb(-1250)
        .unwrap();
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
    let gain = DeviceAutomationTarget::HostInputGain;
    let change = editor
        .project()
        .tempo_map()
        .frame_to_beat(STEP as u64)
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: gain,
            points: [(BeatTime::ZERO, 0.66), (change, 0.74)]
                .map(|(beat, value)| AutomationPoint {
                    beat,
                    value: AutomationValue::new(value).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                })
                .to_vec(),
        })
        .unwrap();
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let routes = [
        (gain, INPUT_DEPTH),
        (DeviceAutomationTarget::HostOutputGain, OUTPUT_DEPTH),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (target, depth))| DeviceModulationRoute {
        id: ModulationRouteId::new(next + index as u64 + 1).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Device {
            device_type: UTILITY_TYPE_ID,
            target,
            vst3_class: None,
        },
        amount: ModulationAmount::new(depth).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    })
    .collect();
    let bank = DeviceModulation::new(
        vec![Modulator {
            id: source,
            enabled: true,
            color: ElementColor::default(),
            state: envelope_follower::Settings {
                attack_seconds: 0.0,
                release_seconds: 0.0,
            }
            .encode()
            .unwrap(),
        }],
        routes,
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank,
        })
        .unwrap();
    for bypassed in [false, true] {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let project = editor.project();
        assert_eq!(project.device(device).unwrap().state(), &state);
        let compare = |pcm: &[[f32; 2]; FRAMES]| {
            let linear = |normalized: f32| {
                let db = (84_000.0 * f64::from(normalized) - 60_000.0).round() as f32 / 1000.0;
                10.0_f32.powf(db / 20.0)
            };
            for (index, (frame, dry)) in pcm.iter().zip(signal).enumerate() {
                let base = if index < STEP { 0.66 } else { 0.74 };
                let envelope =
                    f64::from((dry[0].abs().max(dry[1].abs()) * linear(base) * 2.0).min(1.0));
                let input = linear((f64::from(base) + f64::from(INPUT_DEPTH) * envelope) as f32);
                let output_base = (60_000.0_f64 - 1250.0) / 84_000.0;
                let output = linear(
                    (f64::from(output_base as f32) + f64::from(OUTPUT_DEPTH) * envelope) as f32,
                );
                for (sample, dry) in frame.iter().zip(dry) {
                    let expected = if bypassed {
                        dry
                    } else {
                        (dry * input * 2.0) * output
                    };
                    assert!(
                        (sample - expected).abs() < 1e-6,
                        "frame {index}: {sample} != {expected}"
                    );
                }
            }
        };
        compare_native_and_sealed_pcm(project, asset, &audio, compare);
    }
}
