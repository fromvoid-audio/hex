//! Same provider value law across ordinary and sealed effect preparation.
use super::*;
use crate::{Audio, AudioSourceContext, PlaybackAction, PlaybackCommand, Renderer};
use hex_devices::{
    auto_pan::{AUTO_PAN_DEFINITION, AUTO_PAN_TYPE_ID, AutoPanDefinition, parameter},
    modulation::lfo::{Rate, Settings},
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

const IDS: [ParameterId; 4] = [
    parameter::AMOUNT,
    parameter::RATE,
    parameter::RIGHT_PHASE,
    parameter::WAVEFORM,
];
const RATE: u32 = 8_000;
const FRAMES: usize = 97;

struct ExpectedValues;
impl NativeParameterAutomation for ExpectedValues {
    fn parameter_ids(&self) -> &[ParameterId] {
        &IDS
    }

    fn value_at(&self, id: ParameterId, frame: usize) -> Option<ParameterValue> {
        let wave = [0.0_f32, 1.0, 0.0, -1.0][frame % 4];
        Some(match id {
            parameter::AMOUNT => ParameterValue::Float(0.5 + 0.25 * wave),
            parameter::RATE => ParameterValue::Float(400.0_f32.powf(0.125 * wave)),
            parameter::RIGHT_PHASE => ParameterValue::Float(180.0 + 90.0 * wave),
            parameter::WAVEFORM => ParameterValue::Choice(if wave > 0.0 { 2 } else { 0 }),
            _ => return None,
        })
    }
}

#[test]
fn auto_pan_modulation_matches_independent_sample_frames_in_both_compilers() {
    let state = AUTO_PAN_DEFINITION.default_state().unwrap();
    let factory = AutoPanDefinition::prepare_owned(&state, RATE, &IDS).unwrap();
    for id in IDS {
        let semantics = factory.parameter_modulation_semantics(id).unwrap();
        assert_eq!(semantics.modulation, ParameterModulation::SampleRate);
        assert_eq!(
            AUTO_PAN_DEFINITION
                .parameter_semantics_for_state(&state, id)
                .unwrap(),
            Some(semantics),
        );
    }
    assert!(
        factory
            .parameter_modulation_semantics(ParameterId::from_static(5))
            .is_none()
    );

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "auto pan".into(),
            path: "auto-pan.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("effect fixture");
    };
    let next = editor.project().next_identity();
    let source = |index| ModulationSourceId::new(next + index).unwrap();
    let bank = DeviceModulation::new(
        [0.0, 0.5]
            .into_iter()
            .enumerate()
            .map(|(index, phase)| Modulator {
                color: ElementColor::default(),
                id: source(index as u64),
                enabled: true,
                state: Settings {
                    rate: Rate::Hertz(2_000.0),
                    phase,
                }
                .encode()
                .unwrap(),
            })
            .collect(),
        IDS.into_iter()
            .zip([0.25, 0.125, 0.25, 0.5])
            .enumerate()
            .map(|(index, (parameter, amount))| DeviceModulationRoute {
                id: ModulationRouteId::new(next + index as u64 + 2).unwrap(),
                source: source(index as u64 % 2),
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: AUTO_PAN_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(parameter),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(if index % 2 == 0 { amount } else { -amount })
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
    let project = editor.project();
    for id in IDS {
        assert!(crate::project_device_modulation_target_supported(
            project, device, id
        ));
    }
    assert!(!crate::project_device_modulation_target_supported(
        project,
        device,
        ParameterId::from_static(5)
    ));

    let mut expected = [[0.125, -0.25]; FRAMES];
    let audio = Audio::new(RATE, expected.to_vec()).unwrap();
    factory
        .create_runtime()
        .process_block_with_automation(
            &mut expected,
            None,
            &ExpectedValues,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    let compare = |output: [[f32; 2]; FRAMES]| {
        for (index, (actual, reference)) in output.into_iter().zip(expected).enumerate() {
            for (actual, reference) in actual.into_iter().zip(reference) {
                assert!(
                    (actual - reference).abs() < 1e-6,
                    "frame {index}: {actual} != {reference}"
                );
            }
        }
    };
    let media = BTreeMap::from([(asset, audio.clone())]);
    let graph = crate::compile::project(project, &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    let mut output = [[0.0; 2]; FRAMES];
    renderer.render(&mut output).unwrap();
    compare(output);

    let mut slots = bank.sources().to_vec();
    slots[0].color = ElementColor::Cyan;
    slots.reverse();
    let reordered = DeviceModulation::new(slots, bank.routes().to_vec()).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: reordered.clone(),
        })
        .unwrap();
    let project = editor.project();
    let graph = crate::compile::project(project, &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    renderer.render(&mut output).unwrap();
    compare(output);

    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("sealed modulation playback requires the supported constructor layout");
        return;
    }
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
    for block in output.chunks_mut(11) {
        owner.test_render(block).unwrap();
    }
    compare(output);
    candidate.cleanup_on_control().unwrap();

    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    assert_eq!(
        editor.project().device(device).unwrap().modulation(),
        &reordered
    );
    let graph = crate::compile::project(editor.project(), &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    renderer.render(&mut output).unwrap();
    assert_eq!(output, [[0.125, -0.25]; FRAMES]);
}
