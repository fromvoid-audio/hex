//! Validate the exact prepared and paired runtime custody of closed EQ8.
use super::host_output_modulation_tests::compare_native_and_sealed_pcm;
use crate::Audio;
use hex_devices::eq8::{EQ8_DEFINITION, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, Editor, Project,
};
use std::cell::Cell;

#[test]
fn sealed_eq8_matches_ordinary_pcm_during_a_mode_fade_and_automated_gain() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 557;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "eq eight".into(),
            path: "eq-eight.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let mut state = EQ8_DEFINITION.default_state().unwrap();
    for (id, value) in [
        (parameter::enabled(0, 0), ParameterValue::Bool(true)),
        (parameter::gain(0, 0), ParameterValue::Float(8.0)),
        (parameter::enabled(1, 0), ParameterValue::Bool(true)),
        (parameter::gain(1, 0), ParameterValue::Float(-12.0)),
    ] {
        state = EQ8_DEFINITION.patch_parameter(&state, id, value).unwrap();
    }
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("eq8 fixture");
    };
    let change = editor.project().tempo_map().frame_to_beat(101).unwrap();
    for (parameter, start, end) in [
        (parameter::MODE, 0.0, 0.5),
        (parameter::OUTPUT_GAIN, 0.25, 0.75),
    ] {
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target: DeviceAutomationTarget::Parameter(parameter),
                points: [(BeatTime::ZERO, start), (change, end)]
                    .map(|(beat, value)| AutomationPoint {
                        beat,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    })
                    .to_vec(),
            })
            .unwrap();
    }

    let dry: [[f32; 2]; FRAMES] = std::array::from_fn(|frame| {
        let signal = (frame as f32 * 0.08).sin() * 0.3;
        [signal, -signal * 0.7]
    });
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    let reference = Cell::new(None);
    compare_native_and_sealed_pcm::<FRAMES>(editor.project(), asset, &audio, |pcm| {
        if let Some(expected) = reference.get() {
            assert_eq!(*pcm, expected);
        } else {
            assert!(pcm.iter().flatten().all(|sample| sample.is_finite()));
            assert_ne!(*pcm, dry);
            reference.set(Some(*pcm));
        }
    });
}

#[test]
fn eq8_gain_modulation_matches_the_sample_targets_and_provider_smoother() {
    use hex_devices::eq8::EQ8_TYPE_ID;
    use hex_devices::modulation::lfo::{Rate, Settings, Waveform};
    use hex_project::{
        DeviceModulation, DeviceModulationRoute, ModulationAmount, ModulationDestination,
        ModulationOutputId, ModulationRange, ModulationRouteId, ModulationSourceId, Modulator,
    };
    const RATE: u32 = 48_000;
    const FRAMES: usize = 293;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "eq eight".into(),
            path: "eq-eight.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    let state = EQ8_DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("eq8 fixture")
    };
    assert!(crate::project_device_modulation_target_supported(
        editor.project(),
        device,
        parameter::OUTPUT_GAIN
    ));
    assert!(!crate::project_device_modulation_target_supported(
        editor.project(),
        device,
        parameter::MODE
    ));
    let identity = editor.project().next_identity();
    let source = ModulationSourceId::new(identity).unwrap();
    let modulation = DeviceModulation::new(
        vec![Modulator {
            id: source,
            enabled: true,
            color: Default::default(),
            state: Settings {
                rate: Rate::Hertz(12_000.0),
                waveform: Waveform::Square,
                phase: 0.0,
            }
            .encode()
            .unwrap(),
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(identity + 1).unwrap(),
            source,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type: EQ8_TYPE_ID,
                target: DeviceAutomationTarget::Parameter(parameter::OUTPUT_GAIN),
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.03125).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        }],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();

    let dry = [0.25, -0.125];
    let audio = Audio::new(RATE, vec![dry; FRAMES]).unwrap();
    let project = editor.project();
    assert_eq!(project.device(device).unwrap().state(), &state);
    compare_native_and_sealed_pcm::<FRAMES>(project, asset, &audio, |pcm| {
        let mut gain = 1.0_f64;
        for (index, frame) in pcm.iter().enumerate() {
            // The target alternates after every two samples, restarting the
            // 256-frame smoother from its current gain on each change.
            let target = if index % 4 < 2 { 1.5 } else { 0.5 };
            gain += (target - gain) / if index % 2 == 0 { 256.0 } else { 255.0 };
            assert_eq!(*frame, dry.map(|sample| (f64::from(sample) * gain) as f32));
        }
    });
}

#[test]
fn eq8_band_modulation_matches_independent_per_frame_filter_targets() {
    use hex_devices::eq8::{EQ8_TYPE_ID, Eq8Definition};
    use hex_devices::modulation::lfo::{Rate, Settings, Waveform};
    use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeParameterAutomation, PreparedNativeProcessor};
    use hex_lattice_contract::ParameterId;
    use hex_project::{
        DeviceModulation, DeviceModulationRoute, ModulationAmount, ModulationDestination,
        ModulationOutputId, ModulationRange, ModulationRouteId, ModulationSourceId, Modulator,
    };
    const RATE: u32 = 48_000;
    const FRAMES: usize = 293;
    const IDS: [ParameterId; 3] = [
        parameter::frequency(0, 0),
        parameter::gain(0, 0),
        parameter::q(0, 0),
    ];

    struct BandFrames;
    impl NativeParameterAutomation for BandFrames {
        fn parameter_ids(&self) -> &[ParameterId] {
            &IDS
        }

        fn value_at(&self, id: ParameterId, frame: usize) -> Option<ParameterValue> {
            let depth = if frame % 4 < 2 { 0.0625 } else { -0.0625 };
            let value = match IDS.iter().position(|&parameter| parameter == id)? {
                0 => 1_000.0 * 1_000.0_f64.powf(depth),
                1 => 48.0 * depth,
                _ => 180.0_f64.powf(depth),
            };
            Some(ParameterValue::Float(value as f32))
        }
    }

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "eq eight".into(),
            path: "eq-eight-band.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    let mut state = EQ8_DEFINITION.default_state().unwrap();
    for (id, value) in [
        (parameter::enabled(0, 0), ParameterValue::Bool(true)),
        (IDS[0], ParameterValue::Float(1_000.0)),
        (IDS[1], ParameterValue::Float(0.0)),
        (IDS[2], ParameterValue::Float(1.0)),
        (parameter::enabled(0, 1), ParameterValue::Bool(true)),
        (parameter::gain(0, 1), ParameterValue::Float(-8.0)),
    ] {
        state = EQ8_DEFINITION.patch_parameter(&state, id, value).unwrap();
    }
    let cut = EQ8_DEFINITION
        .patch_parameter(&state, parameter::kind(0, 0), ParameterValue::Choice(3))
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: cut,
        })
        .unwrap()
    else {
        panic!("eq8 fixture")
    };
    assert!(!crate::project_device_modulation_target_supported(
        editor.project(),
        device,
        IDS[1]
    ));
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state.clone(),
        })
        .unwrap();
    for bank in 0..5 {
        for id in [
            parameter::frequency(bank, 0),
            parameter::gain(bank, 0),
            parameter::q(bank, 0),
        ] {
            assert!(crate::project_device_modulation_target_supported(
                editor.project(),
                device,
                id
            ));
        }
    }
    let identity = editor.project().next_identity();
    let source = ModulationSourceId::new(identity).unwrap();
    let modulation = DeviceModulation::new(
        vec![Modulator {
            id: source,
            enabled: true,
            color: Default::default(),
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
            .map(|(index, parameter)| DeviceModulationRoute {
                id: ModulationRouteId::new(identity + 1 + index as u64).unwrap(),
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: EQ8_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(parameter),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(0.0625).unwrap(),
                range: ModulationRange::default(),
                enabled: true,
            })
            .collect(),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();

    let dry: [[f32; 2]; FRAMES] = std::array::from_fn(|frame| {
        let signal = (frame as f32 * 0.35).sin() * 0.2;
        [signal, -signal * 0.8]
    });
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    let mut expected = dry;
    Eq8Definition::prepare_owned(&state, RATE, &IDS)
        .unwrap()
        .create_runtime()
        .process_block_with_automation(&mut expected, None, &BandFrames, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let project = editor.project();
    assert_eq!(project.device(device).unwrap().state(), &state);
    assert_ne!(expected, dry);
    compare_native_and_sealed_pcm::<FRAMES>(project, asset, &audio, |pcm| {
        for (index, (actual, expected)) in pcm.iter().zip(expected).enumerate() {
            for (actual, expected) in actual.iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 1e-6,
                    "frame {index}: {actual} != {expected}"
                );
            }
        }
    });
}
