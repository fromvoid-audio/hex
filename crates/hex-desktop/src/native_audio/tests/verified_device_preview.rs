//! The existing bound Utility-width gesture contract through the real Verified
//! reader, worker, closed publication and callback, with only output substituted.
use super::*;
use hex_session::device_gesture::{
    GestureKey, GestureRequest, GestureTarget, ParameterId, ParameterValue,
};

#[test]
fn verified_utility_width_reaches_callback_before_commit_with_literal_stereo_pcm() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = super::verified_gain::actual_native_harness();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(super::utility_width::stereo_source(&files)));
    assert!(matches!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .next()
            .unwrap()
            .1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    h.applied(h.context.unwrap());
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
    let canonical = inserted.state.project.clone();
    let source = inserted.state.canonical_audio_source.clone().unwrap();
    let output_identity = h.audio.as_ref().unwrap().control.output_identity();
    let mut applied = h.applied(h.context.unwrap()).audio_revision;
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let ready = Instant::now() + Duration::from_secs(3);
    let mut last = loop {
        let sample = h.render()[63];
        if sample == [0.25, -0.125] {
            break sample;
        }
        assert!(
            Instant::now() < ready,
            "verified source never reached the literal steady input: {sample:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let key = GestureKey {
        generation: h.context.unwrap().project_generation,
        token: 1,
    };
    let begin = h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::DeviceParameter {
            device,
            parameter: ParameterId::from_static(3),
        },
    }));
    assert!(begin.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    for (sequence, width, target) in [
        (1, 0.0, [0.0625, 0.0625]),
        (2, 2.0, [0.4375, -0.3125]),
        (3, 4.0, [0.8125, -0.6875]),
    ] {
        let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence,
            value: ParameterValue::Float(width),
        }));
        assert!(changed.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
        assert_eq!(changed.state.project, canonical);
        assert!(Arc::ptr_eq(
            changed.state.canonical_audio_source.as_ref().unwrap(),
            &source
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut pcm = Vec::new();
        loop {
            pcm.extend_from_slice(&h.render());
            if h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source()
                .is_some_and(|actual| {
                    actual.audio_revision > applied
                        && actual.context == h.context.unwrap()
                        && pcm.last().is_some_and(|frame| {
                            (frame[0] - target[0]).abs() < 1e-6
                                && (frame[1] - target[1]).abs() < 1e-6
                        })
                })
            {
                break;
            }
            assert!(
                pcm.len() < 60000 && Instant::now() < deadline,
                "preview publication: expected context={:?}, applied={:?}, resident_playback={}, preview={:?}, events={:?}",
                h.context,
                h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .applied_audio_source(),
                changed.playback.is_some(),
                changed.device_preview.as_ref().map(|p| p.identity),
                h.events
                    .try_iter()
                    .map(|e| match e {
                        EditorAudioEvent::Publication { context, state, .. } =>
                            format!("{context:?}: {state:?}"),
                        EditorAudioEvent::Opened { .. } => "opened".into(),
                        _ => "other event".into(),
                    })
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        applied = h
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
            last = frame;
        }
        assert!(
            (last[0] - target[0]).abs() < 1e-6 && (last[1] - target[1]).abs() < 1e-6,
            "width{width} target {target:?}, actual {last:?}"
        );
        assert!(maximum < 0.005, "width{width} callback step{maximum}");
    }
    let old_preview = h
        .observation
        .lock()
        .unwrap()
        .received_verified_device
        .clone()
        .unwrap();
    let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    assert!(cancelled.gesture_ack.unwrap().accepted);
    assert_eq!(cancelled.state.project, canonical);
    applied = wait_pcm(&h, applied, [0.25, -0.125]);
    h.observation.lock().unwrap().replay_verified_device = Some(old_preview);
    // Give the worker multiple idle polls; a closed key cannot resurrect audio.
    let stable_until = Instant::now() + Duration::from_millis(20);
    while Instant::now() < stable_until {
        assert!(h.render().iter().all(|frame| *frame == [0.25, -0.125]));
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire),
            applied
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let second = GestureKey { token: 2, ..key };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key: second,
        target: GestureTarget::DeviceParameter {
            device,
            parameter: ParameterId::from_static(3),
        },
    }));
    // Queue changes with the callback idle: the existing single overlay coalesces.
    for sequence in 1..=32 {
        h.run(Request::DeviceGesture(GestureRequest::Change {
            key: second,
            sequence,
            value: ParameterValue::Float(sequence as f32 / 8.0),
        }));
    }
    applied = wait_pcm(&h, applied, [0.8125, -0.6875]);
    let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
        manual_edit: true,
        key: second,
        sequence: 32,
        value: ParameterValue::Float(4.0),
    }));
    assert!(committed.gesture_ack.unwrap().accepted);
    applied = wait_pcm(&h, applied, [0.8125, -0.6875]);
    let undo = h.run(Request::Undo);
    assert_eq!(undo.state.project, canonical);
    applied = wait_pcm(&h, applied, [0.25, -0.125]);
    let redone = h.run(Request::Redo);
    assert_eq!(redone.state.project, committed.state.project);
    applied = wait_pcm(&h, applied, [0.8125, -0.6875]);
    let third = GestureKey { token: 3, ..key };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key: third,
        target: GestureTarget::DeviceParameter {
            device,
            parameter: ParameterId::from_static(3),
        },
    }));
    h.run(Request::DeviceGesture(GestureRequest::Change {
        key: third,
        sequence: 1,
        value: ParameterValue::Float(0.0),
    }));
    applied = wait_pcm(&h, applied, [0.0625, 0.0625]);
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: true,
            key: third,
            sequence: 2,
            value: ParameterValue::Float(f32::NAN),
        }))
        .unwrap();
    let refused = h.next();
    assert!(!refused.gesture_ack.unwrap().accepted);
    assert!(refused.active_device_gesture.is_none());
    assert_eq!(refused.state.project, committed.state.project);
    wait_pcm(&h, applied, [0.8125, -0.6875]);
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        output_identity
    );
}

fn wait_pcm(h: &Harness, previous: u64, expected: [f32; 2]) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut settled = 0;
    loop {
        let block = h.render();
        let actual = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .unwrap();
        if actual.audio_revision > previous
            && actual.context == h.context.unwrap()
            && block.iter().all(|frame| {
                (frame[0] - expected[0]).abs() < 1e-6 && (frame[1] - expected[1]).abs() < 1e-6
            })
        {
            settled += 1;
            if settled == 2 {
                return actual.audio_revision;
            }
        } else {
            settled = 0;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected:?}, actual {:?}, revision {actual:?}",
            block[63]
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
