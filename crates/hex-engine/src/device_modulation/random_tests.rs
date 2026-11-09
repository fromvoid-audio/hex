use crate::{Audio, Renderer};
use hex_devices::{
    modulation::random_hold::Settings,
    utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, UTILITY_TYPE_ID, parameter},
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator, Project,
};
use std::collections::BTreeMap;

#[test]
fn saved_random_modulation_drives_utility_and_restarts_on_seek() {
    const FRAMES: usize = 16;
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "random".into(),
            path: "random.wav".into(),
            sample_rate: 8_000,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let state = UTILITY_DEFINITION.default_state().unwrap();
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
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let bank = DeviceModulation::new(
        vec![Modulator {
            id: source,
            enabled: true,
            state: Settings {
                seed: 0,
                rate_hz: 2_000.0,
            }
            .encode()
            .unwrap(),
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(next + 1).unwrap(),
            source,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type: UTILITY_TYPE_ID,
                target: DeviceAutomationTarget::Parameter(parameter::GAIN),
                vst3_class: None,
            },
            amount: ModulationAmount::new(1.0 / MAX_UTILITY_GAIN).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        }],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank,
        })
        .unwrap();
    assert_eq!(editor.project().device(device).unwrap().state(), &state);

    let input = [[0.125, -0.25]; FRAMES];
    let media = BTreeMap::from([(asset, Audio::new(8_000, input.to_vec()).unwrap())]);
    let graph = crate::compile::project(editor.project(), &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    let mut output = [[0.0; 2]; FRAMES];
    for block in output.chunks_mut(3) {
        renderer.render(block).unwrap();
    }
    let expected = [0x3f44_4150, 0xbe0c_3b10, 0xbf72_7746, 0x3f71_1770];
    for (index, frame) in output.iter().enumerate() {
        let gain = 1.0 + f32::from_bits(expected[index / 4]);
        assert!((frame[0] - 0.125 * gain).abs() < 1e-6, "{frame:?}");
        assert!((frame[1] + 0.25 * gain).abs() < 1e-6, "{frame:?}");
    }
    renderer.seek(0);
    let mut restarted = [[0.0; 2]; FRAMES];
    renderer.render(&mut restarted).unwrap();
    assert_eq!(restarted, output);

    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::default(),
        })
        .unwrap();
    let graph = crate::compile::project(editor.project(), &media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    renderer.render(&mut output).unwrap();
    assert_eq!(output, input);
}
