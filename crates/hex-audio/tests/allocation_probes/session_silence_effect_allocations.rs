//! Source-stop residual processing probe; construction and reclaim are control-side.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Audio, LaunchQuantization, PreparedRevision, SessionPlayback, SessionPlaybackCommand,
    SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn stopped_session_audio_drains_live_filter_without_callback_allocation_or_drop() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/silence.wav".into(),
            sample_rate: 48_000,
            frames: 32,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset expected"),
    };
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track expected"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "one shot".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene expected"),
    };
    let clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "original".into(),
            asset,
            source_start: 0,
            source_end: 32,
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("clip expected"),
    };
    let state = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::new("hex.utility").unwrap())
            .unwrap(),
        ParameterId::new(11).unwrap(),
        ParameterValue::Bool(true),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let launch = SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    };
    oracle.apply(launch).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap();
    control.launch_and_play_with_ticket(launch).unwrap();
    let mut prefix = [[0.0; 2]; 32];
    oracle.render(&mut prefix).unwrap();
    callback.begin_block();
    callback.render(&mut prefix).unwrap();
    control
        .session_command_with_ticket(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    // The independent oracle leaves the exhausted one-shot owner in place.
    // Both routes must feed the same zero input after its32 authored frames.
    let mut expected = [[0.0; 2]; 128];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|value| value.abs() > 1.0e-6));
    let mut actual = [[0.0; 2]; 128];
    let measured = measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(actual, expected);
    assert!(control.status.rejected_action().is_none());
    control.command(PlaybackCommand::Stop).unwrap();
    let stopped = measure(|| {
        // The first block is the queued Stop's 128-frame fade (#4151).
        for _ in 0..2 {
            callback.begin_block();
            callback.render(&mut actual).unwrap();
        }
    });
    assert_eq!(
        (
            stopped.count_total,
            stopped.bytes_total,
            stopped.count_current,
            stopped.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(actual, [[0.0; 2]; 128]);
    control.request_shutdown();
    let shutdown = measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    });
    assert_eq!(
        (
            shutdown.count_total,
            shutdown.bytes_total,
            shutdown.count_current,
            shutdown.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
}
