//! The service resolves a captured source choice before an edit or gesture barrier.
use super::service_tests::next_event;
use super::*;
use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
use hex_project::{Created, TrackDeviceSlot};

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

fn assign(
    revision: u64,
    generation: u64,
    track: hex_project::TrackId,
    source: hex_project::SourceId,
) -> Request {
    Request::BindUnassignedNativeSource {
        project_generation: generation,
        revision,
        track,
        source,
    }
}

fn refused(service: &Service, before: &Event, request: Request, message: &str) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(
        event
            .error
            .as_deref()
            .is_some_and(|error| error.contains(message)),
        "{:?}",
        event.error
    );
    assert_eq!(event.state.revision, before.state.revision);
    assert_eq!(event.state.project, before.state.project);
    assert_eq!(event.state.can_undo, before.state.can_undo);
    assert_eq!(event.state.can_redo, before.state.can_redo);
    event
}

#[test]
fn captured_unassigned_source_binding_refuses_stale_and_preserves_history() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let receiver_event = run(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }),
    );
    let Created::Track(receiver) = receiver_event.created.unwrap() else {
        panic!("receiver")
    };
    let source = receiver_event
        .state
        .project
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let target_event = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "target".into(),
        }),
    );
    let Created::Track(target) = target_event.created.unwrap() else {
        panic!("target")
    };
    let captured_revision = target_event.state.revision;
    assert!(!assign(captured_revision, 0, target, source).finalizes_parameter_gesture());
    let renamed = run(
        &service,
        Request::Edit(Command::RenameTrack {
            track: target,
            name: "renamed".into(),
        }),
    );
    refused(
        &service,
        &renamed,
        assign(captured_revision, 0, target, source),
        "project changed",
    );
    refused(
        &service,
        &renamed,
        assign(renamed.state.revision, 1, target, source),
        "project changed",
    );
    refused(
        &service,
        &renamed,
        assign(renamed.state.revision, 0, receiver, source),
        "already has a source",
    );

    let temporary = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "temporary".into(),
        }),
    );
    let Created::Track(deleted_track) = temporary.created.unwrap() else {
        panic!("temporary")
    };
    let removed = run(&service, Request::Edit(Command::RemoveTrack(deleted_track)));
    refused(
        &service,
        &removed,
        assign(removed.state.revision, 0, deleted_track, source),
        "track no longer exists",
    );
    let temporary_source = run(
        &service,
        Request::Edit(Command::CreateNativeSource {
            name: "temporary".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }),
    );
    let Created::NativeSource(deleted_source) = temporary_source.created.unwrap() else {
        panic!("temporary source")
    };
    let before = run(
        &service,
        Request::Edit(Command::RemoveNativeSource(deleted_source)),
    );
    refused(
        &service,
        &before,
        assign(before.state.revision, 0, target, deleted_source),
        "source no longer exists",
    );

    let key = GestureKey {
        generation: 0,
        token: 7613,
    };
    let begun = run(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track: target },
        }),
    );
    let blocked = refused(
        &service,
        &begun,
        assign(begun.state.revision, 0, target, source),
        "active gesture",
    );
    assert!(blocked.active_device_gesture.is_some());
    assert!(blocked.gesture_ack.is_none());
    let cancelled = run(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    let before_bind = cancelled.state.project.clone();
    let bound = run(
        &service,
        assign(cancelled.state.revision, 0, target, source),
    );
    assert_eq!(
        bound.state.project.track(target).unwrap().source_id(),
        Some(source)
    );
    assert_eq!(
        bound
            .state
            .project
            .native_source(source)
            .unwrap()
            .receiver(),
        Some(receiver)
    );
    assert_eq!(bound.state.project.source_contributors(source).count(), 2);
    let after_bind = bound.state.project.clone();
    let undone = run(&service, Request::Undo);
    assert_eq!(undone.state.project, before_bind);
    let redone = run(&service, Request::Redo);
    assert_eq!(redone.state.project, after_bind);

    // The first assignment of an unassigned source chooses this track as its
    // receiver through the existing project command, not a second UI model.
    let unused = run(
        &service,
        Request::Edit(Command::CreateNativeSource {
            name: "unused".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }),
    );
    let Created::NativeSource(unused_source) = unused.created.unwrap() else {
        panic!("unused source")
    };
    let new_track = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "first".into(),
        }),
    );
    let Created::Track(first) = new_track.created.unwrap() else {
        panic!("first")
    };
    let assigned = run(
        &service,
        assign(new_track.state.revision, 0, first, unused_source),
    );
    assert_eq!(
        assigned.state.project.track(first).unwrap().source_id(),
        Some(unused_source)
    );
    assert_eq!(
        assigned
            .state
            .project
            .native_source(unused_source)
            .unwrap()
            .receiver(),
        Some(first)
    );
    assert_eq!(
        assigned
            .state
            .project
            .source_contributors(unused_source)
            .count(),
        1
    );
}
