//! Rattle's smoothed drive follows sample-rate control through both graph owners.
use super::host_output_modulation_tests::compare_native_and_sealed_pcm;
use crate::Audio;
use hex_devices::{
    modulation::lfo::{Rate, Settings, Waveform},
    rattle::{
        LATENCY_FRAMES, RATTLE_DEFINITION, RATTLE_TYPE_ID, RattleDefinition, parameter::DRIVE,
    },
};
use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeParameterAutomation, PreparedNativeProcessor};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use hex_project::{
    Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator, Project,
};

const RATE: u32 = 8_000;
const FRAMES: usize = 273;
const LATENCY: usize = LATENCY_FRAMES as usize;

struct DriveFrames;
impl NativeParameterAutomation for DriveFrames {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[DRIVE]
    }

    fn value_at(&self, id: ParameterId, frame: usize) -> Option<ParameterValue> {
        (id == DRIVE).then(|| ParameterValue::Float(if frame % 16 < 8 { 0.65 } else { 0.15 }))
    }
}

#[test]
fn rattle_modulation_preserves_drive_smoothing_and_aligned_bypass_in_both_graphs() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "rattle".into(),
            path: "rattle.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let state = RATTLE_DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("rattle fixture");
    };
    assert!(crate::project_device_modulation_target_supported(
        editor.project(),
        device,
        DRIVE
    ));
    assert!(!crate::project_device_host_gain_modulation_supported(
        editor.project(),
        device
    ));
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let bank = DeviceModulation::new(
        vec![Modulator {
            id: source,
            enabled: true,
            color: Default::default(),
            state: Settings {
                rate: Rate::Hertz(500.0),
                waveform: Waveform::Square,
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
                device_type: RATTLE_TYPE_ID,
                target: DeviceAutomationTarget::Parameter(DRIVE),
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.25).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        }],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank.clone(),
        })
        .unwrap();

    let dry: [[f32; 2]; FRAMES] = std::array::from_fn(|frame| {
        let signal = (frame as f32 * 0.125).sin() * 0.3;
        [signal, -signal * 0.8]
    });
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    let factory = RattleDefinition::prepare_owned(&state, RATE, &[DRIVE]).unwrap();
    assert_eq!(factory.algorithmic_latency_frames(), Some(LATENCY_FRAMES));
    assert_eq!(
        factory.parameter_modulation_semantics(DRIVE),
        RATTLE_DEFINITION
            .parameter_semantics_for_state(&state, DRIVE)
            .unwrap()
    );
    let mut wet = dry;
    factory
        .create_runtime()
        .process_block_with_automation(&mut wet, None, &DriveFrames, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let mut stock = dry;
    factory
        .create_runtime()
        .process_block(&mut stock, &EMPTY_DEVICE_RESOURCES);
    assert_ne!(wet[LATENCY..], stock[LATENCY..]);
    // The track is silent until its compensated content origin.
    wet[..LATENCY].fill([0.0; 2]);
    let mut aligned_dry = [[0.0; 2]; FRAMES];
    aligned_dry[LATENCY..].copy_from_slice(&dry[..FRAMES - LATENCY]);

    for (bypassed, expected) in [(false, wet), (true, aligned_dry)] {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let project = editor.project();
        assert_eq!(project.device(device).unwrap().state(), &state);
        assert_eq!(project.device(device).unwrap().modulation(), &bank);
        compare_native_and_sealed_pcm::<FRAMES>(project, asset, &audio, |actual| {
            for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                assert!((actual[0] - expected[0]).abs() < 1e-6, "left frame {frame}");
                assert!(
                    (actual[1] - expected[1]).abs() < 1e-6,
                    "right frame {frame}"
                );
            }
        });
    }
}
