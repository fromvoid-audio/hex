//! Main bypass through the session worker, publication and native callback.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_session::{device_gesture::ParameterValue, service::EffectBusDestination};

#[test]
fn frozen_main_reverb_bypass_undo_redo_publishes_without_a_pcm_step() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 262_144)));
    (applied, _) = h.applied(applied);
    let added = h.run(Request::InsertEffectBusDevice {
        project_generation: 0,
        revision: imported.state.revision,
        destination: EffectBusDestination::Main,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: hex_engine::DeviceTypeId::from_static("hex.reverb"),
        before: None,
    });
    let device = added.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    let descriptors =
        hex_engine::native_device_descriptors(hex_engine::DeviceTypeId::from_static("hex.reverb"))
            .unwrap();
    for (label, value) in [
        ("dry/wet", ParameterValue::Float(100.0)),
        ("lo cut", ParameterValue::Bool(true)),
        ("spin", ParameterValue::Bool(false)),
        ("chorus", ParameterValue::Bool(false)),
    ] {
        let parameter = descriptors.iter().find(|p| p.label == label).unwrap().id;
        h.run(Request::SetDeviceParameter {
            device,
            parameter,
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
    // Excite a real room, then freeze its private history before publishing bypass.
    for _ in 0..750 {
        h.render();
    }
    h.run(Request::SetDeviceParameter {
        device,
        parameter: descriptors.iter().find(|p| p.label == "freeze").unwrap().id,
        value: ParameterValue::Bool(true),
    });
    (applied, _) = h.applied(applied);
    for _ in 0..96 {
        h.render();
    }
    let room = h.render();
    assert!(
        room.iter().flatten().any(|v| v.abs() > 1e-8),
        "frozen room was not excited"
    );
    for (request, bypassed) in [
        (
            Request::Edit(hex_project::Command::ToggleDeviceBypass { device }),
            true,
        ),
        (Request::Undo, false),
        (Request::Redo, true),
        (
            Request::Edit(hex_project::Command::ToggleDeviceBypass { device }),
            false,
        ),
    ] {
        let previous = h.render()[63];
        h.service.submit(request).unwrap();
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
        for _ in 0..96 {
            frames.extend_from_slice(&h.render());
        }
        let settled = *frames.last().unwrap();
        if bypassed {
            assert!(
                settled.iter().all(|sample| (*sample - 0.25).abs() < 1e-5),
                "dry endpoint: {settled:?}"
            );
        } else {
            assert!(
                settled.iter().all(|sample| sample.abs() < 0.2),
                "room endpoint: {settled:?}"
            );
            assert!(
                frames[frames.len() - 512..]
                    .iter()
                    .flatten()
                    .any(|v| v.abs() > 1e-8),
                "frozen history lost"
            );
        }
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
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    for _ in 0..super::FADE_BLOCKS {
        h.render();
    }
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..96 {
        assert_eq!(h.render(), [[0.0; 2]; 64]);
    }
}
