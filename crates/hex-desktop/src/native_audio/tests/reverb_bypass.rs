//! Routed bypass through the session worker, publication and native callback.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_session::{device_gesture::ParameterValue, service::EffectBusDestination};

#[test]
fn frozen_reverb_bypass_undo_redo_publishes_without_a_pcm_step_on_all_routes() {
    let _native = native_fixture();
    for location in 0..3 {
        check(location);
    }
}

fn check(location: usize) {
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(262_144);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 262_144)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let type_id = hex_engine::DeviceTypeId::from_static("hex.reverb");
    let added = match location {
        0 => h.run(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        }),
        1 => {
            let bus = h.run(Request::Edit(hex_project::Command::AddReturnTrack {
                name: "room".into(),
            }));
            let Some(hex_project::Created::ReturnTrack(bus)) = bus.created else {
                panic!("return")
            };
            (applied, _) = h.applied(applied);
            let sent = h.run(Request::Edit(hex_project::Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 1.0,
            }));
            (applied, _) = h.applied(applied);
            h.run(Request::InsertEffectBusDevice {
                project_generation: 0,
                revision: sent.state.revision,
                destination: EffectBusDestination::Return(bus),
                class: hex_engine::NativeDeviceClass::Effect,
                type_id,
                before: None,
            })
        }
        _ => h.run(Request::InsertEffectBusDevice {
            project_generation: 0,
            revision: imported.state.revision,
            destination: EffectBusDestination::Main,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id,
            before: None,
        }),
    };
    let device = match location {
        0 => added.state.project.tracks()[0].devices()[0].id(),
        1 => added.state.project.return_tracks()[0].devices()[0].id(),
        _ => added.state.project.main_devices()[0].id(),
    };
    let dry_track = if location == 1 { 0.25 } else { 0.0 };
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
        room.iter().flatten().any(|v| (v - dry_track).abs() > 1e-8),
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
        assert_eq!(
            event.state.project.device(device).unwrap().bypassed(),
            bypassed
        );
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
        let mut last = previous;
        let mut max_step = 0.0_f32;
        let mut largest_transition = (0, 0, 0.0, 0.0);
        for (offset, frame) in frames.iter().copied().enumerate() {
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
            "location={location} bypass={bypassed}: PCM step {max_step}; (offset, lane, before, after): {largest_transition:?}"
        );
        if bypassed {
            assert!(
                settled
                    .iter()
                    .all(|sample| (*sample - dry_track - 0.25).abs() < 1e-5),
                "dry endpoint: {settled:?}"
            );
        } else {
            assert!(
                settled
                    .iter()
                    .all(|sample| (sample - dry_track).abs() < 0.2),
                "room endpoint: {settled:?}"
            );
            assert!(
                frames[frames.len() - 512..]
                    .iter()
                    .flatten()
                    .any(|v| (v - dry_track).abs() > 1e-8),
                "frozen history lost"
            );
        }
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
    for _ in 0..super::FADE_BLOCKS {
        h.render();
    }
    for _ in 0..96 {
        assert_eq!(h.render(), [[dry_track; 2]; 64]);
    }
}
