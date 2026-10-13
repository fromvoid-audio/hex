use super::*;
use hex_stream::{
    DomainCleanupProgress, DomainId, DomainPoll, DomainReturnStatus, MediaCleanupPhase,
    MediaDomainInput, ProtectionRole, domain_escrow,
};

#[test]
fn busy_preflight_preserves_real_installed_protection_and_all_metadata_until_retry() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (control, mut readiness, key) = readiness_cases::pair(&wav, pcm.len(), 4);
    let wanted = readiness_cases::manifest(&control, key, 1, 0);
    readiness_cases::install(&mut readiness, ProtectionRole::Active, &wanted);
    let original = readiness.allocation_footprint();
    let id = DomainId::new(1).unwrap();
    let retained_leases = vec![
        control.catalog_lease().unwrap(),
        control.catalog_lease().unwrap(),
    ];
    let (mut batch, mut terminal, mut root) = domain_escrow(
        vec![MediaDomainInput {
            id,
            control,
            endpoint: readiness,
            retained_leases,
        }],
        1,
    )
    .unwrap();
    drop(root.observer()); // Interest is independent from the actual control root.
    assert_eq!(batch.try_return_domain(id), DomainReturnStatus::Accepted);
    assert_eq!(root.poll_one().unwrap(), DomainPoll::DomainReturned(id));
    assert_eq!(
        root.try_reap_domain(id),
        DomainCleanupProgress::Busy {
            external_references: 1
        }
    );
    let returned = root.returned_readiness(id).unwrap();
    assert_eq!(returned.allocation_footprint(), original);
    assert!(returned.installed(ProtectionRole::Active, &wanted));
    assert!(
        returned
            .acknowledge(ProtectionRole::Active, &wanted)
            .is_some()
    );
    assert_eq!(
        &returned.pages().page(key, 0).unwrap().frames[..256],
        &pcm[..256]
    );
    // Every known lease is still retained: two internal leases are excluded
    // from Busy, only the original external manifest counts as external.
    assert_eq!(root.retained_lease_count(id), Some(2));
    assert_eq!(
        root.try_reap_domain(id),
        DomainCleanupProgress::Busy {
            external_references: 1
        }
    );
    terminal.try_return_stopped_batch(batch).unwrap();
    assert_eq!(root.poll_one().unwrap(), DomainPoll::TerminalReturned);
    assert_eq!(root.phase(), MediaCleanupPhase::Pending);
    drop(wanted);
    let DomainCleanupProgress::Finished(report) = root.try_reap_domain(id) else {
        panic!("cleanup")
    };
    let report = report.successful_report().unwrap();
    assert_eq!(root.retained_lease_count(id), Some(0));
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(
        report.page_buffers,
        report.queued_pages_reclaimed
            + report.held_return_pages_reclaimed
            + report.worker_pages_reclaimed
    );
    assert_eq!(root.phase(), MediaCleanupPhase::Complete);
}

#[test]
fn separate_actual_catalogs_with_identical_source_keys_are_not_shared_or_undercharged() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (first_control, first_endpoint, first_key) = readiness_cases::pair(&wav, pcm.len(), 4);
    let (second_control, second_endpoint, second_key) = readiness_cases::pair(&wav, pcm.len(), 4);
    assert_eq!(first_key, second_key);
    assert!(
        !first_endpoint
            .catalog()
            .same_catalog(second_endpoint.catalog())
    );
    let expected = first_endpoint.retained_data_bytes().unwrap()
        + second_endpoint.retained_data_bytes().unwrap();
    let inputs = vec![
        MediaDomainInput {
            id: DomainId::new(1).unwrap(),
            control: first_control,
            endpoint: first_endpoint,
            retained_leases: Vec::new(),
        },
        MediaDomainInput {
            id: DomainId::new(2).unwrap(),
            control: second_control,
            endpoint: second_endpoint,
            retained_leases: Vec::new(),
        },
    ];
    let (batch, mut terminal, mut root) = domain_escrow(inputs, 1).unwrap();
    assert_eq!(
        root.allocation_footprint().domain_data_bytes,
        Some(expected)
    );
    terminal.try_return_stopped_batch(batch).unwrap();
    assert_eq!(root.poll_one().unwrap(), DomainPoll::TerminalReturned);
    for id in [1, 2] {
        assert!(
            matches!(root.try_reap_domain(DomainId::new(id).unwrap()),DomainCleanupProgress::Finished(report) if report.successful_report().is_some())
        );
    }
    assert_eq!(root.phase(), MediaCleanupPhase::Complete);
}
