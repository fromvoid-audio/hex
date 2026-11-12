//! Compressor consumes the same independently authored values in the two graphs.
use super::host_output_modulation_tests::compare_native_and_sealed_pcm;
use crate::Audio;
use hex_devices::{
    compressor::{COMPRESSOR_DEFINITION, COMPRESSOR_TYPE_ID, CompressorDefinition, parameter as p},
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

const RATE: u32 = 48_000;
const FRAMES: usize = 473;
const DEPTH: f32 = 0.08;
const IDS: [ParameterId; 8] = [
    p::THRESHOLD,
    p::RATIO,
    p::ATTACK,
    p::RELEASE,
    p::KNEE,
    p::MAKEUP,
    p::DRY_WET,
    p::DETECTOR,
];

struct Expected;
impl NativeParameterAutomation for Expected {
    fn parameter_ids(&self) -> &[ParameterId] {
        &IDS
    }

    fn value_at(&self, id: ParameterId, frame: usize) -> Option<ParameterValue> {
        let positive = frame % 4 < 2;
        let shift = if positive {
            f64::from(DEPTH)
        } else {
            -f64::from(DEPTH)
        };
        let value = match id {
            p::THRESHOLD => -18.0 + 60.0 * shift,
            p::RATIO => 4.0 - 19.0 * shift,
            p::ATTACK => 10.0 * 1000.0_f64.powf(shift),
            p::RELEASE => 100.0 * 200.0_f64.powf(shift),
            p::KNEE => 6.0 + 12.0 * shift,
            p::MAKEUP => 24.0 * shift.max(0.0),
            p::DRY_WET => 1.0 + shift.min(0.0),
            p::DETECTOR => return Some(ParameterValue::Choice(u32::from(positive))),
            _ => return None,
        };
        Some(ParameterValue::Float(value as f32))
    }
}

#[test]
fn compressor_modulation_delivers_all_eight_controls_in_ordinary_and_sealed_playback() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "compressor".into(),
            path: "compressor-modulation.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let state = COMPRESSOR_DEFINITION.default_state().unwrap();
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
    let factory = CompressorDefinition::prepare_owned(&state, RATE, &IDS).unwrap();
    for id in IDS {
        let semantics = factory.parameter_modulation_semantics(id).unwrap();
        assert_eq!(semantics.modulation, ParameterModulation::SampleRate);
        assert_eq!(
            COMPRESSOR_DEFINITION
                .parameter_semantics_for_state(&state, id)
                .unwrap(),
            Some(semantics),
        );
        assert!(crate::project_device_modulation_target_supported(
            editor.project(),
            device,
            id
        ));
    }
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
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
        IDS.into_iter()
            .enumerate()
            .map(|(index, target)| DeviceModulationRoute {
                id: ModulationRouteId::new(next + index as u64 + 1).unwrap(),
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: COMPRESSOR_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(target),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(match target {
                    p::DETECTOR => 0.75,
                    p::RATIO => -DEPTH,
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

    let dry: [[f32; 2]; FRAMES] = std::array::from_fn(|index| {
        let level = if (51..333).contains(&index) {
            0.65
        } else {
            0.02
        };
        [level, -level * 0.4]
    });
    let mut expected = dry;
    factory
        .create_runtime()
        .process_block_with_automation(&mut expected, None, &Expected, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let mut stock = dry;
    CompressorDefinition::prepare_owned(&state, RATE, &[])
        .unwrap()
        .create_runtime()
        .process_block(&mut stock, &EMPTY_DEVICE_RESOURCES);
    assert_ne!(expected, stock);
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    for bypassed in [false, true] {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let project = editor.project();
        let authored = project.device(device).unwrap();
        assert_eq!(authored.state(), &state);
        assert_eq!(authored.modulation(), &bank);
        let reference = if bypassed { &dry } else { &expected };
        compare_native_and_sealed_pcm::<FRAMES>(project, asset, &audio, |actual| {
            for (index, (actual, expected)) in actual.iter().zip(reference).enumerate() {
                for (sample, expected) in actual.iter().zip(expected) {
                    assert!(
                        (sample - expected).abs() < 1e-6,
                        "frame {index}: {sample} != {expected}"
                    );
                }
            }
        });
    }
}
