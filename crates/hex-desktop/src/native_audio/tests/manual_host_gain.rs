//! Supported public manual-trim requests through actual preparation/publication.
//! Host trim has no ordinary bound UI knob; this does not invent a UI workflow.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

#[test]
fn resident_manual_host_output_gain_gesture_is_continuous_and_reaches_its_target() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(super::utility_width::stereo_source(&files)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let inserted = h.run(Request::Edit(hex_project::Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.utility",
        ))
        .unwrap(),
    }));
    let Some(hex_project::Created::Device(device)) = inserted.created else {
        panic!("utility")
    };
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..8 {
        h.render();
    }
    let mut last = h.render()[63];
    assert_eq!(last, [0.25, -0.125]);
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    let begin = h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::DeviceParameter {
            device,
            parameter: hex_engine::DEVICE_OUTPUT_GAIN.id,
        },
    }));
    assert!(begin.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    for (sequence, db) in [(1, -6.0), (2, 0.0), (3, -12.0)] {
        let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence,
            value: ParameterValue::Float(db),
        }));
        assert!(changed.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
        applied = wait_gain(&mut h, applied, &mut last, db);
    }
    let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
        key,
        sequence: 4,
        manual_edit: true,
        value: ParameterValue::Float(-12.0),
    }));
    assert!(committed.gesture_ack.as_ref().is_some_and(|a| a.accepted));
    assert_eq!(
        committed
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .output_gain_millidb(),
        -12000
    );
    applied = wait_gain(&mut h, applied, &mut last, -12.0);
    let before_undo = committed.state.project.clone();
    let undone = h.run(Request::Undo);
    assert_eq!(
        undone
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .output_gain_millidb(),
        0
    );
    applied = wait_gain(&mut h, applied, &mut last, 0.0);
    let redone = h.run(Request::Redo);
    assert_eq!(redone.state.project, before_undo);
    applied = wait_gain(&mut h, applied, &mut last, -12.0);
    let cancel_key = GestureKey {
        generation: 0,
        token: 2,
    };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key: cancel_key,
        target: GestureTarget::DeviceParameter {
            device,
            parameter: hex_engine::DEVICE_OUTPUT_GAIN.id,
        },
    }));
    h.run(Request::DeviceGesture(GestureRequest::Change {
        key: cancel_key,
        sequence: 1,
        value: ParameterValue::Float(-6.0),
    }));
    applied = wait_gain(&mut h, applied, &mut last, -6.0);
    let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel {
        key: cancel_key,
    }));
    assert!(cancelled.gesture_ack.as_ref().is_some_and(|a| a.accepted));
    assert_eq!(cancelled.state.project, before_undo);
    wait_gain(&mut h, applied, &mut last, -12.0);
}

fn wait_gain(h: &mut ResidentHarness, after: u64, last: &mut hex_engine::Frame, db: f32) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut pcm = Vec::new();
    loop {
        h.pump();
        pcm.extend_from_slice(&h.render());
        if h.submitted.is_some_and(|(revision, _)| {
            revision > after
                && h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .applied_revision
                    .load(Acquire)
                    == revision
        }) {
            break;
        }
        assert!(
            pcm.len() < 60000 && Instant::now() < deadline,
            "preview publication"
        );
        std::thread::yield_now();
    }
    let applied = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(Acquire);
    for _ in 0..8 {
        pcm.extend_from_slice(&h.render());
    }
    let mut maximum = 0.0_f32;
    for frame in pcm {
        for channel in 0..2 {
            maximum = maximum.max((frame[channel] - last[channel]).abs());
        }
        *last = frame;
    }
    let gain = 10.0_f32.powf(db / 20.0);
    assert!(
        (last[0] - 0.25 * gain).abs() < 1e-6 && (last[1] + 0.125 * gain).abs() < 1e-6,
        "gain{db} target {last:?}"
    );
    assert!(
        maximum < 0.005,
        "manual host output{db}dB callback step{maximum}"
    );
    applied
}
