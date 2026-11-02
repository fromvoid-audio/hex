//! Explicit hierarchy destinations through the captured session request.
use hex_lattice_contract::ParameterValue;
use hex_project::{Command, Created, TrackDestination, TrackId, TrackSelectionIds};
use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget};
use hex_session::service::{Event, Request, Service};
use std::{sync::mpsc::TryRecvError, time::Instant};

fn next(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(1))
            }
            Err(error) => panic!("worker event missing: {error}"),
        }
    }
}
fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn edit(service: &Service, command: Command) -> Event {
    run(service, Request::Edit(command))
}
fn track(event: &Event) -> TrackId {
    let Some(Created::Track(id)) = event.created else {
        panic!("track")
    };
    id
}
fn request(revision: u64, tracks: &[TrackId], destination: TrackDestination) -> Request {
    Request::ReparentTracks {
        project_generation: 0,
        revision,
        tracks: TrackSelectionIds::new(tracks).unwrap(),
        destination,
    }
}

#[test]
fn explicit_empty_group_destination_is_atomic_and_history_safe() {
    let service = Service::new(48_000, 32, 0).unwrap();
    assert!(next(&service).error.is_none());
    let seed = track(&edit(
        &service,
        Command::AddTrack {
            name: "seed".into(),
        },
    ));
    let source = track(&edit(
        &service,
        Command::AddTrack {
            name: "source".into(),
        },
    ));
    let group = track(&edit(
        &service,
        Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[seed]).unwrap(),
            name: "group".into(),
        },
    ));
    let before = edit(&service, Command::RemoveTrack(seed)).state;
    assert_eq!(before.project.track(source).unwrap().parent_group(), None);
    let moved = run(
        &service,
        request(
            before.revision,
            &[source],
            TrackDestination {
                parent_group: Some(group),
                before: None,
            },
        ),
    );
    assert_eq!(
        moved.state.project.track(source).unwrap().parent_group(),
        Some(group)
    );
    assert_eq!(
        moved
            .state
            .project
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>(),
        [group, source]
    );
    let undo = run(&service, Request::Undo);
    assert_eq!(undo.state.project, before.project);
    let redo = run(&service, Request::Redo);
    assert_eq!(redo.state.project, moved.state.project);

    for bad in [
        Request::ReparentTracks {
            project_generation: 1,
            revision: redo.state.revision,
            tracks: TrackSelectionIds::new(&[source]).unwrap(),
            destination: TrackDestination {
                parent_group: None,
                before: None,
            },
        },
        request(
            redo.state.revision,
            &[source],
            TrackDestination {
                parent_group: Some(group),
                before: Some(group),
            },
        ),
        request(
            redo.state.revision,
            &[group],
            TrackDestination {
                parent_group: Some(group),
                before: None,
            },
        ),
        request(
            redo.state.revision,
            &[group],
            TrackDestination {
                parent_group: Some(source),
                before: None,
            },
        ),
        request(
            before.revision,
            &[source],
            TrackDestination {
                parent_group: None,
                before: None,
            },
        ),
    ] {
        service.submit(bad).unwrap();
        let refused = next(&service);
        assert!(refused.error.is_some());
        assert!(refused.request_complete && refused.playback.is_none());
        assert_eq!(refused.state.project, redo.state.project);
        assert_eq!(refused.state.revision, redo.state.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (redo.state.can_undo, redo.state.can_redo)
        );
    }
    let key = GestureKey {
        generation: 0,
        token: 71,
    };
    let begun = run(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track: source },
        }),
    );
    assert!(begun.gesture_ack.unwrap().accepted);
    let draft = run(
        &service,
        Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.25),
        }),
    );
    assert!(draft.gesture_ack.unwrap().accepted);
    service
        .submit(request(
            redo.state.revision,
            &[source],
            TrackDestination {
                parent_group: None,
                before: None,
            },
        ))
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.as_deref().unwrap().contains("active gesture"));
    assert!(refused.active_device_gesture.is_some());
    assert_eq!(refused.state.project, redo.state.project);
    assert_eq!(refused.state.revision, redo.state.revision);
    assert!(refused.playback.is_none());
    assert!(
        run(
            &service,
            Request::DeviceGesture(GestureRequest::Cancel { key })
        )
        .gesture_ack
        .unwrap()
        .accepted
    );
    let nested_event = edit(
        &service,
        Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[source]).unwrap(),
            name: "nested".into(),
        },
    );
    let nested = track(&nested_event);
    let snapshot = nested_event.state;
    service
        .submit(request(
            snapshot.revision,
            &[group],
            TrackDestination {
                parent_group: Some(nested),
                before: None,
            },
        ))
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.is_some());
    assert!(refused.request_complete && refused.playback.is_none());
    assert_eq!(refused.state.project, snapshot.project);
    assert_eq!(refused.state.revision, snapshot.revision);
}
