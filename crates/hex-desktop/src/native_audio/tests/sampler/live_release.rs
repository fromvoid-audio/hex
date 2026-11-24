//! Resident Sampler power-off, input retirement and fresh keyboard activation.
use super::*;

#[test]
fn live_sampler_power_cycle_accepts_only_the_fresh_keyboard_generation() {
    for key_up in ["before-disable", "while-disabled", "after-reenable"] {
        check(key_up);
    }
}

fn check(key_up: &str) {
    use super::resident_preview::ResidentHarness;
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 99,
        project_generation: 0,
        revision: added.state.revision,
        target: hex_session::service::SamplerLoadTarget::Track {
            track,
            slot: hex_session::service::SamplerLoadSlot::New(
                hex_project::InstrumentLoadPlacement::First,
            ),
        },
        path: files.wav(48_000, 65_536),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let device = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (applied, _) = h.applied(applied);
    let state = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .state()
        .clone();
    let state = hex_engine::patch_sampler_parameter_on_control(
        hex_engine::SamplerParameter::ReleaseMs,
        &state,
        100.0,
    )
    .unwrap();
    let changed = h.run(Request::Edit(Command::ReplaceDeviceState { device, state }));
    (applied, _) = h.applied(applied);
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    let keyboard = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: changed.state.revision,
        request_id: 100,
        track,
        input: Arc::clone(&input),
    });
    assert!(keyboard.error.is_none(), "{:?}", keyboard.error);
    h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    input.set_key(0, Some((60, 127, 1))).unwrap();
    for _ in 0..16 {
        h.render();
    }
    let mut before = h.render();
    assert!((before[63][0] - 0.25).abs() < 0.0001);
    if key_up == "before-disable" {
        input.set_key(0, None).unwrap();
        before = h.render();
    }
    let toggled = h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
    assert!(toggled.error.is_none(), "{:?}", toggled.error);
    let after = h.render();
    assert!(
        (after[0][0] - before[63][0]).abs() < 0.001,
        "{key_up}: bypass {before:?} -> {after:?}"
    );
    assert!(
        input.is_cancelled(),
        "bypass terminally retires this input root"
    );
    if key_up == "while-disabled" {
        input.set_key(0, None).unwrap();
    }
    input.set_key(1, Some((67, 127, 2))).unwrap();
    let mut previous = after[63];
    for block in 1..=16 {
        for (offset, frame) in h.render().into_iter().enumerate() {
            for channel in 0..2 {
                assert!(frame[channel] >= 0.0 && frame[channel] <= previous[channel] + 1e-6);
            }
            if block * 64 + offset >= 240 {
                assert_eq!(frame, [0.0; 2], "old voice survived the 5 ms power cut");
            }
            previous = frame;
        }
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let cut = status
        .performance_frame
        .load(std::sync::atomic::Ordering::Acquire);
    let bypass_revision = status
        .applied_revision
        .load(std::sync::atomic::Ordering::Acquire);
    let enabled = h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
    assert!(enabled.error.is_none(), "{:?}", enabled.error);
    let resumed = h.render();
    assert_eq!(
        resumed, [[0.0; 2]; 64],
        "{key_up}: old keys must stay silent"
    );
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert_eq!(
        status
            .performance_frame
            .load(std::sync::atomic::Ordering::Acquire),
        cut + 64
    );
    assert!(
        status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire)
            > bypass_revision
    );
    let enabled_audio = status
        .applied_revision
        .load(std::sync::atomic::Ordering::Acquire);
    let fresh = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    let keyboard = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: enabled.state.revision,
        request_id: 101,
        track,
        input: Arc::clone(&fresh),
    });
    assert!(keyboard.error.is_none(), "{:?}", keyboard.error);
    h.applied(enabled_audio);
    fresh.set_key(1, Some((67, 127, 1))).unwrap();
    let new_note = h.render();
    assert!(
        new_note[63].iter().all(|x| (x - 0.25).abs() < 1e-5),
        "fresh key must reach the level of one voice"
    );
    input.set_key(0, None).unwrap();
    fresh.set_key(0, None).unwrap();
    assert!(
        h.render().iter().flatten().all(|x| (x - 0.25).abs() < 1e-5),
        "old physical key up cannot cut the new generation"
    );
    fresh.set_key(1, None).unwrap();
    let release = h.render();
    assert!(
        release[63]
            .iter()
            .all(|sample| *sample > 0.0 && *sample < 0.25),
        "ordinary key-up must use the fresh voice's configured release"
    );
    let mut end = [[0.0; 2]; 64];
    for _ in 0..100 {
        end = h.render();
    }
    assert!(end.iter().flatten().all(|x| x.abs() < 0.000001));
}
