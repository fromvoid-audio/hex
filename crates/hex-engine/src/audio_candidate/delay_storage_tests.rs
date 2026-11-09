//! Closed preparation uses the original Delay owner and its private ring capacities.
use super::host_output_modulation_tests::compare_native_and_sealed_pcm;
use crate::Audio;
use hex_devices::delay::{DEFINITION, DelayDefinition, DelayRuntime, TYPE_ID};
use hex_devices::modulation::lfo::{Rate, Settings, Waveform};
use hex_lattice::{OwnedProcessorRuntime, PreparedOwnedProcessorRuntime};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterValue};
use hex_project::{
    AssetId, Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute,
    Editor, ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator, Project,
};

#[test]
fn sealed_delay_retains_the_original_stereo_echo_law() {
    const RATE: u32 = 8_000;
    const FRAMES: usize = 259;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "impulse".into(),
            path: "delay-impulse.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    // Exact v1 state. The closed native constructor must use the same
    // migration as the ordinary registry, including its free stereo timing.
    let state = NativeDeviceState::new(
        TYPE_ID,
        1,
        [1.0_f32, 0.5, 1.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect(),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let mut input = vec![[0.0; 2]; FRAMES];
    input[0] = [0.5, -0.25];
    let audio = Audio::new(RATE, input).unwrap();

    compare_native_and_sealed_pcm::<FRAMES>(editor.project(), asset, &audio, |pcm| {
        for (index, frame) in pcm.iter().enumerate() {
            let expected = if index > 0 && index % 8 == 0 {
                let attenuation = 0.5_f32.powi(index as i32 / 8 - 1);
                [0.5 * attenuation, -0.25 * attenuation]
            } else {
                [0.0; 2]
            };
            assert_eq!(*frame, expected, "frame {index}");
        }
    });
}

#[test]
fn closed_delay_charges_both_real_rings_and_the_transaction_box() {
    let state = DEFINITION.default_state().unwrap();
    for rate in [8_000, 48_000, 192_000] {
        let factory = DelayDefinition::prepare_owned(&state, rate, &[]).unwrap();
        let runtime = factory.create_runtime().unwrap();
        let actual = runtime.as_any().downcast_ref::<DelayRuntime>().unwrap();
        let (inline, rings) = actual.control_storage().unwrap();
        assert_eq!(inline, std::mem::size_of::<DelayRuntime>());
        let one_ring = (rate as usize * 16 + 4) * std::mem::size_of::<[f32; 2]>();
        assert_eq!(rings, [one_ring; 2]);
        assert_eq!(
            actual.fixed_storage_pair_bytes(),
            Some(inline + 2 * one_ring)
        );
        assert_eq!(factory.declared_pair_bytes(), inline + 2 * one_ring);
    }
}

#[test]
fn delay_feedback_and_mix_reach_the_ring_and_output_on_the_exact_samples() {
    use hex_devices::delay::{DRY_WET, FEEDBACK};

    const RATE: u32 = 8_000;
    const FRAMES: usize = 157;
    let state = NativeDeviceState::new(
        TYPE_ID,
        1,
        [1.0_f32, 0.5, 0.5]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect(),
    )
    .unwrap();
    let (editor, asset) = delay_with_square_modulation(
        RATE,
        FRAMES,
        state,
        &[(FEEDBACK, 0.25 / 0.95), (DRY_WET, 0.5)],
    );

    let mut dry = [[0.0; 2]; FRAMES];
    dry[0] = [0.5, -0.25];
    dry[11] = [-0.125, 0.25];
    let mut ring = [[0.0; 2]; 8];
    let expected: [[f32; 2]; FRAMES] = std::array::from_fn(|i| {
        let high = i % 16 < 8;
        let feedback = if high { 0.75 } else { 0.25 };
        let slot = &mut ring[i % 8];
        let echo = *slot;
        *slot = [
            dry[i][0] + feedback * echo[0],
            dry[i][1] + feedback * echo[1],
        ];
        if high { echo } else { dry[i] }
    });
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    compare_native_and_sealed_pcm::<FRAMES>(editor.project(), asset, &audio, |pcm| {
        for (i, (actual, expected)) in pcm.iter().zip(expected).enumerate() {
            for (a, b) in actual.iter().zip(expected) {
                assert!((a - b).abs() <= 1.0e-6, "frame {i}: {a} != {b}");
            }
        }
    });
}

#[test]
fn delay_oscillator_depth_and_free_rate_apply_on_the_exact_samples() {
    use ParameterValue::{Bool, Choice, Float};
    use hex_devices::delay::{
        DRY_WET, EQUAL_LOUDNESS, FEEDBACK, FILTER, HIGH_QUALITY, LEFT_SYNC, LFO_DELAY, LFO_RATE,
        LFO_WAVE, RIGHT_SYNC, RIGHT_TIME, SMOOTHING, TIME,
    };

    const RATE: u32 = 8_000;
    const FRAMES: usize = 521;
    let mut state = DEFINITION.default_state().unwrap();
    for (parameter, value) in [
        (LEFT_SYNC, Bool(false)),
        (RIGHT_SYNC, Bool(false)),
        (TIME, Float(4.0)),
        (RIGHT_TIME, Float(4.0)),
        (FEEDBACK, Float(0.0)),
        (DRY_WET, Float(1.0)),
        (FILTER, Bool(false)),
        (HIGH_QUALITY, Bool(false)),
        (EQUAL_LOUDNESS, Bool(false)),
        (SMOOTHING, Choice(2)),
        (LFO_DELAY, Float(0.5)),
        (LFO_WAVE, Choice(4)),
    ] {
        state = DEFINITION
            .patch_parameter(&state, parameter, value)
            .unwrap();
    }
    let (editor, asset) =
        delay_with_square_modulation(RATE, FRAMES, state, &[(LFO_DELAY, 0.25), (LFO_RATE, 1.0)]);
    let dry: [[f32; 2]; FRAMES] =
        std::array::from_fn(|i| [(i % 97) as f32 / 128.0, -((i % 59) as f32) / 128.0]);
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    compare_native_and_sealed_pcm::<FRAMES>(editor.project(), asset, &audio, |pcm| {
        let mut phase = 0.0;
        for (i, actual) in pcm.iter().enumerate() {
            let high = i % 16 < 8;
            let age = match (phase < 0.5, high) {
                (true, true) => 56,
                (true, false) => 40,
                (false, true) => 8,
                (false, false) => 24,
            };
            let expected = i.checked_sub(age).map_or([0.0; 2], |source| dry[source]);
            assert_eq!(*actual, expected, "frame {i}");
            let hz = if high { 40.0 } else { f64::from(0.01_f32) };
            phase = (phase + hz / f64::from(RATE)).fract();
        }
    });
}

fn delay_with_square_modulation(
    rate: u32,
    frames: usize,
    state: NativeDeviceState,
    targets: &[(ParameterId, f32)],
) -> (Editor, AssetId) {
    let mut editor = Editor::new(Project::new(rate).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "delay modulation".into(),
            path: "delay-modulation.wav".into(),
            sample_rate: rate,
            frames: frames as u64,
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
        panic!("delay fixture");
    };
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let routes = targets
        .iter()
        .copied()
        .enumerate()
        .map(|(index, (id, depth))| {
            assert!(crate::project_device_modulation_target_supported(
                editor.project(),
                device,
                id
            ));
            DeviceModulationRoute {
                id: ModulationRouteId::new(next + index as u64 + 1).unwrap(),
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(id),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(depth).unwrap(),
                range: ModulationRange::default(),
                enabled: true,
            }
        })
        .collect();
    let modulation = DeviceModulation::new(
        vec![Modulator {
            id: source,
            enabled: true,
            color: Default::default(),
            state: Settings {
                rate: Rate::Hertz(500.0),
                phase: 0.0,
                waveform: Waveform::Square,
            }
            .encode()
            .unwrap(),
        }],
        routes,
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
    assert_eq!(editor.project().device(device).unwrap().state(), &state);

    (editor, asset)
}
