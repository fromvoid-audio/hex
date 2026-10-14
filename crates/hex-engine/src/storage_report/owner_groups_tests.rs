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

fn demand_root() -> crate::OwnedPreparedRevision {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    for name in ["one", "two"] {
        editor
            .execute(hex_project::Command::ImportAudioTrack {
                name: name.into(),
                path: "metadata-only.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap();
    }
    let sources = editor
        .project()
        .assets()
        .map(|asset| {
            let metadata = crate::StreamedSourceMetadata::new(
                crate::StreamSourceKey::new(19, asset.id()),
                48_000,
                48_000,
                asset.source_frames(),
                asset.playback_frames(),
                asset.conversion_version(),
                [19; 32],
            )
            .unwrap();
            (
                asset.id(),
                crate::compile::PreparedAudioSource::Streamed(Arc::new(metadata)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    crate::PreparedRevision::prepare_with_prepared_audio_sources(
        editor.project(),
        &sources,
        19,
        Arc::new(crate::SineTable::new()),
        None,
    )
    .unwrap()
    .into_owned_shared()
}
fn tracked(root: crate::OwnedPreparedRevision) -> OwnerTrackedPlayback {
    let (owner, anchors) = pair(root);
    owner
        .try_acquire_owner_groups_on_control(&anchors, reservation())
        .unwrap()
        .0
}
fn book_snapshot(
    book: &OwnerGroupBook,
) -> Vec<(OwnerLocation, u64, Option<Origin>, Option<ClaimIdentity>)> {
    book.members
        .iter()
        .map(|m| {
            (
                m.location,
                m.incarnation,
                m.group.as_ref().map(|g| g.0),
                m.planned_by,
            )
        })
        .collect()
}
#[test]
fn sealed_demand_checks_both_exact_books_before_visiting_and_preserves_owners() {
    let root = demand_root();
    let mut candidate = tracked(root.clone_on_control());
    let mut old = tracked(root);
    old.apply(crate::playback::PlaybackCommand::Play);
    let one = candidate.identity();
    let two = old.identity();
    let candidate_book = book_snapshot(&candidate.book);
    let old_book = book_snapshot(&old.book);
    for mismatch_previous in [false, true] {
        if mismatch_previous {
            old.book.runtime = one;
        } else {
            candidate.book.runtime = two;
        }
        let mut visits = 0;
        assert!(
            candidate
                .preview_stream_handoff(&mut old, 256, &mut |_| {
                    visits += 1;
                    Ok(())
                })
                .is_err()
        );
        if !mismatch_previous {
            assert!(
                candidate
                    .visit_next_stream_chunk(256, &mut |_| {
                        visits += 1;
                        Ok(())
                    })
                    .is_err()
            );
        }
        assert_eq!(visits, 0);
        assert_eq!(candidate.identity(), one);
        assert_eq!(old.identity(), two);
        candidate.book.runtime = one;
        old.book.runtime = two;
        assert_eq!(book_snapshot(&candidate.book), candidate_book);
        assert_eq!(book_snapshot(&old.book), old_book);
    }
}
#[test]
fn sealed_preview_matches_exact_legacy_planner_and_refusal_discards_speculative_pages() {
    let root = demand_root();
    let mut next = tracked(root.clone_on_control());
    let mut old = tracked(root.clone_on_control());
    let (mut legacy_next, _) = pair(root.clone_on_control());
    let (mut legacy_old, _) = pair(root);
    old.apply(crate::playback::PlaybackCommand::PlayFrom(100));
    legacy_old
        .playback
        .apply(crate::playback::PlaybackCommand::PlayFrom(100));
    let old_book = book_snapshot(&old.book);
    let next_book = book_snapshot(&next.book);
    let mut pages = Vec::with_capacity(8);
    let mut expected = Vec::with_capacity(8);
    let window = next
        .preview_stream_handoff(&mut old, usize::MAX, &mut |extent| {
            pages.push(extent);
            Ok(())
        })
        .unwrap();
    let reference = legacy_next
        .playback
        .preview_stream_handoff(&mut legacy_old.playback, usize::MAX, &mut |extent| {
            expected.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(window, reference);
    assert_eq!(pages, expected);
    assert_eq!(pages.len(), 2);
    assert_eq!(window.frames, 256);
    pages.clear();
    assert!(
        next.preview_stream_handoff(&mut old, 256, &mut |extent| {
            pages.push(extent);
            Err(crate::Invalid("actual visitor capacity refusal"))
        })
        .is_err()
    );
    assert_eq!(pages.len(), 1); // actual speculative partial output, never installed
    pages.clear();
    assert_eq!(book_snapshot(&old.book), old_book);
    assert_eq!(book_snapshot(&next.book), next_book);
    assert_eq!(old.owner.playback.position(), 100);
    assert_eq!(next.owner.playback.position(), 0);
    let retried = next
        .preview_stream_handoff(&mut old, 256, &mut |extent| {
            pages.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(retried, reference);
    assert_eq!(pages, expected);
    let mut direct = Vec::with_capacity(8);
    assert_eq!(
        old.visit_next_stream_chunk(256, &mut |e| {
            direct.push(e);
            Ok(())
        })
        .unwrap(),
        legacy_old
            .playback
            .visit_next_stream_chunk(256, &mut |_| Ok(()))
            .unwrap()
    );
    assert_eq!(direct, expected);
    assert!(next.is_incomplete_fixture() && old.is_incomplete_fixture());
}
