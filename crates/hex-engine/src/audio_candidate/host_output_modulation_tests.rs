//! The same typed host gain route through ordinary and sealed effect graphs.
use super::*;
use crate::{Audio, AudioSourceContext, PlaybackAction, PlaybackCommand, Renderer};
use hex_devices::{
    modulation::lfo::{Rate, Settings, Waveform},
    utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, UTILITY_TYPE_ID, parameter},
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
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
    let registry = hex_devices::native_registry().unwrap();
    let effects = AudioCandidateEffects::new(&registry);
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
        let media = BTreeMap::from([(asset, audio.clone())]);

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

            let graph = crate::compile::project(project, &media).unwrap();
            let mut renderer = Renderer::new(&graph);
            renderer.play();
            let mut pcm = [[0.0; 2]; FRAMES];
            for block in pcm.chunks_mut(137) {
                renderer.render(block).unwrap();
            }
            compare(&pcm);

            if !crate::audio_storage_constructor_layout_supported() {
                continue;
            }
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
    }
}
