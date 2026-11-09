//! Configured Limiter bypass through the actual Resident worker/publication.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{Command, Created};
use hex_session::device_gesture::{ParameterId, ParameterValue};

#[test]
fn configured_limiter_bypass_preserves_callback_continuity_and_output() {
    let _native = native_fixture();
    for lookahead in 0..3 {
        for mode in 0..3 {
            for routing in 0..2 {
                for owner in 0..3 {
                    check(lookahead, mode, routing, owner);
                }
            }
        }
    }
}

fn check(lookahead: u32, mode: u32, routing: u32, owner: u8) {
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let mut state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap()
    .with_output_gain_db(6.0)
    .unwrap();
    for (parameter, value) in [
        (6, ParameterValue::Choice(lookahead)),
        (8, ParameterValue::Choice(mode)),
        (7, ParameterValue::Choice(routing)),
        (1, ParameterValue::Float(12.0)),
    ] {
        state = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(parameter),
            value,
        )
        .unwrap();
    }
    let command = match owner {
        0 => Command::InsertDevice {
            track,
            before: None,
            state,
        },
        1 => {
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "limited".into(),
            }));
            (applied, _) = h.applied(applied);
            let Some(Created::ReturnTrack(return_track)) = added.created else {
                panic!("return");
            };
            h.run(Request::Edit(Command::SetReturnSend {
                track,
                return_track,
                gain: 1.0,
            }));
            (applied, _) = h.applied(applied);
            Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            }
        }
        _ => Command::InsertMainDevice {
            before: None,
            state,
        },
    };
    let inserted = h.run(Request::Edit(command));
    let Some(Created::Device(device)) = inserted.created else {
        panic!("limiter");
    };
    (applied, _) = h.applied(applied);
    let control = &mut h.audio.as_mut().unwrap().control;
    let output = control.output_identity();
    control.command(PlaybackCommand::PlayFrom(0)).unwrap();
    for _ in 0..32 {
        h.render();
    }
    let mut previous = h.render()[63][0];
    let dry = if owner == 1 { 0.5 } else { 0.25 };
    assert!(
        previous > dry + 0.1,
        "audible limiting fixture {lookahead}/{mode}/{routing}/{owner}: {previous}"
    );
    for (bypassed, blocks) in [(true, 1), (false, 1), (true, 5), (false, 5)] {
        let cut = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        h.run(Request::Edit(Command::SetDeviceBypass { device, bypassed }));
        for _ in 0..blocks {
            for frame in h.render() {
                assert!(
                    (frame[0] - previous).abs() < 0.01,
                    "{lookahead}/{mode}/{routing}/{owner} bypassed={bypassed}: {previous} -> {frame:?}"
                );
                previous = frame[0];
            }
        }
        let audio = h.audio.as_ref().unwrap();
        let status = audio.status().unwrap();
        assert_eq!(status.position.load(Acquire), cut + 64 * blocks);
        assert_eq!(audio.control.output_identity(), output);
        let revision = status.applied_revision.load(Acquire);
        assert!(revision > applied);
        applied = revision;
        if bypassed && blocks == 5 {
            assert!((previous - dry).abs() < 1e-5);
        }
    }
}
