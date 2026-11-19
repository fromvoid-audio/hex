//! Actual routed Session events and handoff through the native prepared adapter.
use hex_audio::prepared;
use hex_engine::{
    LaunchQuantization, PlaybackCommand, PreparedRevision, SessionPlaybackCommand, SineTable,
    playback::{Playback, PlaybackAction},
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
fn compensated_mutable_arrangement_matches_scalar_gates_without_callback_allocations() {
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceMetadata};

    const RATE: u32 = 48_000;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let operator = hex_devices::operator_state::OPERATOR_METADATA
        .default_state()
        .unwrap();
    let operator_type = operator.type_id();
    let Created::Track(instrument) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "operator".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: operator,
        })
        .unwrap()
    else {
        panic!("instrument track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: instrument,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    for (start, duration, pitch) in [(0, 960, 48), (509, 345, 60), (940, 450, 70)] {
        let tempo = editor.project().tempo_map();
        let start = tempo.frame_to_beat(start).unwrap();
        let duration = tempo.frame_to_beat(duration).unwrap();
        editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration,
                pitch,
                channel: 0,
                velocity: 96,
            })
            .unwrap();
    }
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "residual audio".into(),
            path: "memory/companion.wav".into(),
            source_sample_rate: RATE,
            source_frames: 1_250,
            playback_frames: 1_250,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("audio track")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 64,
            end: 1_250,
            enabled: true,
        })
        .unwrap();
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(RATE, vec![[0.01, -0.01]; 1_250]).unwrap(),
    )]);
    let device = editor
        .project()
        .track(instrument)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let prepare = || {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        )
    };
    for command in [PlaybackCommand::Play, PlaybackCommand::PlayFrom(181)] {
        let reference = prepare();
        let expected_meter =
            hex_engine::PreparedPlaybackObservation::prepare_on_control(&reference).unwrap();
        let mut reference = Playback::try_from_prepared_with_session(reference).unwrap();
        reference.set_device_metering(true);
        reference
            .try_apply_declicked_action(PlaybackAction::Transport(command))
            .unwrap();
        let mut expected = [[0.0; 2]; 2_048];
        for frame in expected.chunks_mut(1) {
            reference.render(frame).unwrap();
        }
        // The opposing audio channels cannot produce this audible mono sum.
        assert!(
            expected[200..500]
                .iter()
                .any(|frame| (frame[0] + frame[1]).abs() > 0.01)
        );
        let revision = prepare();
        assert_eq!(revision.arrangement().main_latency_frames(), 72);
        let meter = hex_engine::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        let (mut control, mut callback) = prepared::channel(RATE).unwrap();
        control
            .publish(Playback::try_from_prepared_with_session(revision).unwrap())
            .unwrap_or_else(|error| panic!("{}", error.reason));
        control.command(command).unwrap();
        let mut actual = [[0.0; 2]; 2_048];
        for block in actual.chunks_mut(257) {
            measured(|| {
                callback.begin_block();
                callback.render(block).unwrap();
                callback.finish_gain_recording_quantum(true);
            });
        }
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
        }
        let expected_peaks = expected_meter
            .meter(device.value(), operator_type)
            .unwrap()
            .take_peaks();
        assert_eq!(expected_peaks.input, 0.0);
        assert!(expected_peaks.output.iter().all(|&peak| peak > 0.0));
        assert_eq!(
            meter
                .meter(device.value(), operator_type)
                .unwrap()
                .take_peaks(),
            expected_peaks,
        );
        control.stop();
        control.reclaim();
    }
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
    let Created::Device(child) = editor
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
    let prepare = |project: &Project| {
        let revision = PreparedRevision::prepare_with_device_resources(
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
        Arc::new(revision)
    };
    let first = prepare(editor.project());
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(contributor),
        })
        .unwrap();
    let moved = prepare(editor.project());
    let mut dry_editor = Editor::new(editor.project().clone(), 2).unwrap();
    dry_editor.execute(Command::RemoveDevice(child)).unwrap();
    let dry = prepare(dry_editor.project());
    editor
        .execute(Command::SetDeviceBypass {
            device: child,
            bypassed: true,
        })
        .unwrap();
    let changed = prepare(editor.project());

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
    // A simultaneous receiver and power change still lacks an exact power
    // continuation witness. The source keeps running at the old receiver.
    let rejected = publish(&mut control, Arc::clone(&changed));
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
    let removed = publish(&mut control, Arc::clone(&dry));
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

    // The very same recipe can now change power at its unchanged receiver.
    let position = control.status.position.load(Ordering::Acquire) as usize;
    let mut dry_reference =
        hex_engine::SessionPlayback::new_with_device_resources(dry, &resources).unwrap();
    dry_reference.apply(SessionPlaybackCommand::Play).unwrap();
    dry_reference.render(&mut vec![[0.0; 2]; position]).unwrap();
    let powered_off = publish(&mut control, changed);
    let fade = render(&mut callback);
    let mut expected = [[0.0; 2]; 256];
    dry_reference.render(&mut expected).unwrap();
    assert_eq!(fade[239..], expected[239..]);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        powered_off
    );
    dry_reference.render(&mut expected).unwrap();
    assert_eq!(render(&mut callback), expected);
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
}

#[test]
fn fenced_nested_effect_removal_and_cancel_allocate_and_free_nothing() {
    use hex_audio::prepared::{
        FreshRuntimeCancellationResult, PublicationPolicy, PublicationReceiptState,
    };
    use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use hex_lattice_contract::NativeDeviceMetadata;
    use std::sync::atomic::AtomicBool;

    const RATE: u32 = 8_000;
    const FADE: usize = 40;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "generator".into(),
            devices: vec![
                hex_devices::fm4::device::FM4_DEFINITION
                    .default_state()
                    .unwrap(),
                hex_devices::delay::DEFINITION.default_state().unwrap(),
            ],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source");
    };
    let removed = editor
        .project()
        .native_source(source)
        .unwrap()
        .devices()
        .nth(1)
        .unwrap()
        .id();
    for (ordinal, pitch) in [60, 67].into_iter().enumerate() {
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(source),
            })
            .unwrap();
        if ordinal == 0 {
            editor
                .execute(Command::SetNativeSourceReceiver {
                    source,
                    track: Some(track),
                })
                .unwrap();
        }
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch,
                velocity: 96,
                channel: 0,
            })
            .unwrap();
    }
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "independent audio".into(),
            path: "independent.wav".into(),
            sample_rate: RATE,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("independent audio");
    };
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(RATE, vec![[0.2, -0.05]; 512]).unwrap(),
    )]);
    let table = Arc::new(SineTable::new());
    let cancelled = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1 << 20,
        &cancelled,
    )
    .unwrap();
    let prepare = |project: &Project| {
        Arc::new(
            PreparedRevision::prepare_with_device_resources(
                project,
                &media,
                Arc::clone(&table),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                0,
                &cancelled,
            )
            .unwrap(),
        )
    };
    let old = prepare(editor.project());
    editor.execute(Command::RemoveDevice(removed)).unwrap();
    let new = prepare(editor.project());
    let other_receiver = editor.project().tracks()[1].id();
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(other_receiver),
        })
        .unwrap();
    let incompatible = prepare(editor.project());
    let runtime = |graph: &Arc<PreparedRevision>| {
        Playback::try_from_prepared_with_device_resources(Arc::clone(graph), &resources).unwrap()
    };
    let reference = |graph: &Arc<PreparedRevision>| {
        let mut result =
            hex_engine::SessionPlayback::new_with_device_resources(Arc::clone(graph), &resources)
                .unwrap();
        result.apply(SessionPlaybackCommand::Play).unwrap();
        result
    };
    let (mut wet_reference, mut dry_reference) = (reference(&old), reference(&new));
    let (mut control, mut callback) = prepared::channel(RATE).unwrap();
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
    let context = hex_engine::AudioSourceContext {
        project_generation: 1,
        editor_revision: 2,
    };
    control
        .publish_with_admission_and_source_on_control(
            runtime(&old),
            PublicationPolicy::InstallFreshRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut render = |count: usize| {
        let mut actual = vec![[0.0; 2]; count];
        let mut wet = actual.clone();
        let mut dry = actual.clone();
        wet_reference.render(&mut wet).unwrap();
        dry_reference.render(&mut dry).unwrap();
        measured(|| {
            callback.begin_block();
            callback.render(&mut actual).unwrap();
            callback.finish_gain_recording_quantum(true);
        });
        (actual, wet, dry)
    };
    let (actual, wet, _) = render(129);
    assert_eq!(actual, wet);
    let predecessor = control.status.recording_source().unwrap();
    let next_context = hex_engine::AudioSourceContext {
        editor_revision: 3,
        ..context
    };
    let submit = |control: &mut prepared::Control, graph| {
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let revision = control
            .publish_device_insert_with_receipt_on_control(
                runtime(graph),
                PublicationPolicy::DeviceInsert { predecessor, fence },
                next_context,
                |_| {},
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        control.fresh_runtime_receipt(revision).unwrap()
    };
    let refused = submit(&mut control, &incompatible);
    let (actual, wet, _) = render(11);
    assert!(matches!(
        refused.state(),
        PublicationReceiptState::Refused(_)
    ));
    assert_eq!(control.status.recording_source(), Some(predecessor));
    assert_eq!(
        actual, wet,
        "an incompatible receiver edit must not arm a fade"
    );
    control.reclaim();

    let cancelled = submit(&mut control, &new);
    let _ = render(11);
    assert_eq!(cancelled.state(), PublicationReceiptState::Pending);
    assert_eq!(
        cancelled.cancel(),
        FreshRuntimeCancellationResult::Cancelled
    );
    let (actual, wet, _) = render(FADE + 8);
    assert_eq!(cancelled.state(), PublicationReceiptState::Cancelled);
    assert_eq!(control.status.recording_source(), Some(predecessor));
    assert_eq!(actual[FADE..], wet[FADE..]);
    control.reclaim();

    let accepted = submit(&mut control, &new);
    let mut elapsed = 0;
    for count in [11, 28, 1] {
        let (actual, wet, dry) = render(count);
        for index in 0..count {
            let mix = 1.0 - (elapsed + index + 1) as f32 / FADE as f32;
            for channel in 0..2 {
                let dry = dry[index][channel];
                let wet = wet[index][channel];
                assert!((actual[index][channel] - (dry + (wet - dry) * mix)).abs() < 1e-6);
            }
        }
        elapsed += count;
        assert_eq!(accepted.state(), PublicationReceiptState::Pending);
        assert_eq!(control.status.recording_source(), Some(predecessor));
    }
    let (actual, _, dry) = render(16);
    assert_eq!(actual, dry);
    assert_eq!(accepted.state(), PublicationReceiptState::Installed);
    let applied = control.status.recording_source().unwrap();
    assert_eq!(applied.audio_revision, accepted.revision());
    assert_eq!(applied.context, next_context);
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
}

#[test]
fn selected_session_note_chase_off_handoff_and_pause_allocate_nothing() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "session".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                hex_devices::SINE_INSTRUMENT_TYPE_ID,
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "notes".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let hex_project::SessionCellContentSnapshot::Midi(clip) =
        &mut snapshot.session.cells[0].content
    else {
        panic!("clip");
    };
    const Q: u64 = 1 << 32;
    clip.playback_offset_raw = 3 * Q / 16;
    clip.source.source_length_raw = Q / 4;
    clip.source.source_start_raw = 0;
    clip.source.loop_start_raw = Q / 16;
    clip.source.loop_end_raw = Q / 4;
    clip.source.looping = true;
    clip.source.notes = vec![
        hex_project::MidiNoteSnapshot {
            active: true,
            id: 1_000_001,
            start_raw: Q / 8,
            duration_raw: Q / 8,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
        hex_project::MidiNoteSnapshot {
            active: true,
            id: 1_000_002,
            start_raw: 3 * Q / 16,
            duration_raw: Q / 64,
            pitch: 64,
            channel: 0,
            velocity: 100,
        },
    ];
    snapshot.next_id = 1_000_003;
    let mut oracle_snapshot = snapshot.clone();
    let hex_project::SessionCellContentSnapshot::Midi(clip) =
        &mut oracle_snapshot.session.cells[0].content
    else {
        panic!("clip");
    };
    clip.playback_offset_raw = 0;
    clip.source.source_start_raw = 3 * Q / 16;
    let revisions = [snapshot, oracle_snapshot].map(|snapshot| {
        Arc::new(
            PreparedRevision::prepare(
                &Project::from_snapshot(snapshot).unwrap(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        )
    });
    let (mut control, mut callback) = prepared::channel(8_000).unwrap();
    let (mut oracle_control, mut oracle_callback) = prepared::channel(8_000).unwrap();
    for (control, revision) in [
        (&mut control, &revisions[0]),
        (&mut oracle_control, &revisions[1]),
    ] {
        control
            .publish(Playback::try_from_prepared_with_session(Arc::clone(revision)).unwrap())
            .unwrap_or_else(|e| panic!("{}", e.reason));
        control
            .session_command_with_ticket(SessionPlaybackCommand::LaunchClipWithNoteChase {
                clip: revision.session().midi_clips()[0].id,
                quantization: LaunchQuantization::Immediate,
                note_chase: hex_engine::NoteChase::Off,
            })
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
    }
    for step in 0..8 {
        if step == 1 {
            for (control, revision) in [
                (&mut control, &revisions[0]),
                (&mut oracle_control, &revisions[1]),
            ] {
                control
                    .publish(
                        Playback::try_from_prepared_with_session(Arc::clone(revision)).unwrap(),
                    )
                    .unwrap_or_else(|e| panic!("{}", e.reason));
            }
        }
        if step == 2 {
            control.command(PlaybackCommand::Pause).unwrap();
            oracle_control.command(PlaybackCommand::Pause).unwrap();
        }
        let mut expected = [[0.0; 2]; 32];
        oracle_callback.begin_block();
        oracle_callback.render(&mut expected).unwrap();
        let mut output = [[0.0; 2]; 32];
        measured(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(output, expected, "step {step}");
        if step == 0 {
            assert!(output.iter().flatten().any(|s| *s != 0.0));
        }
        control.reclaim();
        oracle_control.reclaim();
    }
    control.stop();
    oracle_control.stop();
    let mut output = [[0.0; 2]; 32];
    measured(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    oracle_callback.begin_block();
    oracle_callback.render(&mut output).unwrap();
    control.reclaim();
    oracle_control.reclaim();
}
