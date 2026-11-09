//! Saved touch ramps execute on the actual callback without heap activity.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Audio, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{
    BeatTime, Command, Created, Editor, GainTouchContext, GainTouchSpan, GainTouchStep, Project,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn saved_touch_ramp_renders_without_callback_allocation() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "touch".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 1024,
            playback_frames: 1024,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
    let end = editor.project().tempo_map().frame_to_beat(512).unwrap();
    editor
        .execute(Command::SpliceGainTouch {
            track,
            expected_epoch: 0,
            span: GainTouchSpan {
                source_start: BeatTime::ZERO,
                start: BeatTime::ZERO,
                end,
                context: GainTouchContext {
                    recording_epoch: 1,
                    project_generation: 0,
                    canonical_revision: 1,
                    audio_revision: 1,
                    transport_epoch: 1,
                    gesture_epoch: 1,
                    gesture_serial: 1,
                    sample_rate: 48_000,
                    loop_pass: 0,
                },
                steps: vec![GainTouchStep {
                    beat: BeatTime::ZERO,
                    gain: 0.8,
                    origin_beat: BeatTime::ZERO,
                    source_sequence: 1,
                }],
            },
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]; 1024]).unwrap())]),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    let counts = measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
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
    assert!((output[0][0] - 0.2 / 240.0).abs() < 1e-6);
    assert!((output[239][0] - 0.2).abs() < 1e-6);
    for block in 1..=2 {
        let counts = measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
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
        if block == 2 {
            assert!(output[0][0] > 0.0);
            assert_eq!(output[239], [0.0; 2]);
        }
    }
}

#[test]
fn long_native_device_touch_replays_late_steps_without_callback_allocation() {
    use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceMetadata};
    use hex_project::{AutomationValue, DeviceAutomationTouchSpan, DeviceAutomationTouchStep};
    const STRIDE: u64 = 32;
    const COUNT: u64 = 1_101;
    const FRAMES: u64 = (COUNT + 1) * STRIDE;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "long recorded device touch".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48_000,
            source_frames: FRAMES,
            playback_frames: FRAMES,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("media source")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("utility")
    };
    let timing = editor.project().tempo_map();
    let at = |frame| timing.frame_to_beat(frame).unwrap();
    let span = DeviceAutomationTouchSpan {
        start: BeatTime::ZERO,
        end: at(COUNT * STRIDE),
        steps: (0..COUNT)
            .map(|index| DeviceAutomationTouchStep {
                beat: at(index * STRIDE),
                value: AutomationValue::new(if index % 2 == 0 { 1.0 } else { 0.0 }).unwrap(),
            })
            .collect(),
    };
    editor
        .execute(Command::SpliceDeviceAutomationTouch {
            device,
            target: DeviceAutomationTarget::Parameter(hex_devices::utility::parameter::MUTE),
            vst3_class: None,
            spans: vec![span],
        })
        .unwrap();
    let media = BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![[0.25, -0.5]; FRAMES as usize]).unwrap(),
    )]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap();
    let origin = (COUNT - 1) * STRIDE - 4;
    control.command(PlaybackCommand::Seek(origin)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 40];
    let counts = measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
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
    assert_eq!(output[..4], [[0.25, -0.5]; 4]);
    assert_eq!(output[4..36], [[0.0; 2]; 32]);
    assert_eq!(output[36..], [[0.25, -0.5]; 4]);
}
