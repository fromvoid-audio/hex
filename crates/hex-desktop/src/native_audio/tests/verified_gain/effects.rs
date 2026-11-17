//! Built-in effect publication through a real verified-file source.
use super::*;
use hex_project::{Command, Created};
use hex_session::device_gesture::{
    GestureKey, GestureRequest, GestureTarget, ParameterId, ParameterValue,
};

#[test]
fn stopped_verified_stream_installs_auto_pan_and_renders_processed_audio() {
    check_auto_pan(false);
}

#[test]
fn running_verified_stream_installs_auto_pan_and_renders_processed_audio() {
    check_auto_pan(true);
}

fn check_auto_pan(running: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.wait = Duration::from_secs(1);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 100_000)));
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
    if running {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        wait_for_audio(&h);
    }
    let track = imported.state.project.tracks()[0].id();
    let auto_pan = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.auto-pan"),
    )
    .unwrap();
    let inserted = h.run(Request::Edit(Command::InsertMainDevice {
        before: None,
        state: auto_pan,
    }));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("Auto Pan device")
    };
    h.applied(h.context.unwrap());
    if !running {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    wait_for_audio(&h);
    let mut asymmetric = false;
    for _ in 0..20 {
        let block = h.render();
        asymmetric |= block
            .iter()
            .any(|frame| (frame[0] - frame[1]).abs() > 0.0001);
    }
    assert!(
        asymmetric,
        "Auto Pan must process the mono source into different channels"
    );
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(status.rejected_revision().is_none());
    if running {
        let key = GestureKey {
            generation: h.context.unwrap().project_generation,
            token: 1,
        };
        for request in [
            GestureRequest::Begin {
                key,
                target: GestureTarget::TrackGain { track },
            },
            GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.1),
            },
        ] {
            let event = h.run(Request::DeviceGesture(request));
            assert!(
                event.gesture_ack.as_ref().unwrap().accepted,
                "{:?}",
                event.gesture_ack
            );
        }
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = h.render()[63];
            if frame[0].abs().max(frame[1].abs()) < 0.05 && frame != [0.0; 2] {
                break;
            }
            assert!(
                Instant::now() < until,
                "gain preview did not reach audio: {frame:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let event = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
        assert!(event.gesture_ack.as_ref().unwrap().accepted);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = h.render()[63];
            if frame[0].abs().max(frame[1].abs()) > 0.1 {
                break;
            }
            assert!(
                Instant::now() < until,
                "gain cancel did not restore audio: {frame:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let key = GestureKey { token: 2, ..key };
        for request in [
            GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: ParameterId::from_static(1),
                },
            },
            GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.0),
            },
        ] {
            let event = h.run(Request::DeviceGesture(request));
            assert!(
                event.gesture_ack.as_ref().unwrap().accepted,
                "{:?}",
                event.gesture_ack
            );
        }
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = h.render()[63];
            if (frame[0] - frame[1]).abs() < 0.00001 && frame[0].abs() > 0.1 {
                break;
            }
            assert!(
                Instant::now() < until,
                "device preview did not reach audio: {frame:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let event = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
        assert!(event.gesture_ack.as_ref().unwrap().accepted);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = h.render()[63];
            if (frame[0] - frame[1]).abs() > 0.0001 {
                break;
            }
            assert!(
                Instant::now() < until,
                "device cancel did not restore audio: {frame:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn wait_for_audio(h: &Harness) {
    let ready = Instant::now() + Duration::from_secs(3);
    while !h
        .render()
        .iter()
        .any(|frame| frame[0] != 0.0 || frame[1] != 0.0)
    {
        assert!(
            Instant::now() < ready,
            "verified source did not become audible"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn stopped_verified_stream_installs_main_saturator() {
    check_main_saturator(false);
}

#[test]
fn running_verified_stream_installs_main_saturator() {
    check_main_saturator(true);
}

fn check_main_saturator(running: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.wait = Duration::from_secs(1);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    h.run(Request::Import(files.wav(48_000, 100_000)));
    let prior = h.applied(h.context.unwrap());
    if running {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        wait_for_audio(&h);
    }
    let saturator = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.saturator"),
    )
    .unwrap();
    h.run(Request::Edit(Command::InsertMainDevice {
        before: None,
        state: saturator,
    }));
    let installed = h.applied(h.context.unwrap());
    assert_ne!(installed.audio_revision, prior.audio_revision);
    if !running {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    wait_for_audio(&h);
    for _ in 0..8 {
        h.render();
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(
        status.position.load(Acquire) > status.audible_position.load(Acquire),
        "Main processor reports latency"
    );
    assert!(status.rejected_revision().is_none());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Seek(70_000))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire)
        < 70_000
    {
        h.render();
        assert!(Instant::now() < deadline, "streamed seek did not commit");
        std::thread::sleep(Duration::from_millis(1));
    }
    wait_for_audio(&h);
}
