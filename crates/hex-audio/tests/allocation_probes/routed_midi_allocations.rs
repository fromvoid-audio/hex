//! Actual routed Session events and handoff through the native prepared adapter.
use hex_audio::prepared;
use hex_engine::{
    LaunchQuantization, PlaybackCommand, PreparedRevision, SessionPlaybackCommand, SineTable,
    playback::Playback,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

fn measured(action: impl FnOnce()) {
    let stats = allocation_counter::measure(action);
    assert_eq!(
        (
            stats.count_total,
            stats.bytes_total,
            stats.count_current,
            stats.bytes_current
        ),
        (0, 0, 0, 0),
        "{stats:?}"
    );
}

#[test]
fn compensated_session_analytic_midi_renders_without_callback_allocations() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "midi".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("session clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: limiter,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let launch = SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    };
    let mut reference = hex_engine::SessionPlayback::new(Arc::clone(&revision)).unwrap();
    reference.apply(launch).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 512];
    for frame in expected.chunks_mut(1) {
        reference.render(frame).unwrap();
    }
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap_or_else(|error| panic!("{}", error.reason));
    control.session_command_with_ticket(launch).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 512];
    measured(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    control.stop();
    control.reclaim();
}

#[test]
fn compensated_arrangement_analytic_midi_renders_without_callback_allocations() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "midi".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: limiter,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut reference = hex_engine::SessionPlayback::new(Arc::clone(&revision)).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 512];
    for frame in expected.chunks_mut(1) {
        reference.render(frame).unwrap();
    }
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap_or_else(|error| panic!("{}", error.reason));
    control.command(PlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 512];
    measured(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    control.stop();
    control.reclaim();
}

#[test]
fn source_only_midi_fanout_launch_release_relaunch_and_handoff_have_no_callback_allocations() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let mut tracks = Vec::new();
    for _ in 0..3 {
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "midi".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        tracks.push(track);
    }
    let root = tracks[0];
    editor
        .execute(Command::RemoveInstrument { track: root })
        .unwrap();
    for (source, destination) in [(root, tracks[1]), (tracks[1], tracks[2])] {
        editor
            .execute(Command::SetMidiSource {
                source: Some(source),
                destination,
            })
            .unwrap();
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track: root,
            scene,
            name: "source".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );
    let (mut control, mut callback) = prepared::channel(48000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap())
        .unwrap_or_else(|error| panic!("{}", error.reason));
    control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    measured(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    control
        .session_command_with_ticket(SessionPlaybackCommand::StopTrack {
            track: root,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    measured(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    measured(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    editor
        .execute(Command::Gain {
            track: tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let candidate = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            table,
            Some(revision.session()),
        )
        .unwrap(),
    );
    let next = control
        .publish(Playback::try_from_prepared_with_session(candidate).unwrap())
        .unwrap_or_else(|error| panic!("{}", error.reason));
    measured(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        next
    );
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    control.stop();
    measured(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(output, [[0.0; 2]; 256]);
    control.reclaim();
}
