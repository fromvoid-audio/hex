use hex_audio::prepared;
use hex_devices::{
    limiter::{
        DEFINITION, TYPE_ID,
        parameter::{GAIN, LOOKAHEAD},
    },
    modulation::lfo::{Rate, Settings, Waveform},
};
use hex_engine::{
    Audio, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
    project_device_modulation_target_supported,
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor, ModulationAmount,
    ModulationDestination, ModulationOutputId, ModulationRange, ModulationRouteId,
    ModulationSourceId, Modulator, Project,
};
use std::{collections::BTreeMap, hint::black_box, sync::Arc};

#[test]
fn limiter_input_gain_modulation_reaches_both_prepared_paths_without_allocating() {
    for structural in [false, true] {
        limiter_input_gain_modulation_case(structural);
    }
}

fn limiter_input_gain_modulation_case(structural: bool) {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 541;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "limiter fixture".into(),
            path: "limiter-fixture.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("fixture track");
    };
    let state = DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("fixture limiter");
    };
    assert!(project_device_modulation_target_supported(
        editor.project(),
        device,
        GAIN
    ));
    assert!(!project_device_modulation_target_supported(
        editor.project(),
        device,
        LOOKAHEAD
    ));
    if structural {
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: LOOKAHEAD,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.0).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                }],
            })
            .unwrap();
    }
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let modulation = DeviceModulation::new(
        vec![Modulator {
            id: source,
            color: Default::default(),
            enabled: true,
            state: Settings {
                rate: Rate::Hertz(12_000.0),
                phase: 0.0,
                waveform: Waveform::Square,
            }
            .encode()
            .unwrap(),
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(next + 1).unwrap(),
            source,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type: TYPE_ID,
                target: DeviceAutomationTarget::Parameter(GAIN),
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.125).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        }],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
    let dry = [0.03125, -0.015625];
    let audio = Audio::new(RATE, vec![dry; FRAMES]).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, audio)]),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let latency = if structural { 288 } else { 72 };
    assert_eq!(revision.arrangement().main_latency_frames(), latency as u32);
    let playback = Playback::try_from_prepared_with_session(revision).unwrap();
    let (mut control, mut callback) = prepared::channel(RATE).unwrap();
    assert!(control.publish(playback).is_ok());
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; FRAMES];
    let counts = allocation_counter::measure(|| {
        for block in output.chunks_mut(41) {
            callback.begin_block();
            callback.render(black_box(block)).unwrap();
        }
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let mut gain = 1.0;
    for (index, frame) in output.into_iter().enumerate() {
        let expected = if let Some(sample) = index.checked_sub(latency) {
            let db = if sample % 4 < 2 { 6.0 } else { -6.0 };
            gain += (10.0_f64.powf(db / 20.0) - gain) / (0.005 * f64::from(RATE));
            dry.map(|value| (f64::from(value) * gain) as f32)
        } else {
            [0.0; 2]
        };
        assert_eq!(frame, expected, "structural={structural}, frame {index}");
    }
    assert_eq!(editor.project().device(device).unwrap().state(), &state);
}
