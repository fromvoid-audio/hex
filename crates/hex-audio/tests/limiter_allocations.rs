//! Real prepared native Limiter graph through the callback publication path.
//! This proves bounded allocation behavior only, not device deadlines or Live parity.
#![cfg(feature = "allocation-probe")]

use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Audio, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const LIMITER: DeviceTypeId = DeviceTypeId::from_static("hex.limiter");

fn assert_zero(measured: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current,
        ),
        (0, 0, 0, 0),
    );
}

fn parameter(
    state: &hex_lattice_contract::NativeDeviceState,
    id: hex_lattice_contract::ParameterId,
    value: ParameterValue,
) -> hex_lattice_contract::NativeDeviceState {
    hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Effect,
        state,
        id,
        value,
    )
    .expect("validated Limiter test parameter")
}

/// Control-side allocation measurement verifies the endpoint's full requested
/// Arc layout independently of the callback's zero-allocation measurements.
#[test]
fn retained_meter_backing_charge_includes_the_actual_arc_padding() {
    let allocation = measure(|| {
        let meter = Arc::new(hex_lattice::DeviceMeter::default());
        std::hint::black_box(&meter);
    });
    assert_eq!(allocation.count_total, 1);
    assert_eq!(
        allocation.bytes_total,
        (std::mem::size_of::<hex_lattice::DeviceMeter>() + 2 * std::mem::size_of::<usize>()) as u64
    );
    assert_eq!((allocation.count_current, allocation.bytes_current), (0, 0));
}

#[test]
fn limiter_standard_soft_clip_true_peak_and_ms_callbacks_allocate_nothing() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "limiter callback".into(),
            path: "memory/limiter-callback.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 4096,
            playback_frames: 4096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("audio source track")
    };
    let Created::ReturnTrack(return_track) = editor
        .execute(Command::AddReturnTrack {
            name: "limiter return".into(),
        })
        .unwrap()
    else {
        panic!("return track")
    };

    let defaults = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        LIMITER,
    )
    .unwrap();
    let track_limiter = parameter(
        &parameter(
            &defaults,
            hex_devices::limiter::parameter::MODE,
            ParameterValue::Choice(0),
        ),
        hex_devices::limiter::parameter::ROUTING,
        ParameterValue::Choice(0),
    );
    let main_limiter = parameter(
        &parameter(
            &defaults,
            hex_devices::limiter::parameter::MODE,
            ParameterValue::Choice(1),
        ),
        hex_devices::limiter::parameter::ROUTING,
        ParameterValue::Choice(1),
    );
    let return_limiter = parameter(
        &parameter(
            &defaults,
            hex_devices::limiter::parameter::MODE,
            ParameterValue::Choice(2),
        ),
        hex_devices::limiter::parameter::ROUTING,
        ParameterValue::Choice(1),
    );
    let Created::Device(track_device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: track_limiter,
        })
        .unwrap()
    else {
        panic!("track Limiter")
    };
    let Created::Device(main_device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: main_limiter,
        })
        .unwrap()
    else {
        panic!("Main Limiter")
    };
    assert_eq!(
        editor
            .project()
            .device(main_device)
            .unwrap()
            .state()
            .type_id(),
        LIMITER
    );
    let Created::Device(return_device) = editor
        .execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: return_limiter,
        })
        .unwrap()
    else {
        panic!("Return Limiter")
    };
    assert_eq!(
        editor
            .project()
            .device(return_device)
            .unwrap()
            .state()
            .type_id(),
        LIMITER
    );
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        })
        .unwrap();

    let samples: Vec<[f32; 2]> = (0..4096)
        .map(|frame| {
            let phase = frame as f32 * 0.071;
            let transient = if frame % 127 == 0 { 1.35 } else { 0.0 };
            [phase.sin() * 0.9 + transient, (phase * 1.37).cos() * 0.72]
        })
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples).unwrap())]);
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
    );
    assert!(revision.arrangement().main_latency_frames() > 0);

    // The oracle and callback independently render the same full signal.
    // A test-only control channel delivers the same revision after block 24;
    // no waiting or oracle allocation occurs inside a callback measurement.
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (expected, actual, next_revision) = std::thread::scope(|scope| {
        let initial = Arc::clone(&revision);
        let (next_tx, next_rx) = std::sync::mpsc::sync_channel(1);
        let oracle = scope.spawn(move || {
            let mut oracle = Playback::try_from_prepared_with_session(initial).unwrap();
            oracle.apply(PlaybackCommand::Play);
            let mut blocks = vec![[[0.0; 2]; 128]; 32];
            for output in &mut blocks[..24] {
                oracle.render(output).unwrap();
            }
            let next = next_rx.recv().unwrap();
            let mut next_oracle = Playback::try_from_prepared_with_session(next).unwrap();
            next_oracle
                .try_inherit_runtime_from(&mut oracle)
                .expect("Limiter parameter revision preserves compatible history");
            for output in &mut blocks[24..] {
                next_oracle.render(output).unwrap();
            }
            blocks
        });
        let mut blocks = vec![[[0.0; 2]; 128]; 32];
        for output in &mut blocks[..24] {
            assert_zero(measure(|| {
                callback.begin_block();
                callback.render(output).unwrap();
            }));
        }
        let current = editor.project().device(track_device).unwrap().state();
        let changed = parameter(
            current,
            hex_devices::limiter::parameter::GAIN,
            ParameterValue::Float(1.0),
        );
        editor
            .execute(Command::ReplaceDeviceState {
                device: track_device,
                state: changed,
            })
            .unwrap();
        let next_revision = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &media,
                Arc::clone(&table),
                Some(revision.session()),
            )
            .unwrap(),
        );
        next_tx.send(Arc::clone(&next_revision)).unwrap();
        control
            .publish(Playback::try_from_prepared_with_session(Arc::clone(&next_revision)).unwrap())
            .unwrap();
        for output in &mut blocks[24..] {
            assert_zero(measure(|| {
                callback.begin_block();
                callback.render(output).unwrap();
            }));
        }
        (oracle.join().unwrap(), blocks, next_revision)
    });
    assert_eq!(actual, expected);
    assert!(
        control.reclaim() > 0,
        "retired graph returns to the control owner"
    );

    control.request_shutdown();
    let mut actual = [[0.0; 2]; 128];
    assert_zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    control.reclaim();
    assert!(Arc::strong_count(&revision) >= 1);
    assert!(Arc::strong_count(&next_revision) >= 1);
}
