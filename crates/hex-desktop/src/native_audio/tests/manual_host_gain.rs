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
                pcm.len() < 60000 && Instant::now() < deadline,
                "preview publication"
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
        let mut maximum = 0.0_f32;
        for frame in pcm {
            for channel in 0..2 {
                maximum = maximum.max((frame[channel] - last[channel]).abs());
            }
            last = frame;
        }
        let gain = hex_lattice_contract::HostGain::from_db(db)
            .unwrap()
            .linear();
        assert!(
            (last[0] - 0.25 * gain).abs() < 1e-6 && (last[1] + 0.125 * gain).abs() < 1e-6,
            "gain{db} target {last:?}"
        );
        assert!(
            maximum < 0.005,
            "manual host output{db}dB callback step{maximum}"
        );
    }
}
