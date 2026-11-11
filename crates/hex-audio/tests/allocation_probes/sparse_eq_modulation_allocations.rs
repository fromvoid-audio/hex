use hex_audio::prepared;
use hex_devices::{
    eq::{EQ_DEFINITION, EQ_TYPE_ID, MODE, OUTPUT_GAIN},
    modulation::lfo::{Rate, Settings, Waveform},
};
use hex_engine::{
    Audio, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
    project_device_modulation_target_supported,
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    Command, Created, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Editor,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator, Project,
};
use std::{collections::BTreeMap, hint::black_box, sync::Arc};

#[test]
fn sparse_eq_output_gain_modulation_follows_the_exact_callback_samples_without_allocating() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 269;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "eq fixture".into(),
            path: "eq-fixture.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("fixture track");
    };
    let state = EQ_DEFINITION.default_state().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("fixture eq");
    };
    assert!(project_device_modulation_target_supported(
        editor.project(),
        device,
        OUTPUT_GAIN
    ));
    assert!(!project_device_modulation_target_supported(
        editor.project(),
        device,
        MODE
    ));
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
                device_type: EQ_TYPE_ID,
                target: DeviceAutomationTarget::Parameter(OUTPUT_GAIN),
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
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, audio)]),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
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
    for (index, frame) in output.into_iter().enumerate() {
        let gain = if index % 4 < 2 { 1.5 } else { 0.5 };
        assert_eq!(frame, dry.map(|sample| sample * gain), "frame {index}");
    }
    assert_eq!(editor.project().device(device).unwrap().state(), &state);
}
