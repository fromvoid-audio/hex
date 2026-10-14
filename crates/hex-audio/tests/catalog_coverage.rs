//! Actual unpublished graph owners + physical coverage. No native admission.
#![cfg(feature = "allocation-probe")]
use hex_engine::{
    PreparedRevision, SineTable,
    storage_report::{ClaimReservation, incomplete_owned_pair_for_test},
};
use hex_stream::{
    DomainId, MediaDomainInput, PageReadiness, StreamConfig, StreamRuntime,
    VerifiedOwnedStreamRevision, VerifiedPageEndpoint, reusable_domain_escrow,
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
#[test]
fn actual_unpublished_graph_disposals_continue_catalog_custody_without_enabling_native_release() {
    let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(4), Vec::new()).unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, pages) = runtime.split().ok().unwrap();
    let active = control.catalog_lease().unwrap();
    let endpoint = VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap();
    let id = DomainId::new(1).unwrap();
    let (mut batch, mut terminal, mut reaper) = reusable_domain_escrow(2).unwrap();
    let admission = reaper
        .reserve_domain(MediaDomainInput {
            id,
            control,
            endpoint: PageReadiness::new(endpoint, 3).unwrap(),
            retained_leases: Vec::new(),
        })
        .ok()
        .unwrap();
    reaper.configure_catalog_custody(id, 2, usize::MAX).unwrap();
    let baseline = reaper.allocation_footprint();
    for _ in 0..12 {
        let root = PreparedRevision::prepare(
            &hex_project::Project::new(48_000).unwrap(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared();
        let verified = VerifiedOwnedStreamRevision::new(root.clone_on_control(), active.clone())
            .ok()
            .unwrap();
        let (mut owner, anchors) = incomplete_owned_pair_for_test(root).ok().unwrap();
        owner.prepare_owner_groups_for_test().unwrap();
        let (tracked, mut archive, _) = owner
            .try_acquire_owner_groups_on_control(
                &anchors,
                ClaimReservation {
                    items: 32_768,
                    anchors: 16,
                    max_prepared_data_bytes: usize::MAX,
                    max_construction_data_bytes: usize::MAX,
                    other_retained_bytes: 0,
                    other_construction_bytes: 0,
                },
            )
            .unwrap();
        let reservation = reaper
            .reserve_verified_catalog_coverage(id, &verified)
            .unwrap();
        let binding = reservation.binding_on_control();
        let proof = tracked
            .into_control_teardown()
            .dispose_on_control()
            .unwrap();
        archive.record_disposal_on_control(&proof).unwrap();
        assert!(archive.original_runtime_was_disposed());
        assert_eq!(archive.remaining_handle_count(), 0);
        assert!(
            archive.report().incomplete_source,
            "actual fixture is NOT admitted"
        );
        let (_prepared, pending) = reaper
            .commit_verified_catalog_coverage(reservation, verified)
            .ok()
            .unwrap();
        reaper.advance_catalog_coverage_on_control(id);
        let ack = reaper.catalog_coverage_ack(pending).ok().unwrap();
        assert!(binding.matches_ack(&ack));
        let covered = reaper.consume_catalog_coverage_ack(ack).ok().unwrap();
        assert!(binding.matches_covered(&covered));
        assert_eq!(reaper.allocation_footprint(), baseline);
        // Control-owned unpublished fixture disposal only. No native accepted/
        // final-transfer fact or ClaimUnionSlots release is fabricated here.
    }
    batch.install_domain(admission).ok().unwrap();
    terminal.try_return_stopped_batch(batch).ok().unwrap();
    while reaper.poll_one().unwrap() != hex_stream::DomainPoll::Empty {}
    assert!(matches!(
        reaper.try_reap_domain(id),
        hex_stream::DomainCleanupProgress::Busy { .. }
    ));
    drop(active);
    assert!(matches!(
        reaper.try_reap_domain(id),
        hex_stream::DomainCleanupProgress::Finished(_)
    ));
    let receipt = reaper.stable_catalog_cleanup_receipt(id).unwrap();
    reaper
        .consume_catalog_cleanup_receipt(receipt)
        .ok()
        .unwrap();
    assert!(reaper.take_reclaimed_report(id).is_some());
}
