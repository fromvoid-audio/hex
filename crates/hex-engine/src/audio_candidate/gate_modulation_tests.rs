//! Closed Gate storage and sample-rate delivery through the real effect graph.
use super::*;
use crate::{Audio, AudioSourceContext, PlaybackAction, PlaybackCommand, Renderer};
use hex_devices::{
    gate::{GATE_DEFINITION, GATE_TYPE_ID, GateDefinition, parameter},
    modulation::lfo::{Rate, Settings, Waveform},
};
use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeParameterAutomation, PreparedNativeProcessor};
use hex_lattice_contract::{
    NativeDeviceMetadata, ParameterId, ParameterModulation, ParameterValue,
};
use hex_project::{
    Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor,
    ElementColor, ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator, Project,
};
use std::collections::BTreeMap;

const RATE: u32 = 8_000;
const FRAMES: usize = 617;
const DEPTH: f32 = 0.008;
const IDS: [ParameterId; 4] = [
    parameter::FLOOR,
    parameter::ATTACK,
    parameter::HOLD,
    parameter::RELEASE,
];

struct Expected<'a>(&'a [ParameterId]);
impl NativeParameterAutomation for Expected<'_> {
    fn parameter_ids(&self) -> &[ParameterId] {
        self.0
    }

    fn value_at(&self, id: ParameterId, frame: usize) -> Option<ParameterValue> {
        let wave = if frame % 4 < 2 { 1.0 } else { -1.0 };
        let delta = f64::from(DEPTH) * wave;
        let threshold_mapped = self.0.contains(&parameter::THRESHOLD);
        let return_mapped = self.0.contains(&parameter::RETURN);
        let threshold = -40.0
            - if threshold_mapped {
                72.0 * f64::from(0.3_f32) * wave
            } else {
                0.0
            };
        let return_level = -45.0
            + if return_mapped {
                84.0 * f64::from(0.3_f32) * wave
            } else {
                0.0
            };
        let value = match id {
            parameter::THRESHOLD if return_mapped => threshold,
            parameter::THRESHOLD => threshold.max(return_level),
            parameter::RETURN => return_level.min(threshold),
            parameter::FLOOR => -60.0 + 80.0 * delta,
            parameter::ATTACK => 1000.0_f64.powf(delta),
            parameter::HOLD => 20.0 + 2000.0 * delta,
            parameter::RELEASE => 100.0 * 2000.0_f64.powf(delta),
            _ => return None,
        };
        Some(ParameterValue::Float(value as f32))
    }
}

#[test]
fn gate_modulation_uses_the_same_frame_values_in_ordinary_and_sealed_playback() {
    check_gate_routes(&IDS);
}

#[test]
fn gate_modulation_keeps_the_detector_ordered_with_independent_and_opposing_routes() {
    for ids in [
        &[parameter::THRESHOLD][..],
        &[parameter::RETURN][..],
        &[parameter::THRESHOLD, parameter::RETURN][..],
    ] {
        check_gate_routes(ids);
    }
}

fn check_gate_routes(ids: &[ParameterId]) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "gate".into(),
            path: "gate.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let state = GATE_DEFINITION
        .patch_parameter(
            &GATE_DEFINITION.default_state().unwrap(),
            parameter::HOLD,
            ParameterValue::Float(20.0),
        )
        .unwrap();
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

    let factory = GateDefinition::prepare_owned(&state, RATE, ids).unwrap();
    for &id in ids {
        let semantics = factory.parameter_modulation_semantics(id).unwrap();
        assert_eq!(semantics.modulation, ParameterModulation::SampleRate);
        assert_eq!(
            GATE_DEFINITION
                .parameter_semantics_for_state(&state, id)
                .unwrap(),
            Some(semantics)
        );
        assert!(crate::project_device_modulation_target_supported(
            editor.project(),
            device,
            id
        ));
    }
    assert!(!crate::project_device_modulation_target_supported(
        editor.project(),
        device,
        ParameterId::from_static(7)
    ));
    assert!(GateDefinition::prepare_owned(&state, RATE, &[ParameterId::from_static(7)]).is_err());
    assert!(GateDefinition::prepare_owned(&state, RATE - 1, ids).is_err());
    let malformed =
        hex_lattice_contract::NativeDeviceState::new(GATE_TYPE_ID, state.state_version(), vec![0])
            .unwrap();
    assert!(GateDefinition::prepare_owned(&malformed, RATE, ids).is_err());

    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let bank = DeviceModulation::new(
        vec![Modulator {
            id: source,
            color: ElementColor::default(),
            enabled: true,
            state: Settings {
                rate: Rate::Hertz(2_000.0),
                waveform: Waveform::Square,
                phase: 0.0,
            }
            .encode()
            .unwrap(),
        }],
        ids.iter()
            .copied()
            .enumerate()
            .map(|(offset, id)| DeviceModulationRoute {
                id: ModulationRouteId::new(next + offset as u64 + 1).unwrap(),
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: GATE_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(id),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(match id {
                    parameter::THRESHOLD => -0.3,
                    parameter::RETURN => 0.3,
                    _ => DEPTH,
                })
                .unwrap(),
                range: ModulationRange::default(),
                enabled: true,
            })
            .collect(),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank.clone(),
        })
        .unwrap();

    let dry = std::array::from_fn(|frame| {
        let input = if (65..270).contains(&frame) {
            0.15
        } else {
            0.0025
        };
        [input, -input * 0.75]
    });
    let mut expected: [[f32; 2]; FRAMES] = dry;
    factory
        .create_runtime()
        .process_block_with_automation(&mut expected, None, &Expected(ids), &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    assert!(expected[..65].iter().zip(&dry).any(|(a, b)| a != b));
    assert!(expected[65..270].contains(&dry[80]));
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let registry = hex_devices::native_registry().unwrap();
    let effects = AudioCandidateEffects::new(&registry);
    for bypassed in [false, true] {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let project = editor.project();
        let authored = project.device(device).unwrap();
        assert_eq!(authored.state(), &state);
        assert_eq!(authored.modulation(), &bank);
        let reference = if bypassed { &dry } else { &expected };
        let compare = |actual: &[[f32; 2]; FRAMES]| {
            for (index, (actual, expected)) in actual.iter().zip(reference).enumerate() {
                for (&actual, &expected) in actual.iter().zip(expected) {
                    assert!(
                        (actual - expected).abs() < 1e-6,
                        "frame {index}: {actual} != {expected}"
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
                    project_generation: 1,
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
        assert!(summary.owned_runtime_bytes > 0);
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
