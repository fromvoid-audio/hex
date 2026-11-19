//! Main bypass through the session worker, publication and native callback.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_session::{
    device_gesture::{ParameterId, ParameterValue},
    service::EffectBusDestination,
};

#[test]
fn main_delay_bypass_publishes_without_a_pcm_step() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    (applied, _) = h.applied(applied);
    let added = h.run(Request::InsertEffectBusDevice {
        project_generation: 0,
        revision: imported.state.revision,
        destination: EffectBusDestination::Main,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: hex_engine::DeviceTypeId::from_static("hex.delay"),
        before: None,
    });
    let device = added.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    for (id, value) in [
        (5, ParameterValue::Bool(false)),
        (15, ParameterValue::Bool(false)),
        (26, ParameterValue::Bool(false)),
        (1, ParameterValue::Float(10.0)),
        (3, ParameterValue::Float(1.0)),
    ] {
        h.run(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(id),
            value,
        });
        (applied, _) = h.applied(applied);
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    // Let the feedback settle so a normal echo step cannot be mistaken for
    // a discontinuity caused by the bypass publication.
    for _ in 0..100 {
        h.render();
    }
    let previous = h.render()[63];
    assert!(
        previous.iter().all(|sample| *sample > 0.3),
        "delay did not reach its sustained output: {previous:?}"
    );
    for (bypassed, expected_level) in [(true, 0.25_f32), (false, 0.5_f32)] {
        let previous = if bypassed { previous } else { h.render()[63] };
        h.service
            .submit(Request::Edit(hex_project::Command::ToggleDeviceBypass {
                device,
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frames = Vec::new();
        let mut event = None;
        loop {
            h.pump();
            frames.extend_from_slice(&h.render());
            if event.is_none() {
                event = h.service.try_event().ok();
            }
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let active = status.applied_revision.load(Acquire);
            if let (Some(event), Some((revision, metadata))) = (&event, h.submitted)
                && revision > applied
                && active == revision
                && metadata.context.editor_revision == event.state.revision
            {
                break;
            }
            assert!(
                frames.len() < 45_000,
                "source exhausted before bypass applied"
            );
            assert!(Instant::now() < deadline, "bypass was not applied");
            std::thread::yield_now();
        }
        let event = event.unwrap();
        assert!(event.error.is_none(), "{:?}", event.error);
        assert_eq!(event.state.project.main_devices()[0].bypassed(), bypassed);
        applied = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire);
        // Cover the full ramp and the residual feedback, then verify the bypass
        // settles to the original dry source rather than permanently doubling it.
        for _ in 0..96 {
            frames.extend_from_slice(&h.render());
        }
        let settled = *frames.last().unwrap();
        assert!(
            settled
                .iter()
                .all(|sample| (*sample - expected_level).abs() < 0.002),
            "bypass={bypassed} did not settle to {expected_level}: {settled:?}"
        );
        let mut last = previous;
        let mut max_step = 0.0_f32;
        let mut largest_transition = (0, 0, 0.0, 0.0);
        for (offset, frame) in frames.into_iter().enumerate() {
            for lane in 0..2 {
                let step = (frame[lane] - last[lane]).abs();
                if step > max_step {
                    max_step = step;
                    largest_transition = (offset, lane, last[lane], frame[lane]);
                }
            }
            last = frame;
        }
        assert!(
            max_step < 0.003,
            "bypass={bypassed}: main PCM step {max_step}; (offset, lane, before, after): {largest_transition:?}"
        );
    }
}
