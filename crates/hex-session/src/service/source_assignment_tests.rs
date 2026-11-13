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

fn rebind(
    revision: u64,
    generation: u64,
    track: hex_project::TrackId,
    expected_source: Option<hex_project::SourceId>,
    source: Option<hex_project::SourceId>,
) -> Request {
    Request::RebindNativeSource {
        project_generation: generation,
        revision,
        track,
        expected_source,
        source,
    }
}

fn choose_receiver(
    revision: u64,
    generation: u64,
    source: hex_project::SourceId,
    expected_receiver: Option<hex_project::TrackId>,
    track: hex_project::TrackId,
) -> Request {
    Request::ChooseNativeSourceReceiver {
        project_generation: generation,
        revision,
        source,
        expected_receiver,
        track,
    }
}

fn rename_source(
    revision: u64,
    generation: u64,
    source: hex_project::SourceId,
    name: &str,
) -> Request {
    Request::RenameRetainedSource {
        project_generation: generation,
        revision,
        source,
        name: name.into(),
    }
}

fn delete_source(revision: u64, generation: u64, source: hex_project::SourceId) -> Request {
    Request::DeleteRetainedSource {
        project_generation: generation,
        revision,
        source,
    }
}

#[test]
fn captured_source_rename_and_delete_preserve_identity_and_history() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let bound = run(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }),
    );
    let Created::Track(track) = bound.created.unwrap() else {
        panic!("track")
    };
    let bound_source = bound
        .state
        .project
        .track(track)
        .unwrap()
        .source_id()
        .unwrap();
    let created = run(
        &service,
        Request::Edit(Command::CreateNativeSource {
            name: "unused".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }),
    );
    let Created::NativeSource(source) = created.created.unwrap() else {
        panic!("source")
    };
    let revision = created.state.revision;
    assert!(!rename_source(revision, 0, source, "renamed").finalizes_parameter_gesture());
    assert!(!delete_source(revision, 0, source).finalizes_parameter_gesture());
    refused(
        &service,
        &created,
        rename_source(revision, 1, source, "renamed"),
        "project changed",
    );
    refused(
        &service,
        &created,
        delete_source(revision - 1, 0, source),
        "project changed",
    );
    refused(
        &service,
        &created,
        rename_source(revision, 0, source, "  "),
        "name must be nonempty",
    );
    refused(
        &service,
        &created,
        delete_source(revision, 0, bound_source),
        "still assigned",
    );

    let key = GestureKey {
        generation: 0,
        token: 7712,
    };
    let begun = run(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }),
    );
    let blocked = refused(
        &service,
        &begun,
        rename_source(begun.state.revision, 0, source, "renamed"),
        "active gesture",
    );
    assert!(blocked.active_device_gesture.is_some() && blocked.gesture_ack.is_none());
    refused(
        &service,
        &begun,
        delete_source(begun.state.revision, 0, source),
        "active gesture",
    );
    let cancelled = run(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    let before = cancelled.state.project.clone();
    let renamed = run(
        &service,
        rename_source(cancelled.state.revision, 0, source, "renamed"),
    );
    assert_eq!(
        renamed.state.project.native_source(source).unwrap().name(),
        "renamed"
    );
    let after_rename = renamed.state.project.clone();
    assert_eq!(run(&service, Request::Undo).state.project, before);
    let redone = run(&service, Request::Redo);
    assert_eq!(redone.state.project, after_rename);
    let deleted = run(&service, delete_source(redone.state.revision, 0, source));
    assert!(deleted.state.project.native_source(source).is_none());
    assert_eq!(
        deleted.state.project.track(track).unwrap().source_id(),
        Some(bound_source)
    );
    refused(
        &service,
        &deleted,
        rename_source(deleted.state.revision, 0, source, "missing"),
        "source no longer exists",
    );
    refused(
        &service,
        &deleted,
        delete_source(deleted.state.revision, 0, source),
        "source no longer exists",
    );
    assert_eq!(run(&service, Request::Undo).state.project, after_rename);
    assert!(
        run(&service, Request::Redo)
            .state
            .project
            .native_source(source)
            .is_none()
    );
}

#[test]
fn captured_receiver_choice_refuses_stale_and_preserves_history() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let first = run(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }),
    );
    let Created::Track(receiver) = first.created.unwrap() else {
        panic!("receiver")
    };
    let source = first
        .state
        .project
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let added = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "contributor".into(),
        }),
    );
    let Created::Track(contributor) = added.created.unwrap() else {
        panic!("contributor")
    };
    let assigned = run(
        &service,
        Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }),
    );
    let revision = assigned.state.revision;
    assert!(
        !choose_receiver(revision, 0, source, Some(receiver), contributor)
            .finalizes_parameter_gesture()
    );
    refused(
        &service,
        &assigned,
        choose_receiver(revision, 1, source, Some(receiver), contributor),
        "project changed",
    );
    refused(
        &service,
        &assigned,
        choose_receiver(revision - 1, 0, source, Some(receiver), contributor),
        "project changed",
    );
    refused(
        &service,
        &assigned,
        choose_receiver(revision, 0, source, None, contributor),
        "receiver changed",
    );
    refused(
        &service,
        &assigned,
        choose_receiver(revision, 0, source, Some(receiver), receiver),
        "unchanged",
    );

    let unlinked = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "unlinked".into(),
        }),
    );
    let Created::Track(unlinked_track) = unlinked.created.unwrap() else {
        panic!("track")
    };
    refused(
        &service,
        &unlinked,
        choose_receiver(
            unlinked.state.revision,
            0,
            source,
            Some(receiver),
            unlinked_track,
        ),
        "not linked",
    );
    let removed = run(
        &service,
        Request::Edit(Command::RemoveTrack(unlinked_track)),
    );
    refused(
        &service,
        &removed,
        choose_receiver(
            removed.state.revision,
            0,
            source,
            Some(receiver),
            unlinked_track,
        ),
        "track no longer exists",
    );
    let temporary = run(
        &service,
        Request::Edit(Command::CreateNativeSource {
            name: "temporary".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }),
    );
    let Created::NativeSource(missing) = temporary.created.unwrap() else {
        panic!("source")
    };
    let removed = run(
        &service,
        Request::Edit(Command::RemoveNativeSource(missing)),
    );
    refused(
        &service,
        &removed,
        choose_receiver(removed.state.revision, 0, missing, None, contributor),
        "source no longer exists",
    );

    let key = GestureKey {
        generation: 0,
        token: 7710,
    };
    let begun = run(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track: contributor },
        }),
    );
    let blocked = refused(
        &service,
        &begun,
        choose_receiver(begun.state.revision, 0, source, Some(receiver), contributor),
        "active gesture",
    );
    assert!(blocked.active_device_gesture.is_some());
    assert!(blocked.gesture_ack.is_none());
    let cancelled = run(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    let before = cancelled.state.project.clone();
    let changed = run(
        &service,
        choose_receiver(
            cancelled.state.revision,
            0,
            source,
            Some(receiver),
            contributor,
        ),
    );
    assert_eq!(
        changed
            .state
            .project
            .native_source(source)
            .unwrap()
            .receiver(),
        Some(contributor)
    );
    assert_eq!(
        changed.state.project.track(receiver).unwrap().source_id(),
        Some(source)
    );
    assert_eq!(
        changed
            .state
            .project
            .track(contributor)
            .unwrap()
            .source_id(),
        Some(source)
    );
    let after = changed.state.project.clone();
    assert_eq!(run(&service, Request::Undo).state.project, before);
    assert_eq!(run(&service, Request::Redo).state.project, after);
}

#[test]
fn captured_rebinding_preserves_shared_receiver_and_history() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let first = run(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }),
    );
    let Created::Track(receiver) = first.created.unwrap() else {
        panic!("receiver")
    };
    let old = first
        .state
        .project
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let second = run(
        &service,
        Request::Edit(Command::CreateNativeSource {
            name: "other".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }),
    );
    let Created::NativeSource(new) = second.created.unwrap() else {
        panic!("source")
    };
    let added = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "contributor".into(),
        }),
    );
    let Created::Track(contributor) = added.created.unwrap() else {
        panic!("contributor")
    };
    let local = run(
        &service,
        Request::Edit(Command::InsertDevice {
            track: contributor,
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        }),
    );
    let local_devices = local
        .state
        .project
        .track(contributor)
        .unwrap()
        .devices()
        .to_vec();
    let assigned = run(
        &service,
        rebind(local.state.revision, 0, contributor, None, Some(old)),
    );
    assert_eq!(assigned.state.project.source_contributors(old).count(), 2);
    assert_eq!(
        assigned
            .state
            .project
            .native_source(old)
            .unwrap()
            .receiver(),
        Some(receiver)
    );
    assert!(
        !rebind(
            assigned.state.revision,
            0,
            contributor,
            Some(old),
            Some(new)
        )
        .finalizes_parameter_gesture()
    );

    refused(
        &service,
        &assigned,
        rebind(
            assigned.state.revision,
            1,
            contributor,
            Some(old),
            Some(new),
        ),
        "project changed",
    );
    refused(
        &service,
        &assigned,
        rebind(
            assigned.state.revision.saturating_sub(1),
            0,
            contributor,
            Some(old),
            Some(new),
        ),
        "project changed",
    );
    refused(
        &service,
        &assigned,
        rebind(assigned.state.revision, 0, contributor, None, Some(new)),
        "binding changed",
    );
    refused(
        &service,
        &assigned,
        rebind(assigned.state.revision, 0, receiver, Some(old), None),
        "choose another receiver",
    );
    refused(
        &service,
        &assigned,
        rebind(assigned.state.revision, 0, receiver, Some(old), Some(new)),
        "choose another receiver",
    );
    refused(
        &service,
        &assigned,
        rebind(
            assigned.state.revision,
            0,
            contributor,
            Some(old),
            Some(old),
        ),
        "unchanged",
    );

    let unlinked_contributor = run(
        &service,
        rebind(assigned.state.revision, 0, contributor, Some(old), None),
    );
    assert_eq!(
        unlinked_contributor
            .state
            .project
            .track(contributor)
            .unwrap()
            .source_id(),
        None
    );
    assert_eq!(
        unlinked_contributor
            .state
            .project
            .native_source(old)
            .unwrap()
            .receiver(),
        Some(receiver)
    );
    assert_eq!(
        unlinked_contributor
            .state
            .project
            .source_contributors(old)
            .count(),
        1
    );
    assert_eq!(
        run(&service, Request::Undo).state.project,
        assigned.state.project
    );

    // Moving the receiver is an explicit edit. The former receiver can then
    // leave without silencing the contributor that now owns the output.
    let chosen = run(
        &service,
        Request::Edit(Command::SetNativeSourceReceiver {
            source: old,
            track: Some(contributor),
        }),
    );
    let moved = run(
        &service,
        rebind(chosen.state.revision, 0, receiver, Some(old), Some(new)),
    );
    assert_eq!(
        moved.state.project.native_source(old).unwrap().receiver(),
        Some(contributor)
    );
    assert_eq!(
        moved.state.project.native_source(new).unwrap().receiver(),
        Some(receiver)
    );
    assert_eq!(moved.state.project.source_contributors(old).count(), 1);
    let undone_move = run(&service, Request::Undo);
    assert_eq!(undone_move.state.project, chosen.state.project);
    let undone_receiver = run(&service, Request::Undo);
    assert_eq!(undone_receiver.state.project, assigned.state.project);

    let temp = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "deleted".into(),
        }),
    );
    let Created::Track(deleted) = temp.created.unwrap() else {
        panic!("deleted track")
    };
    let removed = run(&service, Request::Edit(Command::RemoveTrack(deleted)));
    refused(
        &service,
        &removed,
        rebind(removed.state.revision, 0, deleted, None, Some(new)),
        "track no longer exists",
    );
    let temp = run(
        &service,
        Request::Edit(Command::CreateNativeSource {
            name: "deleted".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }),
    );
    let Created::NativeSource(deleted_source) = temp.created.unwrap() else {
        panic!("deleted source")
    };
    let removed = run(
        &service,
        Request::Edit(Command::RemoveNativeSource(deleted_source)),
    );
    refused(
        &service,
        &removed,
        rebind(
            removed.state.revision,
            0,
            contributor,
            Some(old),
            Some(deleted_source),
        ),
        "source no longer exists",
    );

    let key = GestureKey {
        generation: 0,
        token: 7634,
    };
    let begun = run(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track: contributor },
        }),
    );
    let blocked = refused(
        &service,
        &begun,
        rebind(begun.state.revision, 0, contributor, Some(old), Some(new)),
        "active gesture",
    );
    assert!(blocked.active_device_gesture.is_some());
    assert!(blocked.gesture_ack.is_none());
    let cancelled = run(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    let before = cancelled.state.project.clone();
    let changed = run(
        &service,
        rebind(
            cancelled.state.revision,
            0,
            contributor,
            Some(old),
            Some(new),
        ),
    );
    assert_eq!(
        changed
            .state
            .project
            .track(contributor)
            .unwrap()
            .source_id(),
        Some(new)
    );
    assert_eq!(
        changed.state.project.track(contributor).unwrap().devices(),
        local_devices
    );
    assert_eq!(
        changed.state.project.native_source(old).unwrap().receiver(),
        Some(receiver)
    );
    assert_eq!(
        changed.state.project.native_source(new).unwrap().receiver(),
        Some(contributor)
    );
    let after = changed.state.project.clone();
    assert_eq!(run(&service, Request::Undo).state.project, before);
    let redone = run(&service, Request::Redo);
    assert_eq!(redone.state.project, after);
    let unlinked = run(
        &service,
        rebind(redone.state.revision, 0, contributor, Some(new), None),
    );
    assert_eq!(
        unlinked
            .state
            .project
            .track(contributor)
            .unwrap()
            .source_id(),
        None
    );
    assert_eq!(
        unlinked
            .state
            .project
            .native_source(new)
            .unwrap()
            .receiver(),
        None
    );
    assert!(unlinked.state.project.native_source(new).is_some());
    let replaced = run(
        &service,
        rebind(unlinked.state.revision, 0, receiver, Some(old), Some(new)),
    );
    assert_eq!(
        replaced
            .state
            .project
            .native_source(old)
            .unwrap()
            .receiver(),
        None
    );
    assert_eq!(
        replaced
            .state
            .project
            .native_source(new)
            .unwrap()
            .receiver(),
        Some(receiver)
    );
    let restored = run(&service, Request::Undo);
    assert_eq!(restored.state.project, unlinked.state.project);
    let cleared = run(
        &service,
        rebind(restored.state.revision, 0, receiver, Some(old), None),
    );
    assert_eq!(
        cleared.state.project.track(receiver).unwrap().source_id(),
        None
    );
    assert_eq!(
        cleared.state.project.native_source(old).unwrap().receiver(),
        None
    );
    assert!(cleared.state.project.native_source(old).is_some());
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
