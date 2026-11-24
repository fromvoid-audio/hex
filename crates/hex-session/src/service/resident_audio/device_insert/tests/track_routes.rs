//! Resident route commands retain canonical/history custody until audio's terminal.
use super::*;
use hex_project::{TrackDestination, TrackId, TrackSelectionIds};

fn route_fixture() -> (
    Service,
    Arc<Mutex<Fixture>>,
    Arc<ViewState>,
    TrackId,
    TrackId,
) {
    let (service, fixture, _) = service();
    let run = |command| {
        service.submit(Request::Edit(command)).unwrap();
        receive(&service)
    };
    let Created::Track(track) = run(Command::AddTrack {
        name: "moving".into(),
    })
    .created
    .unwrap() else {
        panic!("track");
    };
    let Created::Track(child) = run(Command::AddTrack {
        name: "child".into(),
    })
    .created
    .unwrap() else {
        panic!("track");
    };
    let grouped = run(Command::GroupTracks {
        tracks: TrackSelectionIds::new(&[child]).unwrap(),
        name: "group".into(),
    });
    let Created::Track(group) = grouped.created.unwrap() else {
        panic!("group");
    };
    (service, fixture, grouped.state, track, group)
}

fn reparent(track: TrackId, group: TrackId) -> Command {
    Command::ReparentTracks {
        tracks: TrackSelectionIds::new(&[track]).unwrap(),
        destination: TrackDestination {
            parent_group: Some(group),
            before: None,
        },
    }
}

fn submit(service: &Service, state: &ViewState, track: TrackId, group: TrackId) -> Event {
    service
        .submit(Request::ReparentTracks {
            project_generation: 0,
            revision: state.revision,
            tracks: TrackSelectionIds::new(&[track]).unwrap(),
            destination: TrackDestination {
                parent_group: Some(group),
                before: None,
            },
        })
        .unwrap();
    let pending = receive(service);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, state.project);
    assert_eq!(pending.state.revision, state.revision);
    assert_eq!(
        (pending.state.can_undo, pending.state.can_redo),
        (state.can_undo, state.can_redo)
    );
    pending
}

#[test]
fn resident_reparent_waits_for_exact_install_and_commits_one_history_step() {
    let (service, fixture, original, track, group) = route_fixture();
    let pending = submit(&service, &original, track, group);
    assert_eq!(
        pending.status.as_deref(),
        Some("track move pending · waiting for audio")
    );
    let files = SavedFile::new();
    service
        .submit(Request::SaveNew(files.0.join("pending.hex")))
        .unwrap();
    let still_pending = receive(&service);
    assert_eq!(still_pending.state.project, original.project);
    assert_eq!(still_pending.state.revision, original.revision);
    let installed = install_candidate(&service, &fixture);
    assert_eq!(installed.status.as_deref(), Some("track moved"));
    assert_eq!(
        installed.state.project.track(track).unwrap().parent_group(),
        Some(group)
    );
    assert_eq!(installed.state.revision, original.revision + 1);
    let audio_revision = fixture.lock().unwrap().revision;
    service
        .submit(Request::Edit(reparent(track, group)))
        .unwrap();
    let no_op = receive(&service);
    assert_eq!(no_op.state.revision, installed.state.revision);
    assert_eq!(fixture.lock().unwrap().revision, audio_revision);
    service.submit(Request::Undo).unwrap();
    assert_eq!(receive(&service).state.project, original.project);
}

#[test]
fn only_cross_parent_track_gap_waits_for_exact_install() {
    let (service, fixture, original, track, group) = route_fixture();
    submit(&service, &original, track, group);
    let grouped = install_candidate(&service, &fixture).state;
    let sibling = grouped
        .project
        .tracks()
        .iter()
        .find(|row| row.id() != track && row.parent_group() == Some(group))
        .unwrap()
        .id();
    let drop_into_gap = |state: &ViewState, before| {
        service
            .submit(Request::ReorderTracks {
                project_generation: 0,
                revision: state.revision,
                tracks: TrackSelectionIds::new(&[track]).unwrap(),
                before,
            })
            .unwrap();
        receive(&service)
    };
    let order = |state: &ViewState| {
        state
            .project
            .tracks()
            .iter()
            .map(|row| row.id())
            .collect::<Vec<_>>()
    };

    let reordered = drop_into_gap(&grouped, Some(sibling));
    assert!(reordered.request_complete);
    assert!(reordered.device_insert_complete.is_none());
    assert_eq!(reordered.state.revision, grouped.revision + 1);
    assert_eq!(
        reordered.state.project.track(track).unwrap().parent_group(),
        Some(group)
    );
    assert_eq!(order(&reordered.state), [group, track, sibling]);

    let pending = drop_into_gap(&reordered.state, None);
    assert!(
        !pending.request_complete,
        "a gap outside the Group must wait for audio"
    );
    assert_eq!(
        pending.status.as_deref(),
        Some("track move pending · waiting for audio")
    );
    assert_eq!(pending.state.project, reordered.state.project);
    assert_eq!(pending.state.revision, reordered.state.revision);
    let installed = install_candidate(&service, &fixture);
    assert_eq!(installed.state.revision, reordered.state.revision + 1);
    assert_eq!(
        installed.state.project.track(track).unwrap().parent_group(),
        None
    );
    assert_eq!(order(&installed.state), [group, sibling, track]);

    let pending = drop_into_gap(&installed.state, Some(sibling));
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, installed.state.project);
    let entered = install_candidate(&service, &fixture);
    assert_eq!(entered.state.revision, installed.state.revision + 1);
    assert_eq!(
        entered.state.project.track(track).unwrap().parent_group(),
        Some(group)
    );
    assert_eq!(order(&entered.state), [group, track, sibling]);
}

#[test]
fn route_terminals_refuse_without_committing_the_candidate() {
    for (state, outcome) in [
        (CANCELLED, DeviceInsertOutcome::Cancelled),
        (EXPIRED, DeviceInsertOutcome::Expired),
        (REFUSED, DeviceInsertOutcome::ConfirmationRequired),
        (OUTPUT_CLOSED, DeviceInsertOutcome::OutputClosed),
        // Priority Stop/source replacement invalidate the callback fence.
        // Session consumes that trusted terminal rather than owning transport.
        (
            SOURCE_CHANGED,
            DeviceInsertOutcome::Refused(hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged),
        ),
    ] {
        let (service, fixture, original, track, group) = route_fixture();
        submit(&service, &original, track, group);
        let receipt = fixture.lock().unwrap().candidate.clone().unwrap();
        receipt.state.store(state, Ordering::Release);
        let terminal = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(terminal.request_complete);
        assert_eq!(
            terminal.device_insert_complete,
            Some(DeviceInsertCompletion {
                submission: receipt.submission,
                outcome
            })
        );
        assert_eq!(terminal.state.project, original.project);
        assert_eq!(terminal.state.revision, original.revision);
        assert_eq!(
            (terminal.state.can_undo, terminal.state.can_redo),
            (original.can_undo, original.can_redo)
        );
        if state == REFUSED {
            assert_eq!(
                terminal.error.as_deref(),
                Some("stop playback to move this track")
            );
        }
        if state == EXPIRED {
            assert_eq!(
                terminal.error.as_deref(),
                Some("track move timed out before installation")
            );
        }
    }
}

#[test]
fn installed_route_cannot_be_cancelled_or_committed_twice() {
    let (service, fixture, original, track, group) = route_fixture();
    submit(&service, &original, track, group);
    let submission = {
        let mut fixture = fixture.lock().unwrap();
        let receipt = fixture.candidate.clone().unwrap();
        receipt.state.store(INSTALLED, Ordering::Release);
        fixture.active = Some(ResidentRuntimeLoop {
            output_identity: receipt.submission.output_identity,
            audio_revision: receipt.submission.audio_revision,
            source_context: Some(receipt.submission.context),
            source_origin: ResidentSourceOrigin::CanonicalCandidate,
            section_loop: None,
        });
        assert!(!receipt.cancel());
        receipt.submission
    };
    let installed = receive(&service);
    assert_eq!(
        installed.device_insert_complete.unwrap().submission,
        submission
    );
    assert_eq!(installed.state.revision, original.revision + 1);
    service.submit(Request::ContinueDeviceInsert).unwrap();
    let duplicate_poll = receive(&service);
    assert_eq!(duplicate_poll.state.project, installed.state.project);
    assert_eq!(duplicate_poll.state.revision, installed.state.revision);
    assert!(duplicate_poll.device_insert_complete.is_none());
}

#[test]
fn reparent_classification_is_single_resident_and_keeps_no_output_behavior() {
    let (_service, fixture, original, track, group) = route_fixture();
    let command = reparent(track, group);
    assert_eq!(
        resident_graph_edit_operation(&command, false),
        Some(GraphEditOperation::MoveTrack)
    );
    assert_eq!(graph_edit_operation(&command), None); // Verified catalog stays unchanged.
    let mut sink = Arc::clone(&fixture);
    assert!(should_stage_resident_graph_edit(
        &original.project,
        &command,
        &mut sink
    ));
    let child = original
        .project
        .tracks()
        .iter()
        .find(|candidate| candidate.parent_group() == Some(group))
        .unwrap()
        .id();
    let multiple = Command::ReparentTracks {
        tracks: TrackSelectionIds::new(&[track, child]).unwrap(),
        destination: TrackDestination {
            parent_group: Some(group),
            before: None,
        },
    };
    assert_eq!(resident_graph_edit_operation(&multiple, false), None);
    fixture.lock().unwrap().active = None;
    assert!(!should_stage_resident_graph_edit(
        &original.project,
        &command,
        &mut sink
    ));
}

#[test]
fn track_move_consent_is_truthful_and_cancel_keeps_the_old_route() {
    let (service, fixture, original, track, group) = route_fixture();
    submit(&service, &original, track, group);
    {
        let mut fixture = fixture.lock().unwrap();
        fixture.offer_deadline = Some(Instant::now() + Duration::from_secs(3));
        fixture
            .candidate
            .as_ref()
            .unwrap()
            .state
            .store(REFUSED, Ordering::Release);
    }
    let offered = receive(&service);
    assert!(!offered.request_complete);
    assert_eq!(offered.state.project, original.project);
    let offer = offered.device_insert_confirmation.unwrap();
    assert_eq!(offer.operation(), GraphEditOperation::MoveTrack);
    assert!(offer.cancel());
    let cancelled = receive(&service);
    assert_eq!(cancelled.status.as_deref(), Some("track move cancelled"));
    assert_eq!(
        cancelled.device_insert_complete.unwrap().outcome,
        DeviceInsertOutcome::Cancelled
    );
    assert_eq!(cancelled.state.project, original.project);
    assert_eq!(cancelled.state.revision, original.revision);
}
