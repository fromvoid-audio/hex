use super::*;
use crate::storage_report::incomplete_owned_pair_for_test;
use std::collections::BTreeMap;
fn root() -> crate::OwnedPreparedRevision {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(hex_project::Command::AddTrack {
            name: "owned".into(),
        })
        .unwrap();
    crate::PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(crate::SineTable::new()),
        None,
    )
    .unwrap()
    .into_owned_shared()
}
fn reservation() -> ClaimReservation {
    ClaimReservation {
        items: 32_768,
        anchors: 16,
        max_prepared_data_bytes: usize::MAX,
        max_construction_data_bytes: usize::MAX,
        other_retained_bytes: 0,
        other_construction_bytes: 0,
    }
}
fn pair(root: crate::OwnedPreparedRevision) -> (ClaimedPlaybackOwner, AnchoredPlaybackClaims) {
    let (mut owner, anchors) = incomplete_owned_pair_for_test(root).ok().unwrap();
    owner.prepare_owner_groups_for_test().unwrap();
    (owner, anchors)
}
#[test]
fn twin_runtime_same_actual_root_refuses_and_returns_original_box_and_reservation() {
    let root = root();
    let (one, anchors) = pair(root.clone_on_control());
    let (two, _) = pair(root);
    let address = two.playback.as_ref() as *const crate::playback::Playback;
    let id = two.identity();
    let reservation = reservation();
    let error = two
        .try_acquire_owner_groups_on_control(&anchors, reservation)
        .err()
        .unwrap();
    assert_eq!(error.reason, OwnerGroupError::PairMismatch);
    assert_eq!(error.owner.identity(), id);
    assert_eq!(
        error.owner.playback.as_ref() as *const crate::playback::Playback,
        address
    );
    assert_eq!(error.reservation.items, reservation.items);
    assert_eq!(error.reservation.anchors, reservation.anchors);
    assert_eq!(
        error.reservation.max_prepared_data_bytes,
        reservation.max_prepared_data_bytes
    );
    assert_eq!(error.report.member_capacity, 0);
    assert!(anchors.matches_owner(&one));
}
#[test]
fn every_injected_reserve_refuses_without_path_owner_or_reservation_loss() {
    for stage in 11..=13 {
        let (owner, anchors) = pair(root());
        let address = owner.playback.as_ref() as *const crate::playback::Playback;
        let reservation = reservation();
        super::super::FAIL_RESERVE.with(|fail| fail.set(stage));
        let error = owner
            .try_acquire_owner_groups_on_control(&anchors, reservation)
            .err()
            .unwrap();
        assert_eq!(error.reason, OwnerGroupError::Allocation);
        assert_eq!(
            error.owner.playback.as_ref() as *const crate::playback::Playback,
            address
        );
        assert!(anchors.matches_owner(&error.owner));
        assert_eq!(
            error.reservation.max_construction_data_bytes,
            reservation.max_construction_data_bytes
        );
        assert_eq!(error.report.member_capacity > 0, stage > 11);
        assert_eq!(error.report.archive_capacity > 0, stage > 12);
        assert_eq!(error.report.journal_capacity, 0);
    }
}
#[test]
fn capacity_overflow_and_unprewarmed_refusals_preserve_original_inputs() {
    for reservation in [
        ClaimReservation {
            items: 0,
            ..reservation()
        },
        ClaimReservation {
            max_prepared_data_bytes: 0,
            ..reservation()
        },
        ClaimReservation {
            other_retained_bytes: usize::MAX,
            ..reservation()
        },
    ] {
        let (owner, anchors) = pair(root());
        let address = owner.playback.as_ref() as *const crate::playback::Playback;
        let error = owner
            .try_acquire_owner_groups_on_control(&anchors, reservation)
            .err()
            .unwrap();
        assert!(matches!(
            error.reason,
            OwnerGroupError::Capacity
                | OwnerGroupError::ReservationExceeded
                | OwnerGroupError::Overflow
        ));
        assert_eq!(
            error.owner.playback.as_ref() as *const crate::playback::Playback,
            address
        );
        assert!(anchors.matches_owner(&error.owner));
    }
    let (owner, anchors) = incomplete_owned_pair_for_test(root()).ok().unwrap();
    let error = owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .err()
        .unwrap();
    assert_eq!(error.reason, OwnerGroupError::NotPrewarmed);
    assert!(anchors.matches_owner(&error.owner));
}
#[test]
fn actual_capacities_and_parent_headers_stay_distinct_and_fixture_unknown_stays_sticky() {
    let root = root();
    let (owner, anchors) = pair(root.clone_on_control());
    let before = Arc::strong_count(owner.playback.claim_prepared_owner().unwrap());
    let (tracked, archive, report) = owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .unwrap();
    assert_eq!(Arc::strong_count(&archive.root), before + 1);
    assert_eq!(report.member_capacity, tracked.book.members.capacity());
    assert_eq!(report.journal_capacity, tracked.book.journal.capacity());
    assert!(report.member_capacity > archive.owning_handle_count());
    assert_eq!(
        report.runtime_metadata_bytes,
        Some(
            size_of::<OwnerGroupBook>()
                + report.member_capacity * size_of::<Member>()
                + report.journal_capacity * size_of::<TransferRow>()
        )
    );
    assert_eq!(
        report.control_archive_bytes,
        Some(size_of::<OwnerGroupArchive>() + report.archive_capacity * size_of::<ArchivedGroup>())
    );
    assert!(archive.same_prepared_owner(&root));
    assert!(tracked.metadata().is_some());
    assert!(report.incomplete_source && tracked.is_incomplete_fixture());
}
#[test]
fn persistent_output_pressure_keeps_both_actual_owners_and_candidate_fresh() {
    let (owner, anchors) = pair(root());
    let (mut candidate, _, _) = owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .unwrap();
    let (owner, anchors) = pair(root());
    let (previous, _, _) = owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .unwrap();
    let previous_identity = previous.identity();
    let mut previous = Some(previous);
    let mut retired = None;
    let mut manifest = Some(OwnerTransferManifest {
        previous: previous_identity,
        successor: candidate.identity(),
        rows: Vec::new(),
        incomplete_source: true,
    });
    let error = candidate
        .try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut manifest)
        .unwrap_err();
    assert!(!error.candidate_consumed);
    assert_eq!(previous.as_ref().unwrap().identity(), previous_identity);
    assert!(retired.is_none());
    assert_eq!(manifest.as_ref().unwrap().predecessor(), previous_identity);
    assert!(!candidate.owner.playback.claim_candidate_consumed());
}

#[test]
fn mixed_origin_negative_control_cannot_launder_fixture_resources_into_ordinary_successor() {
    let (owner, anchors) = pair(root());
    let (mut candidate, _, _) = owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .unwrap();
    let (owner, anchors) = pair(root());
    let (previous, _, _) = owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .unwrap();
    let identity = previous.identity();
    // Deliberate private negative control, NOT a supported ordinary constructor
    // or successful admission/report fixture. Both original reports stay incomplete.
    candidate.owner.fixture_incomplete = false;
    let mut previous = Some(previous);
    let mut retired = None;
    let mut manifest = None;
    let error = candidate
        .try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut manifest)
        .unwrap_err();
    assert!(!error.candidate_consumed);
    assert_eq!(previous.as_ref().unwrap().identity(), identity);
    assert!(retired.is_none() && manifest.is_none());
}
#[test]
fn metadata_incarnation_duplicate_and_destination_refusals_precede_any_transfer() {
    let (owner, anchors) = pair(root());
    let (mut candidate, _, _) = owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .unwrap();
    let (owner, anchors) = pair(root());
    let (mut previous, _, _) = owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .unwrap();
    // Association-only negative controls cannot mint a real disposal or claim
    // admission. Actual-member transfer/disposal is tested through real tails.
    assert!(previous.book.members[0].group.is_some());
    let from = previous.book.members[0].location;
    let destination = candidate
        .book
        .members
        .iter()
        .position(|member| member.group.is_none())
        .unwrap();
    let to = candidate.book.members[destination].location;
    candidate.book.members[destination].incarnation = u64::MAX;
    let mut transfer = OwnerGroupTransfer::new(&mut candidate.book, &mut previous.book);
    assert_eq!(
        transfer.plan(from, to),
        Err(crate::SessionPlaybackError::ArithmeticOverflow)
    );
    assert!(transfer.candidate.journal.is_empty());
    assert!(transfer.previous.members[0].group.is_some());
    transfer.candidate.members[destination].incarnation = 1;
    transfer.plan(from, to).unwrap();
    assert_eq!(
        transfer.plan(from, to),
        Err(crate::SessionPlaybackError::ReleaseBankFull)
    );
    assert!(transfer.previous.members[0].group.is_some());
    assert!(transfer.candidate.members[destination].group.is_none());
}

#[test]
fn real_destructor_success_and_panic_retain_the_book_without_repeat_destruction_or_false_closure() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for fail in [false, true] {
        let (mut owner, anchors) = pair(root());
        let drops = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&drops);
        let control = std::thread::current().id();
        owner.observe_disposal_for_test(move |_| {
            assert_eq!(std::thread::current().id(), control);
            counter.fetch_add(1, Ordering::SeqCst);
            assert!(!fail, "actual control destructor negative control");
        });
        let (tracked, mut archive, _) = owner
            .try_acquire_owner_groups_on_control(&anchors, reservation())
            .unwrap();
        let total = archive.owning_handle_count();
        let mut capsule = tracked.into_control_teardown();
        let result = capsule.dispose_on_control();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        if fail {
            assert!(matches!(
                result,
                Err(super::super::ControlTeardownError::Incomplete)
            ));
            assert!(capsule.book.is_some());
            assert!(matches!(
                capsule.dispose_on_control(),
                Err(super::super::ControlTeardownError::Incomplete)
            ));
            assert_eq!(archive.remaining_handle_count(), total);
            assert!(!archive.original_runtime_was_disposed());
        } else {
            let proof = result.ok().unwrap();
            assert!(proof.is_incomplete_fixture());
            archive.record_disposal_on_control(&proof).unwrap();
            assert_eq!(archive.remaining_handle_count(), 0);
            assert!(archive.original_runtime_was_disposed());
            assert_eq!(
                archive.record_disposal_on_control(&proof),
                Err(OwnerGroupError::Consumed)
            );
            assert!(matches!(
                capsule.dispose_on_control(),
                Err(super::super::ControlTeardownError::Consumed)
            ));
        }
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(archive.report().incomplete_source);
    }
}
