use super::*;
use std::time::Duration;
fn setup(
    capacity: usize,
) -> (
    Box<CallbackDomainBatch>,
    TerminalBatchSender,
    MediaDomainReaper,
    DomainAdmission,
) {
    let (batch, terminal, mut reaper) = reusable_domain_escrow(2).unwrap();
    let mut runtime =
        crate::StreamRuntime::new(crate::StreamConfig::with_pool_pages(4), Vec::new()).unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, pages) = runtime.split().ok().unwrap();
    let endpoint =
        crate::VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap();
    let input = MediaDomainInput {
        id: DomainId::new(1).unwrap(),
        endpoint: PageReadiness::new(endpoint, 3).unwrap(),
        retained_leases: vec![control.catalog_lease().unwrap()],
        control,
    };
    let admission = reaper.reserve_domain(input).ok().unwrap();
    reaper
        .configure_catalog_custody(DomainId::new(1).unwrap(), capacity, usize::MAX)
        .unwrap();
    (batch, terminal, reaper, admission)
}
fn cover(reaper: &mut MediaDomainReaper, lease: CatalogLease) -> CatalogCoverageCovered {
    let id = DomainId::new(1).unwrap();
    let reservation = reaper.reserve_catalog_coverage(id, &lease).unwrap();
    let binding = reservation.binding_on_control();
    let pending = reaper
        .commit_catalog_coverage(reservation, lease)
        .ok()
        .unwrap();
    assert_ne!(
        reaper.advance_catalog_coverage_on_control(id),
        CatalogCoverageProgress::Incomplete
    );
    let ack = reaper.catalog_coverage_ack(pending).ok().unwrap();
    assert!(binding.matches_ack(&ack));
    let covered = reaper.consume_catalog_coverage_ack(ack).ok().unwrap();
    assert!(binding.matches_covered(&covered));
    covered
}
#[test]
fn actual_same_catalog_continuous_coverage_reuses_records_while_live_lease_keeps_domain() {
    let (mut batch, mut terminal, mut reaper, admission) = setup(2);
    let id = DomainId::new(1).unwrap();
    let lease = reaper.catalog_lease(id).unwrap();
    let before = reaper.footprint;
    for _ in 0..32 {
        let covered = cover(&mut reaper, lease.clone());
        assert_eq!(reaper.slots[0].custody.lease_count(), 1);
        assert_eq!(
            reaper.footprint, before,
            "whole domain charge never subtracted per graph"
        );
        assert!(!reaper.domain_has_only_escrow_references(id));
        drop(covered);
    }
    batch.install_domain(admission).ok().unwrap();
    terminal.try_return_stopped_batch(batch).ok().unwrap();
    while reaper.poll_one().unwrap() != DomainPoll::Empty {}
    assert!(matches!(
        reaper.try_reap_domain(id),
        DomainCleanupProgress::Busy { .. }
    ));
    assert!(matches!(
        reaper.stable_catalog_cleanup_receipt(id),
        Err(CatalogCustodyError::Incomplete)
    ));
    drop(lease);
    let report = match reaper.try_reap_domain(id) {
        DomainCleanupProgress::Finished(report) => report,
        _ => panic!("actual cleanup"),
    };
    assert!(report.successful_report().is_some());
    assert!(reaper.take_reclaimed_report(id).is_none());
    let receipt = reaper.stable_catalog_cleanup_receipt(id).unwrap();
    assert!(matches!(
        reaper.stable_catalog_cleanup_receipt(id),
        Err(CatalogCustodyError::Busy)
    ));
    assert_eq!(
        reaper.consume_catalog_cleanup_receipt(receipt).ok(),
        Some(report)
    );
    assert_eq!(reaper.take_reclaimed_report(id), Some(report));
}
#[test]
fn reservation_pressure_cancel_wrong_actual_catalog_and_root_return_original_owners() {
    let (_, _, mut one, _) = setup(2);
    let (_, _, mut two, _) = setup(2);
    let id = DomainId::new(1).unwrap();
    let actual = one.catalog_lease(id).unwrap();
    let wrong = two.catalog_lease(id).unwrap();
    assert!(matches!(
        one.reserve_catalog_coverage(id, &wrong),
        Err(CatalogCustodyError::WrongPair)
    ));
    let reservation = one.reserve_catalog_coverage(id, &actual).unwrap();
    assert!(matches!(
        one.reserve_catalog_coverage(id, &actual),
        Err(CatalogCustodyError::Capacity)
    ));
    assert_eq!(
        one.release_catalog_custody_capacity(id),
        Err(CatalogCustodyError::Busy)
    );
    let error = two
        .commit_catalog_coverage(reservation, actual)
        .err()
        .unwrap();
    assert_eq!(error.reason, CatalogCustodyError::Stale);
    assert!(
        error
            .lease
            .same_verified_catalog(&one.catalog_lease(id).unwrap())
    );
    let error = one
        .commit_catalog_coverage(error.reservation, wrong)
        .err()
        .unwrap();
    assert_eq!(error.reason, CatalogCustodyError::WrongPair);
    assert!(
        error
            .lease
            .same_verified_catalog(&two.catalog_lease(id).unwrap())
    );
    one.cancel_catalog_coverage(error.reservation).ok().unwrap();
    assert!(one.slots[0].custody.coverage.is_none());
    assert_eq!(one.slots[0].custody.lease_count(), 0);
    drop(error.lease);
}
#[test]
fn abandoned_ack_and_failed_actual_redundant_disposal_retain_custody_without_retry() {
    let (_, _, mut reaper, _) = setup(3);
    let id = DomainId::new(1).unwrap();
    let lease = reaper.catalog_lease(id).unwrap();
    let _ = cover(&mut reaper, lease.clone());
    let reservation = reaper.reserve_catalog_coverage(id, &lease).unwrap();
    let index = reservation.0.source;
    let pending = reaper
        .commit_catalog_coverage(reservation, lease.clone())
        .ok()
        .unwrap();
    reaper.slots[0].custody.records[index].fail_disposal = true;
    assert_eq!(
        reaper.advance_catalog_coverage_on_control(id),
        CatalogCoverageProgress::Incomplete
    );
    assert!(reaper.slots[0].custody.records[index].first_panic.is_some());
    assert_eq!(reaper.slots[0].custody.lease_count(), 1);
    assert!(reaper.catalog_coverage_ack(pending).is_err());
    assert_eq!(
        reaper.advance_catalog_coverage_on_control(id),
        CatalogCoverageProgress::Incomplete
    );
    assert_eq!(reaper.slots[0].custody.records[index].disposal_attempts, 1);
    assert_eq!(
        reaper.release_catalog_custody_capacity(id),
        Err(CatalogCustodyError::Busy)
    );
    assert!(reaper.take_reclaimed_report(id).is_none());
    assert!(matches!(
        reaper.reserve_catalog_coverage(id, &lease),
        Err(CatalogCustodyError::Busy)
    ));
    let (_, _, mut healthy, _) = setup(2);
    let lease = healthy.catalog_lease(id).unwrap();
    let reservation = healthy.reserve_catalog_coverage(id, &lease).unwrap();
    let pending = healthy
        .commit_catalog_coverage(reservation, lease)
        .ok()
        .unwrap();
    let ack = healthy.catalog_coverage_ack(pending).ok().unwrap();
    drop(ack);
    assert!(healthy.slots[0].custody.reserved());
    assert_eq!(
        healthy.release_catalog_custody_capacity(id),
        Err(CatalogCustodyError::Busy)
    );
}

#[test]
fn measured_indexed_capacity_subset_and_declared_overlap_refuse_checked_overflow() {
    let (_, _, reaper, _) = setup(3);
    let id = DomainId::new(1).unwrap();
    let report = reaper.catalog_coverage_storage_report(id, 17, 23).unwrap();
    assert_eq!(
        report.records.capacity,
        reaper.slots[0].custody.records.capacity()
    );
    assert_eq!(
        report.lease_cells.capacity,
        reaper.slots[0].custody.leases.capacity()
    );
    assert_eq!(
        report.indexed_bytes_already_in_domain,
        report.records.bytes().unwrap() + report.lease_cells.bytes().unwrap()
    );
    assert_eq!(
        report.retained_metadata_bound,
        report.indexed_bytes_already_in_domain + 17
    );
    assert_eq!(
        report.construction_metadata_overlap,
        report.retained_metadata_bound + 23
    );
    assert!(report.whole_domain_data_bytes >= report.indexed_bytes_already_in_domain);
    assert!(!report.failure_payload_unknown);
    assert_eq!(
        reaper.catalog_coverage_storage_report(id, usize::MAX, 0),
        Err(CatalogCustodyError::Overflow)
    );
}
fn copy_negative_key(key: &Key) -> Key {
    // Private negative-control duplicate only; public receipt types have no Clone
    // or scalar factory and this does not mint a successful graph/role receipt.
    Key {
        seal: Arc::clone(&key.seal),
        domain: key.domain,
        incarnation: key.incarnation,
        source: key.source,
        source_nonce: key.source_nonce,
        target: key.target,
        target_nonce: key.target_nonce,
        created: key.created,
    }
}
#[test]
fn stale_duplicate_nonce_and_exhaustion_do_not_free_replacement_reservation() {
    let (_, _, mut reaper, _) = setup(2);
    let id = DomainId::new(1).unwrap();
    let lease = reaper.catalog_lease(id).unwrap();
    let reservation = reaper.reserve_catalog_coverage(id, &lease).unwrap();
    let pending = reaper
        .commit_catalog_coverage(reservation, lease.clone())
        .ok()
        .unwrap();
    let ack = reaper.catalog_coverage_ack(pending).ok().unwrap();
    let duplicate = CatalogCoverageAck(copy_negative_key(&ack.0));
    let old_nonce = ack.0.source_nonce;
    let _ = reaper.consume_catalog_coverage_ack(ack).ok().unwrap();
    let replacement = reaper.reserve_catalog_coverage(id, &lease).unwrap();
    assert!(replacement.0.source_nonce > old_nonce);
    let failed = reaper
        .consume_catalog_coverage_ack(duplicate)
        .err()
        .unwrap();
    assert_eq!(failed.0, CatalogCustodyError::Stale);
    assert_eq!(
        reaper.slots[0].custody.records[replacement.0.source].state,
        RecordState::CoverageSourceReserved
    );
    reaper.cancel_catalog_coverage(replacement).ok().unwrap();
    reaper.slots[0].custody.next_nonce = u64::MAX;
    let before = reaper.slots[0].custody.lease_count();
    assert!(matches!(
        reaper.reserve_catalog_coverage(id, &lease),
        Err(CatalogCustodyError::Exhausted)
    ));
    assert_eq!(reaper.slots[0].custody.lease_count(), before);
}
#[test]
fn owning_commit_and_nonfinal_redundant_drop_have_zero_allocations_frees_or_catalog_clones() {
    let (_, _, mut reaper, _) = setup(2);
    let id = DomainId::new(1).unwrap();
    let lease = reaper.catalog_lease(id).unwrap();
    let _ = cover(&mut reaper, lease.clone());
    let mut reservation = Some(reaper.reserve_catalog_coverage(id, &lease).unwrap());
    let mut source = Some(lease.clone());
    let before = reaper.slots[0].control.as_ref().unwrap().owner_references();
    let mut pending = None;
    let measured = allocation_counter::measure(|| {
        pending = reaper
            .commit_catalog_coverage(reservation.take().unwrap(), source.take().unwrap())
            .ok();
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        reaper.slots[0].control.as_ref().unwrap().owner_references(),
        before
    );
    let measured = allocation_counter::measure(|| {
        assert_eq!(
            reaper.advance_catalog_coverage_on_control(id),
            CatalogCoverageProgress::Advanced(1)
        );
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        reaper.slots[0].control.as_ref().unwrap().owner_references(),
        before - 1
    );
    let ack = reaper.catalog_coverage_ack(pending.unwrap()).ok().unwrap();
    let _ = reaper.consume_catalog_coverage_ack(ack).ok().unwrap();
    let positive = allocation_counter::measure(|| {
        let value = std::hint::black_box(Box::new(std::hint::black_box(7_u64)));
        std::hint::black_box(&value);
        drop(value);
    });
    assert_eq!(positive.count_total, 1);
    assert_eq!(positive.bytes_total, 8);
}

#[test]
fn actual_domain_slot_reuse_rejects_prior_incarnation_even_when_record_nonces_restart() {
    let (mut batch, _terminal, mut reaper, admission) = setup(2);
    let id = DomainId::new(1).unwrap();
    let lease = reaper.catalog_lease(id).unwrap();
    let reservation = reaper.reserve_catalog_coverage(id, &lease).unwrap();
    let old = copy_negative_key(&reservation.0);
    let pending = reaper
        .commit_catalog_coverage(reservation, lease)
        .ok()
        .unwrap();
    let ack = reaper.catalog_coverage_ack(pending).ok().unwrap();
    let _ = reaper.consume_catalog_coverage_ack(ack).ok().unwrap();
    batch.install_domain(admission).ok().unwrap();
    assert_eq!(batch.try_return_domain(id), DomainReturnStatus::Accepted);
    while reaper.poll_one().unwrap() != DomainPoll::Empty {}
    assert!(matches!(
        reaper.try_reap_domain(id),
        DomainCleanupProgress::Finished(_)
    ));
    let receipt = reaper.stable_catalog_cleanup_receipt(id).unwrap();
    reaper
        .consume_catalog_cleanup_receipt(receipt)
        .ok()
        .unwrap();
    reaper.take_reclaimed_report(id).unwrap();
    let mut runtime =
        crate::StreamRuntime::new(crate::StreamConfig::with_pool_pages(4), Vec::new()).unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, pages) = runtime.split().ok().unwrap();
    let endpoint =
        crate::VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap();
    let input = MediaDomainInput {
        id,
        endpoint: PageReadiness::new(endpoint, 3).unwrap(),
        retained_leases: Vec::new(),
        control,
    };
    let admission = reaper.reserve_domain(input).ok().unwrap();
    reaper.configure_catalog_custody(id, 2, usize::MAX).unwrap();
    let lease = reaper.catalog_lease(id).unwrap();
    let current = reaper.reserve_catalog_coverage(id, &lease).unwrap();
    assert_eq!(current.0.source_nonce, old.source_nonce);
    assert!(current.0.incarnation > old.incarnation);
    let error = reaper
        .consume_catalog_coverage_ack(CatalogCoverageAck(old))
        .err()
        .unwrap();
    assert_eq!(error.0, CatalogCustodyError::Stale);
    reaper.cancel_catalog_coverage(current).ok().unwrap();
    assert!(reaper.release_catalog_custody_capacity(id).is_ok());
    drop(lease);
    // Actual new uninstalled owner remains recoverable through legacy rollback.
    assert!(reaper.rollback_domain(admission).is_ok());
}
