//! Keep Overdrive's native coefficient smoothing and latency under modulation.
use super::*;
use crate::{Audio, AudioSourceContext, PlaybackAction, PlaybackCommand, Renderer};
use hex_devices::{
    modulation::lfo::{Rate, Settings},
    overdrive::{OVERDRIVE_DEFINITION, OVERDRIVE_TYPE_ID, parameter},
};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition,
    NativeParameterAutomation, PreparedProcessorFactory, ProcessorChainOwner,
    ProcessorInstanceIdentity,
};
use hex_lattice_contract::{
    NativeDeviceMetadata, ParameterId, ParameterModulation, ParameterValue,
};
use hex_project::{
    Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator, Project,
};
use std::collections::BTreeMap;

const RATE: u32 = 8_000;
const FRAMES: usize = 97;
const LATENCY: usize = 11;
const IDS: [ParameterId; 6] = [
    parameter::FREQUENCY,
    parameter::BANDWIDTH,
    parameter::DRIVE,
    parameter::TONE,
    parameter::DYNAMICS,
    parameter::DRY_WET,
];

struct IndependentValues;
impl NativeParameterAutomation for IndependentValues {
    fn parameter_ids(&self) -> &[ParameterId] {
        &IDS
    }
    fn value_at(&self, id: ParameterId, frame: usize) -> Option<ParameterValue> {
        let wave = [0.0_f32, 1.0, 0.0, -1.0][frame % 4];
        Some(ParameterValue::Float(match id {
            parameter::FREQUENCY => 1250.0 * 400.0_f32.powf(0.0625 * wave),
            parameter::BANDWIDTH => 6.5 + 8.5 * 0.0625 * wave,
            parameter::DRIVE => 0.5 + 0.125 * wave,
            parameter::TONE => 0.5 - 0.125 * wave,
            parameter::DYNAMICS => 0.5 + 0.25 * wave,
            parameter::DRY_WET => 0.5 + 0.375 * wave,
            _ => return None,
        }))
    }
}

#[test]
fn overdrive_modulation_matches_independent_sample_frames_in_both_compilers() {
    let state = OVERDRIVE_DEFINITION.default_state().unwrap();
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "overdrive".into(),
            path: "overdrive.wav".into(),
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
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("effect fixture");
    };

    let tuning = hex_lattice::TuningTable::default();
    let prepared = OVERDRIVE_DEFINITION
        .prepare_automated_runtime_instance(
            ProcessorInstanceIdentity::new(
                ProcessorChainOwner::Track(track.value()),
                device.value(),
            )
            .unwrap(),
            &state,
            &DevicePreparationContext {
                sample_rate: RATE,
                tuning: &tuning,
                resources: &EMPTY_DEVICE_RESOURCES,
            },
            &IDS,
        )
        .unwrap();
    let Some(PreparedProcessorFactory::Legacy(factory)) = prepared.processor else {
        panic!("overdrive factory");
    };
    for id in IDS {
        let semantics = factory.parameter_modulation_semantics(id).unwrap();
        assert_eq!(semantics.modulation, ParameterModulation::SampleRate);
        assert_eq!(
            OVERDRIVE_DEFINITION
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
    let unknown = ParameterId::from_static(7);
    assert!(factory.parameter_modulation_semantics(unknown).is_none());
    assert!(!crate::project_device_modulation_target_supported(
        editor.project(),
        device,
        unknown
    ));

    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let bank = DeviceModulation::new(
        vec![Modulator {
            color: Default::default(),
            id: source,
            enabled: true,
            state: Settings {
                rate: Rate::Hertz(2_000.0),
                phase: 0.0,
            }
            .encode()
            .unwrap(),
        }],
        IDS.into_iter()
            .zip([0.0625, 0.0625, 0.125, -0.125, 0.25, 0.375])
            .enumerate()
            .map(|(index, (parameter, amount))| DeviceModulationRoute {
                id: ModulationRouteId::new(next + index as u64 + 1).unwrap(),
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: OVERDRIVE_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(parameter),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(amount).unwrap(),
                range: ModulationRange::default(),
                enabled: true,
            })
            .collect(),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank,
        })
        .unwrap();
    let project = editor.project();
    assert_eq!(project.device(device).unwrap().state(), &state);

    let mut expected = std::array::from_fn::<_, FRAMES, _>(|i| {
        let value = (i as f32 * 0.125).sin() * 0.3;
        [value, -value * 0.8]
    });
    let audio = Audio::new(RATE, expected.to_vec()).unwrap();
    factory
        .create_runtime()
        .process_block_with_automation(
            &mut expected,
            None,
            &IndependentValues,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    // The track fader opens once the compensated content origin arrives.
    // The underlying oversampler has a response before its group delay.
    expected[..LATENCY].fill([0.0; 2]);
    let compare = |output: &[[f32; 2]; FRAMES]| {
        for (index, (actual, reference)) in output.iter().zip(expected).enumerate() {
            for (actual, reference) in actual.iter().zip(reference) {
                assert!(
                    (actual - reference).abs() < 1e-6,
                    "frame {index}: {actual} != {reference}"
                );
            }
        }
    };
    let media = BTreeMap::from([(asset, audio.clone())]);
    let graph = crate::compile::project(project, &media).unwrap();
    assert_eq!(graph.main_latency_frames(), LATENCY as u32);
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    let mut output = [[0.0; 2]; FRAMES];
    for block in output.chunks_mut(13) {
        renderer.render(block).unwrap();
    }
    compare(&output);

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
    for block in output.chunks_mut(7) {
        owner.test_render(block).unwrap();
    }
    compare(&output);
    candidate.cleanup_on_control().unwrap();
}
