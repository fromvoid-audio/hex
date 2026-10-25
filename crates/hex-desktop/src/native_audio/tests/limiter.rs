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
