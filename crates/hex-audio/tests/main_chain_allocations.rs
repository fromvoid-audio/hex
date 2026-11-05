//! Real prepared publication with stateful Main; no native device or OS audio.
#![cfg(feature = "allocation-probe")]
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};
fn zero(info: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
#[test]
fn main_history_publication_render_and_shutdown_have_no_callback_allocations_or_drops() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "memory/main-callback.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 4096,
            playback_frames: 4096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let utility =
        hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility")).unwrap();
    let state = hex_engine::patch_native_device_parameter(
        &utility,
        ParameterId::from_static(11),
        ParameterValue::Bool(true),
    )
    .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let audio = Audio::new(
        48_000,
        (0..4096)
            .map(|n| [0.35 + (n as f32 * 0.07).sin() * 0.1, -0.2])
            .collect(),
    )
    .unwrap();
    let media = BTreeMap::from([(asset, audio)]);
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
    );
    let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 513];
    let mut actual = expected;
    oracle.render(&mut expected).unwrap();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    assert_eq!(actual, expected);
    // A real authored edit creates a new revision; the newly inserted unity device is
    // fresh, while the existing Main DC filter must continue its warm history.
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: utility,
        })
        .unwrap();
    let next = Arc::new(PreparedRevision::prepare(editor.project(), &media, table, None).unwrap());
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&next)).unwrap())
        .unwrap();
    oracle.render(&mut expected).unwrap();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    assert_eq!(
        actual, expected,
        "actual callback keeps Main history across authored revision"
    );
    assert!(control.status.rejected_action().is_none());
    assert!(control.reclaim() > 0, "old owners retire on control side");
    control.request_shutdown();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    control.reclaim();
    // Explicit strong owners keep terminal cleanup outside the measured callback.
    assert!(Arc::strong_count(&revision) >= 1);
    assert!(Arc::strong_count(&next) >= 1);
}

#[test]
fn latency_delayed_metronome_mix_handoff_and_stop_allocate_and_drop_nothing() {
    use hex_engine::PlaybackAction;
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "click".into(),
            path: "memory/main-click.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 256,
            playback_frames: 256,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let definition = &hex_devices::limiter::DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.0; 2]; 256]).unwrap())]);
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
    );
    assert_eq!(revision.arrangement().main_latency_frames(), 288);
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap())
        .unwrap();
    control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    // The beat-0 click enters the delay ring and starts sounding at frame 288.
    let mut output = [[0.0; 2]; 512];
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }));
    assert!(output[..288].iter().all(|frame| *frame == [0.0; 2]));
    assert!(output[288][0] > 0.1, "delayed accented click");
    // A compatible revision copies the in-flight ring without allocating.
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let next = Arc::new(PreparedRevision::prepare(editor.project(), &media, table, None).unwrap());
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&next)).unwrap())
        .unwrap();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }));
    assert!(control.reclaim() > 0, "old owners retire on control side");
    // Stop clears the dirty ring once; stopped blocks then do no ring work.
    control.command(PlaybackCommand::Stop).unwrap();
    for _ in 0..2 {
        zero(measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }));
    }
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    assert!(control.status.rejected_action().is_none());
    control.request_shutdown();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }));
    control.reclaim();
    assert!(Arc::strong_count(&revision) >= 1);
    assert!(Arc::strong_count(&next) >= 1);
}

#[path = "../../hex-engine/examples/support/latency_fixture.rs"]
mod latency_fixture;

#[test]
fn shrinking_main_clock_and_detached_tail_publication_allocate_and_drop_nothing() {
    use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentRegistry};
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "causal".into(),
            path: "memory/main-clock.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 256,
            playback_frames: 256,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let definition = latency_fixture::Definition;
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = NativeInstrumentRegistry::new(&[]).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: definition.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    let mut samples = vec![[0.0; 2]; 256];
    samples[0] = [0.25, -0.5];
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples).unwrap())]);
    let table = Arc::new(SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare_with_test_registries(
                editor.project(),
                &media,
                Arc::clone(&table),
                None,
                &effects,
                &instruments,
            )
            .unwrap(),
        )
    };
    let revision = prepare(&editor);
    let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    oracle.render(&mut [[0.0; 2]; 4]).unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut warm = [[0.0; 2]; 4];
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut warm).unwrap();
    }));
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let next = prepare(&editor);
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&next)).unwrap())
        .unwrap();
    let mut expected = [[0.0; 2]; 16];
    oracle.render(&mut expected).unwrap();
    assert_eq!(expected[3], [0.25, -0.5]);
    let mut actual = [[0.0; 2]; 16];
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    assert_eq!(
        actual, expected,
        "actual callback retains the old larger clock and old pending PCM"
    );
    assert!(control.status.rejected_action().is_none());
    assert!(control.reclaim() > 0);
    control.request_shutdown();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    control.reclaim();
    assert!(Arc::strong_count(&revision) >= 1);
    assert!(Arc::strong_count(&next) >= 1);
}
