//! Source-bound drag admission retains canonical validation and history ownership.
use super::service_tests::next_event;
use super::*;
use hex_project::BeatTime;

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

#[test]
fn clip_move_worker_refuses_stale_or_unrelated_commands_without_history_changes() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "track".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let placed = run(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }),
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let move_command = Command::MoveMidiClipWithOverwrite {
        clip,
        track,
        start: BeatTime::ONE,
    };
    for (generation, revision, command) in [
        (1, placed.state.revision, move_command.clone()),
        (0, placed.state.revision + 1, move_command.clone()),
        (
            1,
            placed.state.revision,
            Command::MoveMidiClipToNewTrack {
                clip,
                name: "new".into(),
                before_track: Some(track),
                start: BeatTime::ONE,
                initial_gain: Some(0.5),
            },
        ),
        (
            0,
            placed.state.revision + 1,
            Command::CopyMidiClipToNewTrack {
                clip,
                name: "new".into(),
                before_track: Some(track),
                start: BeatTime::ONE,
                initial_gain: Some(0.5),
            },
        ),
        (
            0,
            placed.state.revision,
            Command::RenameTrack {
                track,
                name: "wrong".into(),
            },
        ),
        (
            0,
            placed.state.revision,
            Command::MoveClipsWithOverwrite {
                clips: vec![],
                anchor: clip,
                track,
                position: hex_project::ClipGroupPosition::Midi(BeatTime::ONE),
                copy: false,
            },
        ),
        (
            0,
            placed.state.revision,
            Command::MoveClipsWithOverwrite {
                clips: vec![clip; hex_project::MAX_CLIPS + 1],
                anchor: clip,
                track,
                position: hex_project::ClipGroupPosition::Midi(BeatTime::ONE),
                copy: false,
            },
        ),
    ] {
        service
            .submit(Request::MoveExistingClips {
                project_generation: generation,
                revision,
                command,
            })
            .unwrap();
        let refused = next_event(&service);
        assert!(refused.error.is_some() && refused.request_complete);
        assert!(refused.created.is_none() && refused.playback.is_none());
        assert_eq!(refused.state.project, placed.state.project);
        assert_eq!(refused.state.revision, placed.state.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (placed.state.can_undo, placed.state.can_redo)
        );
    }
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    let key = GestureKey {
        generation: 0,
        token: 72,
    };
    run(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }),
    );
    service
        .submit(Request::MoveExistingClips {
            project_generation: 0,
            revision: placed.state.revision,
            command: move_command.clone(),
        })
        .unwrap();
    let refused = next_event(&service);
    assert!(refused.error.as_ref().unwrap().contains("active gesture"));
    assert!(refused.active_device_gesture.is_some());
    assert_eq!(refused.state.project, placed.state.project);
    assert_eq!(refused.state.revision, placed.state.revision);
    run(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    // A same-revision control request may precede the captured move.
    run(
        &service,
        Request::PrepareTuningAudition {
            revision: placed.state.revision,
            input: Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0)),
        },
    );
    let moved = run(
        &service,
        Request::MoveExistingClips {
            project_generation: 0,
            revision: placed.state.revision,
            command: move_command,
        },
    );
    assert_eq!(
        moved.state.project.midi_clip(clip).unwrap().start(),
        BeatTime::ONE
    );
    assert_eq!(moved.state.revision, placed.state.revision + 1);
    let undone = run(&service, Request::Undo);
    assert_eq!(undone.state.project, placed.state.project);
    let redone = run(&service, Request::Redo);
    assert_eq!(redone.state.project, moved.state.project);
}

#[test]
fn alias_destination_uses_source_bound_worker_and_one_undo_step() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "track".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let placed = run(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }),
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let command = Command::DuplicateAliasClipsAt {
        clips: vec![clip],
        anchor: clip,
        track,
        position: hex_project::ClipGroupPosition::Midi(BeatTime::from_quarters(2).unwrap()),
    };
    service
        .submit(Request::MoveExistingClips {
            project_generation: 1,
            revision: placed.state.revision,
            command: command.clone(),
        })
        .unwrap();
    let stale = next_event(&service);
    assert!(stale.error.is_some());
    assert_eq!(stale.state.project, placed.state.project);
    let result = run(
        &service,
        Request::MoveExistingClips {
            project_generation: 0,
            revision: placed.state.revision,
            command,
        },
    );
    let Some(Created::Clips { first: alias, .. }) = result.created else {
        panic!("alias")
    };
    assert_eq!(
        result.state.project.clip_alias(clip),
        result.state.project.clip_alias(alias)
    );
    let undone = run(&service, Request::Undo);
    assert!(undone.state.project.midi_clip(alias).is_none());
    assert!(undone.state.project.midi_clip(clip).is_some());
    let redone = run(&service, Request::Redo);
    assert_eq!(
        redone.state.project.clip_alias(clip),
        redone.state.project.clip_alias(alias)
    );
}

#[test]
fn new_track_alias_is_revision_checked_and_undoable_in_one_step() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "track".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let placed = run(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }),
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let command = Command::DuplicateAliasClipToNewTrack {
        initial_gain: Some(0.25),
        clip,
        name: "new".into(),
        before_track: None,
        position: hex_project::ClipGroupPosition::Midi(BeatTime::from_quarters(2).unwrap()),
    };
    service
        .submit(Request::MoveExistingClips {
            project_generation: 1,
            revision: placed.state.revision,
            command: command.clone(),
        })
        .unwrap();
    let stale = next_event(&service);
    assert!(stale.error.is_some());
    assert_eq!(stale.state.project, placed.state.project);
    let result = run(
        &service,
        Request::MoveExistingClips {
            project_generation: 0,
            revision: placed.state.revision,
            command,
        },
    );
    let Some(Created::Clips { first: alias, .. }) = result.created else {
        panic!("alias")
    };
    assert_eq!(result.state.project.tracks().len(), 2);
    assert_eq!(result.state.project.tracks()[1].gain(), 0.25);
    assert_eq!(
        result.state.project.clip_alias(clip),
        result.state.project.clip_alias(alias)
    );
    let undone = run(&service, Request::Undo);
    assert_eq!(undone.state.project.tracks().len(), 1);
    assert!(undone.state.project.midi_clip(alias).is_none());
    let redone = run(&service, Request::Redo);
    assert_eq!(redone.state.project.tracks().len(), 2);
    assert_eq!(
        redone.state.project.clip_alias(clip),
        redone.state.project.clip_alias(alias)
    );
}
