//! Main timing edits cross the production publication and callback boundaries.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_session::{
    device_gesture::{ParameterId, ParameterValue},
    service::EffectBusDestination,
};

#[test]
fn loop_edit_after_pausing_a_track_limiter_publishes_current_audio() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    });
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..12 {
        h.render();
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Pause)
        .unwrap();
    h.render();
    let edit = h.run(Request::Edit(hex_project::Command::SetLoopRegion {
        start: 256,
        end: 4096,
        enabled: true,
    }));
    assert!(edit.error.is_none(), "{:?}", edit.error);
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..12 {
        h.render();
    }
    for _ in 0..100 {
        assert!(
            h.render()
                .iter()
                .flatten()
                .all(|v| (*v - 0.25).abs() < 1e-5)
        );
    }
    let position = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    assert!((256..4096).contains(&position), "loop position {position}");
    let added = h.run(Request::InsertEffectBusDevice {
        project_generation: 0,
        revision: edit.state.revision,
        destination: EffectBusDestination::Main,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: hex_engine::DeviceTypeId::from_static("hex.limiter"),
        before: None,
    });
    assert!(added.error.is_none(), "{:?}", added.error);
    let device = added.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    h.run(Request::SetDeviceParameter {
        device,
        parameter: ParameterId::from_static(2),
        value: ParameterValue::Float(-18.0),
    });
    h.applied(applied);
    for _ in 0..12 {
        h.render();
    }
    let ceiling = 10.0_f32.powf(-18.0 / 20.0);
    assert!(
        h.render()
            .iter()
            .flatten()
            .all(|v| (*v - ceiling).abs() < 1e-5)
    );
}

#[test]
fn main_limiter_added_after_sounding_track_limiter_applies_to_audio() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    let initial = h.next();
    assert!(initial.error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    let track_added = h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    });
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..12 {
        h.render();
    }
    assert!(
        h.render()
            .iter()
            .flatten()
            .all(|v| (*v - 0.25).abs() < 1e-5)
    );
    let added = h.run(Request::InsertEffectBusDevice {
        project_generation: 0,
        revision: track_added.state.revision,
        destination: EffectBusDestination::Main,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: hex_engine::DeviceTypeId::from_static("hex.limiter"),
        before: None,
    });
    assert!(added.error.is_none(), "{:?}", added.error);
    let device = added.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    h.run(Request::SetDeviceParameter {
        device,
        parameter: ParameterId::from_static(2),
        value: ParameterValue::Float(-18.0),
    });
    h.applied(applied);
    for _ in 0..12 {
        h.render();
    }
    let ceiling = 10.0_f32.powf(-18.0 / 20.0);
    assert!(
        h.render()
            .iter()
            .flatten()
            .all(|v| (*v - ceiling).abs() < 1e-5)
    );
}

#[test]
fn main_limiter_quiet_timing_changes_publish_and_render_the_new_delay() {
    let _native = native_fixture();
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
fn changing_existing_latency_while_playing_applies_without_stopping() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    let limiter = || Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    };
    h.run(limiter());
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..12 {
        h.render();
    }
    // A second limiter changes the latency layout while playing. It applies
    // at a block boundary (audio restarts there) and the transport keeps
    // running; no stop, pause or rebuild is needed.
    let added = h.run(limiter());
    let device = added.state.project.track(track).unwrap().devices()[1].id();
    (applied, _) = h.applied(applied);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(status.position.load(Acquire) > 0);
    h.run(Request::SetDeviceParameter {
        device,
        parameter: ParameterId::from_static(2),
        value: ParameterValue::Float(-18.0),
    });
    h.applied(applied);
    for _ in 0..20 {
        h.render();
    }
    let ceiling = 10.0_f32.powf(-18.0 / 20.0);
    let output = h.render();
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    assert!(
        output
            .iter()
            .flatten()
            .all(|sample| *sample <= ceiling + 1e-5)
    );
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
}

fn first_track_limiter_after_transport_state(paused: bool) {
    let _native = native_fixture();
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
    let undone = h.run(Request::Undo);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            undone.state.project.device(device).unwrap().state(),
            ParameterId::from_static(1),
        )
        .unwrap(),
        ParameterValue::Float(0.0)
    );
    (applied, _) = h.applied(applied);
    // The exponential input ramp and lookahead precede this steady-state oracle.
    for _ in 0..100 {
        h.render();
    }
    let undo_output = h.render();
    assert!(
        undo_output
            .iter()
            .flatten()
            .all(|v| (*v - ceiling).abs() < 1e-5),
        "undo ceiling {ceiling}; first={:?}, last={:?}",
        undo_output.first(),
        undo_output.last()
    );
    h.run(Request::Redo);
    h.applied(applied);
    for _ in 0..600 {
        h.render();
    }
    assert!(h.render().iter().flatten().all(|v| *v > 0.025 && *v < 0.04));
    assert!(h.publication_refusals.is_empty());
}

#[test]
fn first_track_limiter_after_play_publishes_and_parameter_edits_are_audible() {
    first_track_limiter_after_transport_state(false);
}

#[test]
fn first_track_limiter_after_pause_publishes_and_parameter_edits_are_audible() {
    first_track_limiter_after_transport_state(true);
}

#[test]
fn first_track_limiter_keeps_active_reverb_prefix_through_native_publication() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(65_536);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 65_536)));
    let track = imported.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.reverb")),
        before: None,
    });
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..16 {
        h.render();
    }
    let added = h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    });
    let device = added.state.project.track(track).unwrap().devices()[1].id();
    (applied, _) = h.applied(applied);
    h.run(Request::SetDeviceParameter {
        device,
        parameter: ParameterId::from_static(2),
        value: ParameterValue::Float(-18.0),
    });
    h.applied(applied);
    for _ in 0..16 {
        h.render();
    }
    let frames = h.render();
    let ceiling = 10.0_f32.powf(-18.0 / 20.0);
    assert!(
        frames
            .iter()
            .flatten()
            .all(|sample| sample.abs() <= ceiling + 1e-6)
    );
    assert!(frames.iter().flatten().any(|sample| sample.abs() > 0.02));
    assert!(h.publication_refusals.is_empty());
    // Every h.render also measures total and signed-net callback allocations/bytes.
}

/// Field report (build 6de4bd2d): a latency-changing device dropped while
/// paused mid-song was refused, and every later revision, including a new
/// instrument track, was refused against the stale applied graph, so live
/// input could not bind. Paused publication applies at the cursor instead.
#[test]
fn paused_latency_change_then_new_instrument_track_applies_and_binds_live_input() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    let limiter = || Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    };
    h.run(limiter());
    (applied, _) = h.applied(applied);
    let control = |h: &mut ResidentHarness, command| {
        h.audio.as_mut().unwrap().control.command(command).unwrap();
    };
    control(&mut h, PlaybackCommand::Play);
    for _ in 0..12 {
        h.render();
    }
    control(&mut h, PlaybackCommand::Pause);
    // The queued Pause settles after its declick fade.
    for _ in 0..super::FADE_BLOCKS {
        h.render();
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let paused = status.position.load(Acquire);
    assert!(paused > 0 && !status.playing.load(Acquire));

    h.run(limiter());
    (applied, _) = h.applied(applied);
    let added = h.run(Request::Edit(hex_project::Command::InsertTrackWithDevice {
        name: "keys".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap(),
    }));
    (applied, _) = h.applied(applied);
    let (keys, instrument) = added
        .state
        .project
        .tracks()
        .iter()
        .find_map(|track| Some((track.id(), track.instrument()?.id())))
        .unwrap();
    let input = Arc::new(hex_engine::live_expression::LiveExpressionInput::new(
        0, keys, instrument,
    ));
    let bound = h.run(Request::PrepareLiveExpression {
        input: Arc::clone(&input),
        revision: added.state.revision,
    });
    assert!(Arc::ptr_eq(
        bound.live_expression_complete.as_ref().unwrap(),
        &input
    ));
    (applied, _) = h.applied(applied);
    // Pulse #3772: the limiter's PDC latency no longer disables the computer
    // keyboard on another track.
    let keyboard = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: added.state.revision,
        request_id: 1,
        track: keys,
        input: Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default()),
    });
    assert_eq!(keyboard.computer_keyboard_complete, Some(1));
    h.applied(applied);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert_eq!(
        status.position.load(Acquire),
        paused,
        "paused cursor is kept"
    );
    assert!(!status.playing.load(Acquire));
}
