//! Native adapter quantum semantics without opening a physical device.
use hex_audio::{
    gain_recording::{AbortReason, Receipt, Target},
    prepared,
};
use hex_engine::{Audio, PlaybackCommand, PreparedRevision, SineTable, playback::Playback};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering::Acquire},
};

fn fixture() -> (prepared::Control, prepared::Callback, Target) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "touch".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48000,
            source_frames: 4096,
            playback_frames: 4096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
    let prepared = Arc::new(
        PreparedRevision::prepare_for_gain_recording(
            editor.project(),
            &BTreeMap::from([(asset, Audio::new(48000, vec![[0.25, -0.5]; 4096]).unwrap())]),
            Arc::new(SineTable::new()),
            None,
            track,
        )
        .unwrap(),
    );
    let playback = Playback::try_from_prepared_with_session(prepared).unwrap();
    let (mut control, mut callback) = prepared::channel(48000).unwrap();
    let revision = control
        .publish(playback)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut output = [[9.0; 2]; 16];
    callback.render(&mut output).unwrap();
    callback.finish_gain_recording_quantum(true);
    assert_eq!(output, [[0.0; 2]; 16]);
    assert!(control.status.gain_recording_ready.load(Acquire));
    let target = Target {
        track_id: track.value(),
        project_generation: 0,
        canonical_revision: 1,
        audio_revision: revision,
        recording_epoch: 1,
        transport_epoch: control.status.transport_epoch.load(Acquire),
        gesture_serial: 1,
        sample_rate: 48000,
    };
    (control, callback, target)
}

fn callback_scope(action: impl FnOnce()) {
    #[cfg(feature = "allocation-probe")]
    {
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
    #[cfg(not(feature = "allocation-probe"))]
    action();
}

#[test]
fn touch_recording_rapid_retarget_and_release_are_click_free() {
    let (mut control, mut callback, target) = fixture();
    let mut captured = Vec::new();
    control.gain_recording.begin(target, 0.8).unwrap();
    for operation in 0..3 {
        if operation == 1 {
            control.gain_recording.change(0.2).unwrap();
        } else if operation == 2 {
            control.gain_recording.release().unwrap();
        }
        let mut output = [[0.0; 2]; 16];
        callback_scope(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
            callback.finish_gain_recording_quantum(true);
        });
        captured.extend(output);
    }
    assert!(captured.iter().flatten().any(|sample| sample.abs() > 1e-5));
    let mut previous = [0.0_f32; 2];
    for (index, frame) in captured.iter().enumerate() {
        for channel in 0..2 {
            assert!(
                (frame[channel] - previous[channel]).abs() < 0.005,
                "frame {index}, channel {channel}: {} -> {}",
                previous[channel],
                frame[channel]
            );
        }
        previous = *frame;
    }
}

#[test]
fn delivered_touch_quantums_record_absolute_gain_and_release_returns_to_saved_read() {
    let (mut control, mut callback, target) = fixture();
    control.gain_recording.begin(target, 0.8).unwrap();
    let mut output = [[9.0; 2]; 16];
    callback_scope(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(output, [[0.2, -0.4]; 16]);
    assert!(
        control.gain_recording.try_receipt().is_none(),
        "rendering alone never acknowledges sink delivery"
    );
    callback_scope(|| callback.finish_gain_recording_quantum(true));
    assert!(
        matches!(control.gain_recording.try_receipt(), Some(Receipt::Applied {
        target: actual, gain: 0.8, performance_frame: 16, content_frame: 16, loop_pass: 0, ..
    }) if actual == target)
    );
    control.gain_recording.release().unwrap();
    callback_scope(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(output, [[0.0; 2]; 16]);
    assert!(matches!(
        control.gain_recording.try_receipt(),
        Some(Receipt::Finished {
            performance_frame: 32,
            content_frame: 32,
            ..
        })
    ));
    assert!(control.gain_recording.target().is_none());
}

#[test]
fn failed_sink_discards_take_and_priority_stop_works_with_a_full_transport_queue() {
    let (mut control, mut callback, target) = fixture();
    control.gain_recording.begin(target, 0.8).unwrap();
    let mut output = [[0.0; 2]; 16];
    callback_scope(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(false);
    });
    assert!(matches!(
        control.gain_recording.try_receipt(),
        Some(Receipt::Superseded { .. })
    ));
    assert!(matches!(
        control.gain_recording.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::OutputFailed,
            ..
        })
    ));
    let next = Target {
        gesture_serial: 2,
        ..target
    };
    control.gain_recording.begin(next, 0.5).unwrap();
    for _ in 0..32 {
        control.command(PlaybackCommand::Play).unwrap();
    }
    assert!(control.command(PlaybackCommand::Play).is_err());
    control.stop();
    callback_scope(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(output, [[0.0; 2]; 16]);
    assert!(matches!(
        control.gain_recording.try_receipt(),
        Some(Receipt::Superseded { .. })
    ));
    assert!(matches!(
        control.gain_recording.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::TransportChanged,
            ..
        })
    ));
    assert!(control.gain_recording.try_receipt().is_none());
    assert!(!control.status.gain_recording_fault.load(Acquire));
}

#[cfg(feature = "allocation-probe")]
#[path = "../../hex-engine/examples/support/latency_fixture.rs"]
mod latency_fixture;

#[cfg(feature = "allocation-probe")]
#[test]
fn compensated_touch_waits_for_the_main_sample_and_preserves_release_to_read() {
    use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentRegistry};
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    for delayed in [false, true] {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "compensated touch".into(),
                path: "memory.wav".into(),
                source_sample_rate: 48000,
                source_frames: 4096,
                playback_frames: 4096,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::InsertInstrument {
                track,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        if delayed {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: latency_fixture::Definition.default_state().unwrap(),
                })
                .unwrap();
        } else {
            editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
        }
        media.insert(asset, Audio::new(48000, vec![[0.25, -0.5]; 4096]).unwrap());
        tracks.push(track);
    }
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&latency_fixture::Definition];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = NativeInstrumentRegistry::new(&[]).unwrap();
    let revision = PreparedRevision::prepare_with_test_registries(
        editor.project(),
        &media,
        Arc::new(SineTable::new()),
        None,
        &effects,
        &instruments,
    )
    .unwrap()
    .with_test_gain_recording(editor.project(), tracks[0])
    .unwrap();
    let playback = Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap();
    let (mut control, mut callback) = prepared::channel(48000).unwrap();
    let audio_revision = control
        .publish(playback)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut warmup = [[0.0; 2]; 16];
    callback.render(&mut warmup).unwrap();
    callback.finish_gain_recording_quantum(true);
    assert_eq!(warmup[15], [0.25, -0.5]);
    let target = Target {
        track_id: tracks[0].value(),
        project_generation: 0,
        canonical_revision: 1,
        audio_revision,
        recording_epoch: 1,
        transport_epoch: control.status.transport_epoch.load(Acquire),
        gesture_serial: 1,
        sample_rate: 48000,
    };
    control.gain_recording.begin(target, 0.8).unwrap();
    let mut output = [[0.0; 2]; 4];
    callback_scope(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(output, [[0.25, -0.5]; 4]);
    assert!(control.gain_recording.try_receipt().is_none());
    callback_scope(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(&output[..3], &[[0.25, -0.5]; 3]);
    assert_eq!(output[3], [0.45, -0.9]);
    assert!(matches!(
        control.gain_recording.try_receipt(),
        Some(Receipt::Applied {
            performance_frame: 16,
            content_frame: 16,
            gain: 0.8,
            ..
        })
    ));
    control.gain_recording.release().unwrap();
    callback_scope(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(output, [[0.45, -0.9]; 4]);
    assert!(control.gain_recording.try_receipt().is_none());
    callback_scope(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
    });
    assert_eq!(&output[..3], &[[0.45, -0.9]; 3]);
    assert_eq!(output[3], [0.25, -0.5]);
    assert!(matches!(
        control.gain_recording.try_receipt(),
        Some(Receipt::Finished {
            performance_frame: 24,
            content_frame: 24,
            ..
        })
    ));
    assert!(control.gain_recording.target().is_none());
}
