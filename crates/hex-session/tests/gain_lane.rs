//! Revision-stamped UI gain edits exercise the real control worker and history.
use hex_project::{BeatTime, Command, Created, GainInterpolation, GainPoint};
use hex_session::service::{Event, GainLaneEdit, Request, Service};
use std::time::Instant;
fn event(service: &Service) -> Event {
    let until = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(1))
            }
            Err(reason) => panic!("event: {reason}"),
        }
    }
}
#[test]
fn queued_stale_revision_and_reused_generation_refuse_before_history_and_preview_publication() {
    let service = Service::new(48000, 16, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "gain".into(),
        }))
        .unwrap();
    let added = event(&service);
    let Created::Track(track) = added.created.unwrap() else {
        panic!("track")
    };
    let revision = added.state.revision;
    let point = GainPoint {
        incoming_gain: None,
        beat: BeatTime::from_raw(1),
        gain: 0.25,
        outgoing: GainInterpolation::Linear,
    };
    service
        .submit(Request::GainLane {
            project_generation: 0,
            revision,
            track,
            edit: GainLaneEdit::Upsert(point),
        })
        .unwrap();
    // This request can already be waiting when the first one commits.
    service
        .submit(Request::GainLane {
            project_generation: 0,
            revision,
            track,
            edit: GainLaneEdit::Clear,
        })
        .unwrap();
    let committed = event(&service);
    assert!(committed.error.is_none());
    assert_eq!(
        committed
            .state
            .project
            .track(track)
            .unwrap()
            .gain_envelope(),
        [point]
    );
    assert_eq!(
        committed
            .state
            .gain_preview
            .as_ref()
            .unwrap()
            .value_at(track, 0),
        Some(0.25)
    );
    let stale = event(&service);
    assert!(stale.error.unwrap().contains("project changed"));
    assert!(stale.playback.is_none());
    assert_eq!(stale.state.revision, committed.state.revision);
    assert_eq!(stale.state.project, committed.state.project);
    service
        .submit(Request::GainLane {
            project_generation: 1,
            revision: stale.state.revision,
            track,
            edit: GainLaneEdit::Clear,
        })
        .unwrap();
    let stale_generation = event(&service);
    assert!(stale_generation.error.unwrap().contains("project changed"));
    assert!(stale_generation.playback.is_none());
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert!(
        undone
            .state
            .project
            .track(track)
            .unwrap()
            .gain_envelope()
            .is_empty()
    );
    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert_eq!(
        redone.state.project.track(track).unwrap().gain_envelope(),
        [point]
    );
    // Clear is a single undoable command, not a static-gain mutation.
    service
        .submit(Request::GainLane {
            project_generation: 0,
            revision: redone.state.revision,
            track,
            edit: GainLaneEdit::Clear,
        })
        .unwrap();
    let cleared = event(&service);
    assert!(
        cleared
            .state
            .project
            .track(track)
            .unwrap()
            .gain_envelope()
            .is_empty()
    );
    assert_eq!(
        cleared.state.project.track(track).unwrap().gain(),
        redone.state.project.track(track).unwrap().gain()
    );
    service.submit(Request::Undo).unwrap();
    assert_eq!(
        event(&service)
            .state
            .project
            .track(track)
            .unwrap()
            .gain_envelope(),
        [point]
    );
}
#[test]
fn occupied_move_and_removed_owner_preserve_exact_committed_history() {
    let service = Service::new(48000, 16, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "gain".into(),
        }))
        .unwrap();
    let added = event(&service);
    let Created::Track(track) = added.created.unwrap() else {
        panic!("track")
    };
    let points = vec![
        GainPoint {
            incoming_gain: None,
            beat: BeatTime::from_raw(1),
            gain: 0.25,
            outgoing: GainInterpolation::Step,
        },
        GainPoint {
            incoming_gain: None,
            beat: BeatTime::from_raw(2),
            gain: 0.75,
            outgoing: GainInterpolation::Linear,
        },
    ];
    service
        .submit(Request::GainLane {
            project_generation: 0,
            revision: added.state.revision,
            track,
            edit: GainLaneEdit::Replace(points.clone()),
        })
        .unwrap();
    let committed = event(&service);
    service
        .submit(Request::GainLane {
            project_generation: 0,
            revision: committed.state.revision,
            track,
            edit: GainLaneEdit::Move {
                from: points[0].beat,
                to: points[1].beat,
            },
        })
        .unwrap();
    let refused = event(&service);
    assert!(refused.error.is_some());
    assert!(refused.playback.is_none());
    assert_eq!(refused.state.project, committed.state.project);
    assert_eq!(refused.state.revision, committed.state.revision);
    service
        .submit(Request::Edit(Command::RemoveTrack(track)))
        .unwrap();
    let removed = event(&service);
    service
        .submit(Request::GainLane {
            project_generation: 0,
            revision: removed.state.revision,
            track,
            edit: GainLaneEdit::Clear,
        })
        .unwrap();
    let no_owner = event(&service);
    assert!(no_owner.error.unwrap().contains("track no longer exists"));
    assert!(no_owner.playback.is_none());
    service.submit(Request::Undo).unwrap();
    assert_eq!(
        event(&service)
            .state
            .project
            .track(track)
            .unwrap()
            .gain_envelope(),
        points
    );
}

#[test]
fn arming_gain_recording_preserves_canonical_revision_dirty_history_and_cache() {
    let service = Service::new(48000, 16, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "record".into(),
        }))
        .unwrap();
    let added = event(&service);
    let Created::Track(track) = added.created.unwrap() else {
        panic!("track")
    };
    let revision = added.state.revision;
    let canonical = added.state.project.clone();
    for (generation, stamp) in [(1, revision), (0, revision + 1)] {
        service
            .submit(Request::ArmGainRecording {
                project_generation: generation,
                revision: stamp,
                track,
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.unwrap().contains("project changed"));
        assert!(refused.playback.is_none());
        assert_eq!(refused.state.project, canonical);
    }
    service
        .submit(Request::ArmGainRecording {
            project_generation: 0,
            revision,
            track,
        })
        .unwrap();
    let armed = event(&service);
    assert!(armed.error.is_none(), "{:?}", armed.error);
    assert_eq!(armed.state.revision, revision);
    assert_eq!(armed.state.project, canonical);
    assert_eq!(armed.state.unsaved, added.state.unsaved);
    assert_eq!(
        (armed.state.can_undo, armed.state.can_redo),
        (added.state.can_undo, added.state.can_redo)
    );
    let playback = armed.playback.unwrap();
    assert_eq!(
        playback.prepared_revision().unwrap().gain_recording_track(),
        Some(track)
    );
    assert!(playback.gain_recording_ready(track.value()));
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert!(
        undone.state.project.track(track).is_none(),
        "arming did not add an undo step"
    );
    assert_eq!(
        undone
            .playback
            .unwrap()
            .prepared_revision()
            .unwrap()
            .gain_recording_track(),
        None
    );
    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert_eq!(redone.state.project, canonical);
    assert_eq!(
        redone
            .playback
            .unwrap()
            .prepared_revision()
            .unwrap()
            .gain_recording_track(),
        None,
        "canonical cache stays unarmed"
    );
}
