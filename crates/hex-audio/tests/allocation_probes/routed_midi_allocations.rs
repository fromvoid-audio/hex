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

#[test]
fn source_delay_history_survives_live_receiver_move_without_callback_allocations() {
    use hex_devices::delay;
    use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(receiver) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor")
    };
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let parent = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: contributor,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
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

    let mut definitions = Editor::new(editor.project().clone(), 8).unwrap();
    for track in [receiver, contributor] {
        definitions
            .execute(Command::BindTrackSource {
                track,
                source: None,
            })
            .unwrap();
    }
    let mut state = delay::DEFINITION.default_state().unwrap();
    for (parameter, value) in [
        (delay::LEFT_SYNC, ParameterValue::Bool(false)),
        (delay::RIGHT_SYNC, ParameterValue::Bool(false)),
        (delay::TIME, ParameterValue::Float(3.0)),
        (delay::RIGHT_TIME, ParameterValue::Float(3.0)),
        (delay::DRY_WET, ParameterValue::Float(1.0)),
    ] {
        state = delay::DEFINITION
            .patch_parameter(&state, parameter, value)
            .unwrap();
    }
    let Created::Device(child) = definitions
        .execute(Command::InsertNestedDevice { parent, state })
        .unwrap()
    else {
        panic!("source delay")
    };
    let table = Arc::new(SineTable::new());
    let cancel = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1 << 20,
        &cancel,
    )
    .unwrap();
    let prepare = |project: &Project, retained: &Project| {
        let mut revision = PreparedRevision::prepare_with_device_resources(
            project,
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancel,
        )
        .unwrap();
        revision
            .test_install_compiled_source_effects(retained)
            .unwrap();
        Arc::new(revision)
    };
    let first = prepare(editor.project(), definitions.project());
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(contributor),
        })
        .unwrap();
    let moved = prepare(editor.project(), definitions.project());
    let dry = Arc::new(
        PreparedRevision::prepare_with_device_resources(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancel,
        )
        .unwrap(),
    );
    definitions
        .execute(Command::SetDeviceBypass {
            device: child,
            bypassed: true,
        })
        .unwrap();
    let changed = prepare(editor.project(), definitions.project());

    let mut reference =
        hex_engine::SessionPlayback::new_with_device_resources(Arc::clone(&first), &resources)
            .unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 256,
                root_rows: 64,
                max_retained_bytes: 2_000_000,
                max_construction_peak_bytes: 2_000_000,
            },
        )
        .unwrap();
    let publish = |control: &mut prepared::Control, revision| {
        control
            .publish_with_admission_on_control(
                Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap(),
                prepared::PublicationPolicy::InheritRuntime,
                1,
                |_| Ok::<_, ()>(()),
            )
            .unwrap_or_else(|_| panic!("source graph could not be queued"))
    };
    let old = publish(&mut control, first);
    control.command(PlaybackCommand::Play).unwrap();

    let mut compare = |callback: &mut prepared::Callback| {
        let mut actual = [[0.0; 2]; 256];
        let mut expected = [[0.0; 2]; 256];
        reference.render(&mut expected).unwrap();
        measured(|| {
            callback.begin_block();
            callback.render(&mut actual).unwrap();
            callback.finish_gain_recording_quantum(true);
        });
        for (actual, expected) in actual.iter().zip(expected) {
            assert!((actual[0] - expected[0]).abs() < 1e-6);
            assert!((actual[1] - expected[1]).abs() < 1e-6);
        }
        assert!(expected.iter().flatten().any(|sample| sample.abs() > 1e-5));
        actual
    };
    let wet = compare(&mut callback);
    assert!(wet[..100].iter().flatten().all(|sample| *sample == 0.0));
    let rejected = publish(&mut control, changed);
    compare(&mut callback);
    assert_ne!(old, rejected);
    assert_eq!(control.status.applied_revision.load(Ordering::Acquire), old);
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));

    let next = publish(&mut control, Arc::clone(&moved));
    compare(&mut callback);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        next
    );
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));

    let rejected_removal = publish(&mut control, Arc::clone(&dry));
    compare(&mut callback);
    assert_ne!(rejected_removal, next);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        next
    );
    control.reclaim();

    let render = |callback: &mut prepared::Callback| {
        let mut pcm = [[0.0; 2]; 256];
        measured(|| {
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
            callback.finish_gain_recording_quantum(true);
        });
        pcm
    };
    control.stop();
    assert_eq!(render(&mut callback), [[0.0; 2]; 256]);
    let removed = publish(&mut control, dry);
    assert_eq!(render(&mut callback), [[0.0; 2]; 256]);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        removed
    );
    control.reclaim();

    control.command(PlaybackCommand::Play).unwrap();
    let dry_pcm = render(&mut callback);
    assert!(
        dry_pcm[..100]
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1e-5)
    );
    let attached = publish(&mut control, moved);
    let wet_pcm = render(&mut callback);
    assert!(wet_pcm[..100].iter().flatten().all(|sample| *sample == 0.0));
    assert!(
        wet_pcm[160..]
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1e-5)
    );
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        attached
    );
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
}
