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
    // At 48 kHz the three lookaheads are 1.5, 3 and 6 ms. True peak
    // adds 31 reconstruction frames and an 11-frame output guard per stage.
    let stage_delay =
        [72_usize, 144, 288][lookahead as usize] + if mode == 2 { 31 + 11 } else { 0 };
    let latency = stage_delay * if routing == 1 { 2 } else { 1 };
    const FADE: usize = 24;
    let reference_blocks = (latency + FADE).div_ceil(64) + 1;
    control.command(PlaybackCommand::PlayFrom(4096)).unwrap();
    let fresh_wet: Vec<_> = (0..reference_blocks).map(|_| h.render()).collect();
    for _ in reference_blocks..32 {
        h.render();
    }
    let mut previous = h.render()[63][0];
    let dry = if owner == 1 { 0.5 } else { 0.25 };
    assert!(
        previous > dry + 0.1,
        "audible limiting fixture {lookahead}/{mode}/{routing}/{owner}: {previous}"
    );
    for (bypassed, blocks, cold_reference) in [
        (true, 1, None),
        (false, 1, None),
        (true, 5, None),
        (false, fresh_wet.len(), Some(&fresh_wet)),
    ] {
        let cut = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        h.run(Request::Edit(Command::SetDeviceBypass { device, bypassed }));
        for block_index in 0..blocks {
            let actual = h.render();
            for (index, frame) in actual.iter().enumerate() {
                if let Some(reference) = cold_reference {
                    let elapsed = block_index * actual.len() + index + 1;
                    let mix = elapsed.saturating_sub(latency).min(FADE) as f32 / FADE as f32;
                    for (lane, fresh) in reference[block_index][index].iter().enumerate() {
                        let expected = dry + (fresh - dry) * mix;
                        assert!(
                            (frame[lane] - expected).abs() < 1e-5,
                            "{lookahead}/{mode}/{routing}/{owner} at {elapsed} lane {lane}: {} != {expected}",
                            frame[lane]
                        );
                    }
                } else {
                    assert!(
                        (frame[0] - previous).abs() < 0.01,
                        "{lookahead}/{mode}/{routing}/{owner} bypassed={bypassed}: {previous} -> {frame:?}"
                    );
                }
                previous = frame[0];
            }
        }
        let audio = h.audio.as_ref().unwrap();
        let status = audio.status().unwrap();
        assert_eq!(status.position.load(Acquire), cut + 64 * blocks as u64);
        assert_eq!(audio.control.output_identity(), output);
        let revision = status.applied_revision.load(Acquire);
        assert!(revision > applied);
        applied = revision;
        if bypassed && blocks == 5 {
            assert!((previous - dry).abs() < 1e-5);
        }
    }
}
