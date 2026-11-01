//! Regression for Vector B edits, restarted live handoff and explicit Stop recovery.
use super::*;
use hex_engine::{NativeDeviceClass, playback::PlaybackCommand};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

fn next(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .expect("worker response")
}
fn submit(service: &Service, request: Request) -> Event {
    service.submit(request).expect("request admitted");
    next(service)
}
fn patch(
    service: &Service,
    device: hex_project::DeviceId,
    id: u32,
    value: ParameterValue,
) -> Event {
    submit(
        service,
        Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(id),
            value,
        },
    )
}
fn render(playback: &mut Playback) -> Vec<hex_engine::Frame> {
    let mut output = vec![[0.0; 2]; 512];
    playback.render(&mut output).expect("render");
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    output
}

#[test]
fn refused_compile_preserves_canonical_state_history_and_active_sound() {
    // The default sine instrument still refuses pitches at or above nyquist.
    let service = Service::new(8_000, 16, 0).expect("service");
    next(&service);
    let added = submit(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let clip = submit(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_raw(4 << 32),
        }),
    )
    .state
    .project
    .tracks()[0]
        .midi_clips()[0]
        .id();
    let note = |pitch| {
        Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_raw(2 << 32),
            pitch,
            channel: 0,
            velocity: 100,
        })
    };
    let before = submit(&service, note(60));
    assert!(before.error.is_none(), "{:?}", before.error);
    let canonical = Arc::clone(&before.state);
    let mut active = before.playback.unwrap();
    active.apply(PlaybackCommand::Play);
    render(&mut active);

    let refused = submit(&service, note(127));
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|reason| reason.contains("cannot be prepared by its instrument")),
        "{:?}",
        refused.error
    );
    assert!(refused.playback.is_none());
    assert_eq!(refused.state.project, canonical.project);
    assert_eq!(refused.state.revision, canonical.revision);
    assert_eq!(refused.state.can_undo, canonical.can_undo);
    render(&mut active);
}

#[test]
fn vector_b_tuning_edits_keep_canonical_sound_and_stop_can_restore_a_restarted_handoff() {
    let service = Service::new(48_000, 16, 0).expect("service");
    next(&service);
    let added = submit(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "vector".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let device = added.state.project.tracks()[0].instrument().unwrap().id();
    let replaced = submit(
        &service,
        Request::ReplaceNativeDevice {
            device,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(DeviceTypeId::from_static("hex.instrument.operator")),
        },
    );
    assert!(replaced.error.is_none());
    assert!(
        patch(&service, device, 61, ParameterValue::Choice(1))
            .error
            .is_none()
    ); // serial FM
    // New patches start with silent modulators. Raise each serial stage so
    // every later level edit changes the audible FM signal.
    for level in [5, 8, 11] {
        assert!(
            patch(&service, device, level, ParameterValue::Float(1.0))
                .error
                .is_none()
        );
    }
    assert!(
        patch(&service, device, 20_001, ParameterValue::Bool(true))
            .error
            .is_none()
    ); // mutable filter
    let clip = submit(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_raw(4 << 32),
        }),
    )
    .state
    .project
    .tracks()[0]
        .midi_clips()[0]
        .id();
    let before = submit(
        &service,
        Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_raw(2 << 32),
            pitch: 69,
            channel: 0,
            velocity: 100,
        }),
    );
    assert!(before.error.is_none(), "{:?}", before.error);
    let canonical = Arc::clone(&before.state);
    let mut active = before.playback.unwrap();
    active.apply(PlaybackCommand::Play);
    render(&mut active);

    // B ratio 128 exceeds Nyquist for this authored A4. Only that oscillator
    // is silenced (Pulse #3957): the edit compiles and the note still sounds.
    let silenced = patch(&service, device, 4, ParameterValue::Float(128.0));
    assert!(silenced.error.is_none(), "{:?}", silenced.error);
    assert_ne!(silenced.state.revision, canonical.revision);
    let mut silenced = silenced.playback.expect("above-nyquist edit compiles");
    silenced.apply(PlaybackCommand::Play);
    render(&mut silenced);
    render(&mut active);

    // A supported B ratio compiles. Its filter histories cannot carry over, so
    // the live handoff restarts audio at the cursor instead of refusing
    // (Pulse #3784): the replacement takes the runtime and keeps sounding.
    let edited = patch(&service, device, 4, ParameterValue::Float(3.0));
    assert!(edited.error.is_none(), "{:?}", edited.error);
    let mut active = {
        let mut candidate = edited.playback.unwrap();
        candidate
            .try_inherit_runtime_from(&mut active)
            .expect("unavailable source history restarts at the cursor");
        candidate
    };
    render(&mut active);

    active.apply(PlaybackCommand::Stop);
    let restored = submit(&service, Request::RestoreStoppedPlayback);
    assert!(restored.stopped_playback_restore_complete);
    assert!(restored.error.is_none());
    assert!(restored.restore_complete.is_none()); // independent of device recovery
    assert_eq!(restored.state.project, edited.state.project);
    assert_eq!(restored.state.revision, edited.state.revision);
    assert_eq!(restored.state.can_undo, edited.state.can_undo);
    let mut restored = restored.playback.unwrap();
    let mut oracle =
        Playback::try_from_prepared_with_session(Arc::clone(restored.prepared_revision().unwrap()))
            .unwrap();
    restored
        .try_inherit_runtime_from(&mut active)
        .expect("Stop hands over the restarted runtime");
    restored.apply(PlaybackCommand::Play);
    oracle.apply(PlaybackCommand::Play);
    assert_eq!(render(&mut restored), render(&mut oracle));

    // Each connected B/C/D level changes the serial FM signal independently
    // of the earlier restarted tuning handoff.
    for level in [5, 8, 11] {
        let leveled = patch(&service, device, level, ParameterValue::Float(0.0));
        assert!(leveled.error.is_none());
        let mut muted = leveled.playback.unwrap();
        muted.apply(PlaybackCommand::Play);
        let mut fm = Playback::try_from_prepared_with_session(Arc::clone(
            oracle.prepared_revision().unwrap(),
        ))
        .unwrap();
        fm.apply(PlaybackCommand::Play);
        assert_ne!(render(&mut muted), render(&mut fm));
        assert!(
            patch(&service, device, level, ParameterValue::Float(1.0))
                .error
                .is_none()
        );
    }
}
