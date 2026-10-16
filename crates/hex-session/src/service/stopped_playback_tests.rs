//! Regression for Vector B edits, rejected live handoff and explicit Stop recovery.
use super::*;
use hex_engine::{NativeDeviceClass, playback::PlaybackCommand};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use std::time::Duration;

fn next(service: &Service) -> Event {
    service
        .events
        .recv_timeout(Duration::from_secs(10))
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
fn vector_b_tuning_refusal_preserves_canonical_sound_and_stop_can_restore_a_refused_handoff() {
    let service = Service::new(48_000, 16, 0).expect("service");
    next(&service);
    let added = submit(
        &service,
        Request::Edit(Command::AddTrack {
            name: "vector".into(),
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

    // B ratio 128 exceeds Nyquist for this authored A4. Compilation must leave
    // the accepted state/history intact and the active graph still renderable.
    let refused = patch(&service, device, 4, ParameterValue::Float(128.0));
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|reason| reason.contains("mutable instrument note is unsupported"))
    );
    assert!(refused.playback.is_none());
    assert_eq!(refused.state.project, canonical.project);
    assert_eq!(refused.state.revision, canonical.revision);
    assert_eq!(refused.state.can_undo, canonical.can_undo);
    render(&mut active);

    // A supported B ratio compiles but cannot replace active filter histories.
    let edited = patch(&service, device, 4, ParameterValue::Float(3.0));
    assert!(edited.error.is_none(), "{:?}", edited.error);
    let mut candidate = edited.playback.unwrap();
    assert!(candidate.try_inherit_runtime_from(&mut active).is_err());
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
        .expect("Stop releases incompatible histories");
    restored.apply(PlaybackCommand::Play);
    oracle.apply(PlaybackCommand::Play);
    assert_eq!(render(&mut restored), render(&mut oracle));

    // Each connected B/C/D level changes the FM signal. The default custom
    // topology leaves them disconnected, which is separate from handoff failure.
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
