//! Utility width gestures through the real worker and resident callback.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_session::device_gesture::{
    GestureKey, GestureRequest, GestureTarget, ParameterId, ParameterValue,
};

fn stereo_source(files: &Files) -> PathBuf {
    let frames = 131_072_u32;
    let rate = 48_000_u32;
    let path = files.0.join("asymmetric-stereo.wav");
    let mut bytes = Vec::from(*b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 4).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 4).to_le_bytes());
    for _ in 0..frames {
        bytes.extend_from_slice(&8192_i16.to_le_bytes());
        bytes.extend_from_slice(&(-4096_i16).to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn unautomated_width_gesture_preserves_callback_continuity() {
    width_gesture(false, false);
}

#[test]
fn width_gesture_with_balance_automation_preserves_callback_continuity() {
    width_gesture(true, false);
}

#[test]
fn width_gesture_with_automation_and_dc_filter_preserves_callback_continuity() {
    width_gesture(true, true);
}

fn width_gesture(automated: bool, dc_remove: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(stereo_source(&files)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let inserted = h.run(Request::Edit(hex_project::Command::InsertDevice {
        track,
        before: None,
        state,
    }));
    let Some(hex_project::Created::Device(device)) = inserted.created else {
        panic!("utility")
    };
    (applied, _) = h.applied(applied);
    if dc_remove {
        h.run(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(11),
            value: ParameterValue::Bool(true),
        });
        (applied, _) = h.applied(applied);
    }
    if automated {
        h.run(Request::Edit(
            hex_project::Command::ReplaceDeviceAutomation {
                device,
                parameter: ParameterId::from_static(2),
                points: vec![hex_project::AutomationPoint {
                    beat: hex_project::BeatTime::ZERO,
                    value: hex_project::AutomationValue::new(0.5).unwrap(),
                    outgoing: hex_project::AutomationInterpolation::Linear,
                }],
            },
        ));
        (applied, _) = h.applied(applied);
    }
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
    if dc_remove {
        assert!(
            last[0] > 0.1 && last[1] < -0.05,
            "filtered source: {last:?}"
        );
    } else {
        assert!(
            (last[0] - 0.25).abs() < 0.002 && (last[1] + 0.125).abs() < 0.002,
            "source: {last:?}"
        );
    }
    let key = GestureKey {
        generation: 0,
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
        (4, 0.0, [0.0625, 0.0625]),
    ] {
        let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence,
            value: ParameterValue::Float(width),
        }));
        assert!(changed.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut pcm = Vec::new();
        loop {
            h.pump();
            pcm.extend_from_slice(&h.render());
            if h.submitted.is_some_and(|(revision, _)| {
                revision > applied
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
                pcm.len() < 60_000 && Instant::now() < deadline,
                "preview not applied"
            );
            std::thread::yield_now();
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
        let mut max_step = 0.0_f32;
        for frame in pcm {
            for channel in 0..2 {
                max_step = max_step.max((frame[channel] - last[channel]).abs());
            }
            last = frame;
        }
        assert!(max_step < 0.005, "width {width}: callback step {max_step}");
        if !dc_remove {
            assert!(
                (last[0] - target[0]).abs() < 0.002 && (last[1] - target[1]).abs() < 0.002,
                "width {width}: {last:?}"
            );
        }
    }
}
