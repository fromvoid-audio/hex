//! Actual Resident input retirement, callback re-enable and fresh keyboard
//! activation while the original dry Sampler release is still audible.
use super::*;

#[test]
fn long_live_sampler_release_rejoins_and_accepts_a_fresh_keyboard_generation() {
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
        track,
        slot: hex_session::service::SamplerLoadSlot::New { replacing: None },
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
    let mut previous = after;
    for _ in 0..16 {
        previous = h.render();
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
    assert!(
        (resumed[0][0] - previous[63][0]).abs() < 0.001,
        "{key_up}: re-enable {previous:?} -> {resumed:?}"
    );
    assert!(resumed.iter().flatten().any(|x| *x > 0.1));
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
        new_note.iter().flatten().any(|x| *x > 0.3),
        "new key must sound during the inherited release"
    );
    input.set_key(0, None).unwrap();
    fresh.set_key(0, None).unwrap();
    assert!(
        h.render().iter().flatten().any(|x| *x > 0.3),
        "old physical key up cannot cut the new generation"
    );
    fresh.set_key(1, None).unwrap();
    let mut end = [[0.0; 2]; 64];
    for _ in 0..100 {
        end = h.render();
    }
    assert!(end.iter().flatten().all(|x| x.abs() < 0.000001));
}
