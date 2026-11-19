//! Production-default worker publication, with only the physical output substituted.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{Command, Created};

#[test]
fn ordinary_native_reverb_track_return_edits_frozen_removal_and_stop_reach_real_callback() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 1)));
    let track = imported.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    let type_id = hex_engine::DeviceTypeId::from_static("hex.reverb");
    let added = h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(type_id),
        before: None,
    });
    let device = added.state.project.track(track).unwrap().devices()[0].id();
    let state = added.state.project.device(device).unwrap().state().clone();
    (applied, _) = h.applied(applied);
    let bus = h.run(Request::Edit(Command::AddReturnTrack {
        name: "room".into(),
    }));
    let Some(Created::ReturnTrack(bus)) = bus.created else {
        panic!("return");
    };
    (applied, _) = h.applied(applied);
    let returned = h.run(Request::Edit(Command::InsertReturnDevice {
        return_track: bus,
        before: None,
        state,
    }));
    let return_device = returned.state.project.return_tracks()[0].devices()[0].id();
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::SetReturnSend {
        track,
        return_track: bus,
        gain: 1.0,
    }));
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render(); // The one-frame original has ended before the early reflections.
    let mut tail_energy = 0.0_f64;
    for _ in 0..32 {
        tail_energy += h
            .render()
            .iter()
            .flatten()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>();
    }
    assert!(
        tail_energy > 1e-8,
        "native output contains the reverberant decay"
    );
    for id in [device, return_device] {
        let authored = returned.state.project.device(id).unwrap().state().clone();
        let frozen = hex_engine::patch_native_device_parameter(
            &authored,
            hex_session::device_gesture::ParameterId::from_static(26),
            hex_session::device_gesture::ParameterValue::Bool(true),
        )
        .unwrap();
        h.run(Request::Edit(Command::ReplaceDeviceState {
            device: id,
            state: frozen,
        }));
        (applied, _) = h.applied(applied);
    }
    for id in [device, return_device] {
        h.run(Request::Edit(Command::RemoveDevice(id)));
        (applied, _) = h.applied(applied);
    }
    assert!(
        h.render().iter().flatten().any(|v| v.abs() > 1e-10),
        "removed frozen native histories still sound"
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    // The queued Stop settles after its declick fade.
    for _ in 0..super::FADE_BLOCKS {
        h.render();
    }
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    // Every h.render above checks both total and signed-net allocations/bytes.
}
