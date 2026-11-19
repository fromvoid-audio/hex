use super::*;

fn next(service: &Service) -> Event {
    // Blocks on the event channel; bounded only against hangs.
    match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("next: worker event absent: {error}"),
    }
}
fn fixture() -> (
    Service,
    hex_project::TrackId,
    hex_project::ClipId,
    Arc<ViewState>,
) {
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "notes".into(),
        }))
        .unwrap();
    let added = next(&service);
    let Created::Track(track) = added.created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let added = next(&service);
    let Created::Clip(clip) = added.created.unwrap() else {
        panic!("clip")
    };
    (service, track, clip, added.state)
}
fn request(
    revision: u64,
    source: hex_project::ClipId,
    before_track: Option<hex_project::TrackId>,
) -> Request {
    Request::CopyArrangementClipToConsoleTrack {
        initial_gain: Some(0.25),
        project_generation: 0,
        revision,
        source,
        before_track,
        scene: hex_project::SessionSceneTarget::Insert { before: None },
    }
}
#[test]
fn console_clip_column_worker_prepares_one_stamped_birth_and_one_undo_redo() {
    let (service, original_track, clip, before) = fixture();
    service
        .submit(request(before.revision, clip, Some(original_track)))
        .unwrap();
    let created = next(&service);
    assert!(created.error.is_none(), "{:?}", created.error);
    let Created::Track(track) = created.created.unwrap() else {
        panic!("new column")
    };
    assert_eq!(created.state.project.tracks()[0].id(), track);
    assert_eq!(created.state.project.track(track).unwrap().gain(), 0.25);
    assert_eq!(created.state.revision, before.revision + 1);
    assert_eq!(created.state.project.scenes().len(), 1);
    assert!(
        matches!(created.state.project.session_cell(track, created.state.project.scenes()[0].id()), Some(hex_project::SessionCellContent::Midi(copy)) if copy.id() != clip)
    );
    assert!(
        created
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );
    assert_eq!(
        created.state.project.midi_clip(clip),
        before.project.midi_clip(clip)
    );
    service.submit(Request::Undo).unwrap();
    let undone = next(&service);
    assert!(undone.error.is_none());
    assert_eq!(undone.state.project.tracks(), before.project.tracks());
    assert!(undone.state.project.scenes().is_empty());
    assert!(undone.state.project.session_cells().is_empty());
    service.submit(Request::Redo).unwrap();
    let redone = next(&service);
    assert!(redone.error.is_none());
    assert_eq!(
        redone.state.project.tracks(),
        created.state.project.tracks()
    );
    assert_eq!(
        redone.state.project.session_cells(),
        created.state.project.session_cells()
    );
}
#[test]
fn console_clip_column_stale_generation_or_revision_is_rejected_without_births() {
    let (service, track, clip, before) = fixture();
    for (generation, revision) in [(1, before.revision), (0, before.revision - 1)] {
        service
            .submit(Request::CopyArrangementClipToConsoleTrack {
                initial_gain: None,
                project_generation: generation,
                revision,
                source: clip,
                before_track: Some(track),
                scene: hex_project::SessionSceneTarget::Insert { before: None },
            })
            .unwrap();
        let refused = next(&service);
        assert!(
            refused
                .error
                .as_deref()
                .unwrap()
                .contains("project changed")
        );
        assert_eq!(refused.state.revision, before.revision);
        assert_eq!(refused.state.project, before.project);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (before.can_undo, before.can_redo)
        );
        assert!(refused.created.is_none());
    }
}
#[test]
fn console_clip_column_queued_after_an_edit_uses_captured_revision_and_keeps_that_edit() {
    let (service, track, clip, before) = fixture();
    service
        .submit(Request::Edit(Command::AddScene {
            name: "intervening".into(),
        }))
        .unwrap();
    service
        .submit(request(before.revision, clip, Some(track)))
        .unwrap();
    let changed = next(&service);
    assert!(changed.error.is_none());
    let refused = next(&service);
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("project changed")
    );
    assert_eq!(refused.state.project, changed.state.project);
    assert_eq!(refused.state.revision, changed.state.revision);
    assert_eq!(refused.state.project.tracks().len(), 1);
    assert_eq!(refused.state.project.scenes().len(), 1);
}

#[test]
fn console_clip_column_rejects_invalid_gain_without_birth_or_history() {
    let (service, track, clip, before) = fixture();
    service
        .submit(Request::CopyArrangementClipToConsoleTrack {
            project_generation: 0,
            revision: before.revision,
            source: clip,
            before_track: Some(track),
            scene: hex_project::SessionSceneTarget::Insert { before: None },
            initial_gain: Some(f32::NAN),
        })
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, before.project);
    assert_eq!(refused.state.revision, before.revision);
    assert_eq!(
        (refused.state.can_undo, refused.state.can_redo),
        (before.can_undo, before.can_redo)
    );
}
