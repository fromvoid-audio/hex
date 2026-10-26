//! Main timing edits cross the production publication and callback boundaries.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_session::{
    device_gesture::{ParameterId, ParameterValue},
    service::EffectBusDestination,
};

#[test]
fn main_limiter_quiet_timing_changes_publish_and_render_the_new_delay() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::InsertEffectBusDevice {
        project_generation: 0,
        revision: initial.state.revision,
        destination: EffectBusDestination::Main,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: hex_engine::DeviceTypeId::from_static("hex.limiter"),
        before: None,
    });
    let device = added.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    for playing in [false, true] {
        if playing {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap();
            h.render();
        }
        for (parameter, choice) in [
            (6, 2),
            (8, 2),
            (7, 1),
            (6, 0),
            (7, 0),
            (8, 0),
            (6, 2),
            (8, 2),
            (7, 1),
        ] {
            h.run(Request::SetDeviceParameter {
                device,
                parameter: ParameterId::from_static(parameter),
                value: ParameterValue::Choice(choice),
            });
            (applied, _) = h.applied(applied);
        }
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    h.render();
    h.run(Request::Import(files.wav(48_000, 4096)));
    h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let frames: Vec<_> = (0..12).flat_map(|_| h.render()).collect();
    // Both mid/side stages retain 6 ms lookahead plus 31 true-peak frames.
    let latency = 2 * (288 + 31);
    assert!(frames[..latency].iter().flatten().all(|v| *v == 0.0));
    assert!(
        frames[latency..]
            .iter()
            .flatten()
            .all(|v| (*v - 0.25).abs() < 1e-5)
    );
    assert!(h.publication_refusals.is_empty());
    // Every render checks zero total and signed-net callback allocations/bytes.
}

#[test]
fn first_track_limiter_after_play_or_pause_publishes_and_parameter_edits_are_audible() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for paused in [false, true] {
        let files = Files::new();
        let mut h = ResidentHarness::with_frame_limit(131_072);
        let initial = h.next();
        assert!(initial.error.is_none());
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
        let track = imported.state.project.tracks()[0].id();
        (applied, _) = h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        assert_eq!(h.render(), [[0.25; 2]; 64]);
        if paused {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Pause)
                .unwrap();
            h.render();
        }
        let added = h.run(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
            before: None,
        });
        let device = added.state.project.track(track).unwrap().devices()[0].id();
        (applied, _) = h.applied(applied);
        h.run(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(2),
            value: ParameterValue::Float(-18.0),
        });
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
        let ceiling = 10.0_f32.powf(-18.0 / 20.0);
        for frame in h.render() {
            assert!(frame.into_iter().all(|v| (v - ceiling).abs() < 1e-5));
        }
        h.run(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(1),
            value: ParameterValue::Float(-18.0),
        });
        (applied, _) = h.applied(applied);
        // Allow the documented input-gain ramp and limiter release to settle.
        for _ in 0..600 {
            h.render();
        }
        for frame in h.render() {
            assert!(frame.into_iter().all(|v| v > 0.025 && v < 0.04));
        }
        h.run(Request::Undo);
        (applied, _) = h.applied(applied);
        for _ in 0..16 {
            h.render();
        }
        assert!(
            h.render()
                .iter()
                .flatten()
                .all(|v| (*v - ceiling).abs() < 1e-5)
        );
        h.run(Request::Redo);
        h.applied(applied);
        for _ in 0..600 {
            h.render();
        }
        assert!(h.render().iter().flatten().all(|v| *v > 0.025 && *v < 0.04));
        assert!(h.publication_refusals.is_empty());
    }
}
