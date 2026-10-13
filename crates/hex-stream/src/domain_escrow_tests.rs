use super::*;
use crate::{StreamConfig, StreamRuntime, VerifiedPageEndpoint, WorkerJoinObservation};
use std::{thread, time::Duration};

fn input(id: u64) -> MediaDomainInput {
    let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(4), Vec::new()).unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, pages) = runtime.split().ok().unwrap();
    let endpoint = VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap();
    MediaDomainInput {
        id: DomainId::new(id).unwrap(),
        endpoint: PageReadiness::new(endpoint, 3).unwrap(),
        retained_leases: vec![control.catalog_lease().unwrap()],
        control,
    }
}
fn reclaim(input: MediaDomainInput) {
    let (pages, catalog) = input.endpoint.into_endpoint().into_parts();
    drop((catalog, input.retained_leases));
    assert!(input.control.shutdown(pages).unwrap().accounting_complete);
}
fn finish(
    batch: Box<CallbackDomainBatch>,
    mut terminal: TerminalBatchSender,
    mut root: MediaDomainReaper,
) {
    terminal.try_return_stopped_batch(batch).unwrap();
    while root.poll_one().unwrap() != DomainPoll::Empty {}
    let ids: Vec<_> = root.slots.iter().map(|slot| slot.id).collect();
    for id in ids {
        let DomainCleanupProgress::Finished(report) = root.try_reap_domain(id) else {
            panic!("cleanup")
        };
        assert!(report.successful_report().is_some());
    }
    assert_eq!(root.phase(), MediaCleanupPhase::Complete);
}

#[test]
fn every_fallible_setup_reservation_preserves_original_input_allocations_and_owners() {
    for failure in 0..2 {
        let mut inputs = Vec::with_capacity(16);
        inputs.push(input(1));
        inputs.push(input(2));
        let pointer = inputs.as_ptr();
        let capacity = inputs.capacity();
        let lease_pointers: Vec<_> = inputs
            .iter()
            .map(|input| input.retained_leases.as_ptr())
            .collect();
        let reports: Vec<_> = inputs
            .iter()
            .map(|input| input.endpoint.stream_footprint())
            .collect();
        let mut attempts = 0;
        let error = setup(inputs, 1, || {
            let current = attempts;
            attempts += 1;
            if current == failure { Err(()) } else { Ok(()) }
        })
        .err()
        .unwrap();
        let (inputs, reason, observation) = error.into_parts();
        assert_eq!(reason, DomainRefusal::Allocation);
        assert_eq!(inputs.as_ptr(), pointer);
        assert_eq!(inputs.capacity(), capacity);
        assert_eq!(attempts, failure + 1);
        assert_eq!(observation.control_slots.capacity != 0, failure == 1);
        assert_eq!(observation.callback_slots.capacity, 0);
        for (index, input) in inputs.into_iter().enumerate() {
            assert_eq!(input.retained_leases.as_ptr(), lease_pointers[index]);
            assert_eq!(input.endpoint.stream_footprint(), reports[index]);
            reclaim(input);
        }
    }
}

#[test]
fn borrowed_validation_refuses_wrong_pairs_leases_duplicate_ids_and_capacity() {
    for case in 0..4 {
        let mut inputs = vec![input(1), input(2)];
        if case == 0 {
            let (a, b) = inputs.split_at_mut(1);
            std::mem::swap(&mut a[0].control, &mut b[0].control);
        }
        if case == 1 {
            let (a, b) = inputs.split_at_mut(1);
            std::mem::swap(&mut a[0].retained_leases, &mut b[0].retained_leases);
        }
        if case == 2 {
            inputs[1].id = inputs[0].id;
        }
        let pointer = inputs.as_ptr();
        let error = domain_escrow(inputs, if case == 3 { usize::MAX } else { 1 })
            .err()
            .unwrap();
        let (mut inputs, reason, observation) = error.into_parts();
        assert_eq!(
            reason,
            [
                DomainRefusal::WrongPair,
                DomainRefusal::WrongLease,
                DomainRefusal::DuplicateIdentity,
                DomainRefusal::InvalidCapacity
            ][case]
        );
        assert_eq!(inputs.as_ptr(), pointer);
        assert_eq!(observation.control_slots.capacity, 0);
        if case == 0 {
            let (a, b) = inputs.split_at_mut(1);
            std::mem::swap(&mut a[0].control, &mut b[0].control);
        }
        if case == 1 {
            let (a, b) = inputs.split_at_mut(1);
            std::mem::swap(&mut a[0].retained_leases, &mut b[0].retained_leases);
        }
        for input in inputs {
            reclaim(input);
        }
    }
}

#[test]
fn full_normal_return_ring_cannot_steal_terminal_slot_or_drop_same_slot_owner() {
    let (mut batch, terminal, root) = domain_escrow(vec![input(1), input(2)], 1).unwrap();
    let second = DomainId::new(2).unwrap();
    let original = batch.readiness_mut(second).unwrap().allocation_footprint();
    assert_eq!(
        batch.try_return_domain(DomainId::new(1).unwrap()),
        DomainReturnStatus::Accepted
    );
    assert_eq!(batch.try_return_domain(second), DomainReturnStatus::Pending);
    assert_eq!(
        batch.readiness_mut(second).unwrap().allocation_footprint(),
        original
    );
    // Terminal succeeds despite saturated normal ring. It contains remaining
    // owners; normal returned payload remains separately owned until polled.
    finish(batch, terminal, root);
}

#[test]
fn disconnected_normal_and_terminal_queues_preserve_callback_batch_owners() {
    let (mut batch, mut terminal, root) = domain_escrow(vec![input(1)], 1).unwrap();
    let id = DomainId::new(1).unwrap();
    let original = batch.readiness_mut(id).unwrap().allocation_footprint();
    drop(root); // control-side only; callback batch retains catalog/worker owners.
    assert_eq!(
        batch.try_return_domain(id),
        DomainReturnStatus::Refused(DomainRefusal::QueueClosed)
    );
    let error = terminal.try_return_stopped_batch(batch).unwrap_err();
    assert_eq!(error.reason, DomainRefusal::QueueClosed);
    let mut batch = error.into_batch();
    assert_eq!(
        batch.readiness_mut(id).unwrap().allocation_footprint(),
        original
    );
    drop(batch); // explicit control-side final owner reclamation.
}

#[test]
fn wrong_escrow_terminal_refusal_and_duplicate_payloads_are_owning_errors() {
    let (mut batch, mut terminal, mut root) = domain_escrow(vec![input(1)], 1).unwrap();
    let (other, other_terminal, other_root) = domain_escrow(vec![input(1)], 1).unwrap();
    let error = terminal.try_return_stopped_batch(other).unwrap_err();
    assert_eq!(error.reason, DomainRefusal::WrongEscrow);
    let mut other = error.into_batch();
    let id = DomainId::new(1).unwrap();
    assert_eq!(batch.try_return_domain(id), DomainReturnStatus::Accepted);
    assert_eq!(root.poll_one().unwrap(), DomainPoll::DomainReturned(id));
    // Inject a duplicate ID/index using another actual endpoint owner. This
    // cannot overwrite the already-returned endpoint; the payload is recovered.
    let endpoint = other.slots[0].endpoint.take().unwrap();
    batch
        .returns
        .push(ReturnedMediaDomain {
            index: 0,
            id,
            endpoint,
        })
        .ok()
        .unwrap();
    let error = root.poll_one().unwrap_err();
    assert_eq!(error.reason, DomainRefusal::AlreadyReturned);
    let RejectedMediaReturn::Domain(returned) = error.into_returned() else {
        panic!("domain")
    };
    other.slots[0].endpoint = Some(returned.into_parts().1);
    finish(other, other_terminal, other_root);
    finish(batch, terminal, root);
}

#[test]
fn capacity_report_counts_actual_slots_and_each_catalog_once_not_per_lease() {
    let first = input(1);
    let original = first.endpoint.retained_data_bytes().unwrap();
    let lease_bytes = AllocationCapacity::of::<CatalogLease>(first.retained_leases.capacity())
        .bytes()
        .unwrap();
    let (batch, terminal, root) = domain_escrow(vec![first], 1).unwrap();
    let report = root.allocation_footprint();
    assert_eq!(report.control_slots.capacity, root.slots.capacity());
    assert_eq!(report.callback_slots.capacity, batch.slots.capacity());
    assert_eq!(
        report.normal_return_slots.capacity,
        root.returns.buffer().capacity()
    );
    assert_eq!(
        report.terminal_return_slots.capacity,
        root.terminal.buffer().capacity()
    );
    assert_eq!(report.domain_data_bytes, Some(original + lease_bytes));
    assert!(report.construction_data_bound().unwrap() >= report.retained_data_bytes().unwrap());
    eprintln!(
        "domain escrow fixture capacity: {report:?}; retained={:?}; construction_bound={:?}",
        report.retained_data_bytes(),
        report.construction_data_bound()
    );
    finish(batch, terminal, root);
}

fn damaged_input(panic_worker: bool) -> MediaDomainInput {
    use crate::{
        TransportSetupObservation,
        construction_transport::{PreparedTransport, SetupFaults},
    };
    let config = StreamConfig::with_pool_pages(4);
    let mut observation = TransportSetupObservation::default();
    let transport =
        PreparedTransport::build(config, &[], &mut observation, &SetupFaults::default()).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut free = transport.free_pages;
    let mut queues = transport.worker_queues;
    let worker = thread::spawn(move || {
        if panic_worker {
            queues.ready.push(free.pop().unwrap()).ok().unwrap();
            panic!("injected terminal worker panic");
        }
        drop(free.pop());
        free
    });
    let catalog = crate::types::VerifiedCatalog {
        sources: Vec::new(),
        footprint: Default::default(),
        allocations: Default::default(),
        construction: Default::default(),
    };
    let runtime = StreamRuntime::from_construction(
        config,
        observation.transport,
        transport.endpoint,
        stop,
        worker,
        catalog,
    );
    let (control, pages) = runtime.split().ok().unwrap();
    let endpoint = VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap();
    MediaDomainInput {
        id: DomainId::new(1).unwrap(),
        control,
        endpoint: PageReadiness::new(endpoint, 3).unwrap(),
        retained_leases: Vec::new(),
    }
}

#[test]
fn terminal_panic_and_imbalance_keep_actual_partial_observations_and_legacy_mapping() {
    for panic_worker in [false, true] {
        let (batch, mut terminal, mut root) =
            domain_escrow(vec![damaged_input(panic_worker)], 1).unwrap();
        let observer = root.observer();
        terminal.try_return_stopped_batch(batch).unwrap();
        root.poll_one().unwrap();
        let DomainCleanupProgress::Finished(report) =
            root.try_reap_domain(DomainId::new(1).unwrap())
        else {
            panic!("report")
        };
        assert!(report.successful_report().is_none());
        if panic_worker {
            assert_eq!(report.join, WorkerJoinObservation::Panicked);
            assert_eq!(report.queued_pages_reclaimed, 1);
            assert_eq!(report.worker_pages_reclaimed, None);
            assert_eq!(report.conservation, None);
            assert_eq!(
                report.failure,
                Some(crate::StreamRuntimeError::WorkerJoinFailed)
            );
        } else {
            assert_eq!(report.join, WorkerJoinObservation::Joined);
            assert_eq!(report.worker_pages_reclaimed, Some(3));
            assert_eq!(report.conservation, Some(false));
            assert_eq!(
                report.failure,
                Some(crate::StreamRuntimeError::PoolAccounting)
            );
        }
        assert_eq!(observer.phase(), MediaCleanupPhase::FailedIncomplete);
        assert_eq!(root.report(DomainId::new(1).unwrap()), Some(report));
        // The legacy API retains its existing error mapping on the same faults.
        let input = damaged_input(panic_worker);
        let (pages, lease) = input.endpoint.into_endpoint().into_parts();
        drop(lease);
        let error = input.control.shutdown(pages).unwrap_err();
        assert!(matches!(error,ShutdownError::Runtime(kind) if Some(kind) == report.failure));
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn callback_normal_and_terminal_return_moves_allocate_free_and_clone_nothing() {
    let (mut batch, mut terminal, mut root) = domain_escrow(vec![input(1), input(2)], 1).unwrap();
    let info = allocation_counter::measure(|| {
        assert_eq!(
            batch.try_return_domain(DomainId::new(1).unwrap()),
            DomainReturnStatus::Accepted
        );
        assert_eq!(
            batch.try_return_domain(DomainId::new(2).unwrap()),
            DomainReturnStatus::Pending
        );
    });
    assert_eq!(info.count_total, 0);
    assert_eq!(info.bytes_total, 0);
    assert_eq!(info.count_current, 0);
    assert_eq!(info.bytes_current, 0);
    let mut batch = Some(batch);
    let info = allocation_counter::measure(|| {
        terminal
            .try_return_stopped_batch(batch.take().unwrap())
            .unwrap()
    });
    assert_eq!(info.count_total, 0);
    assert_eq!(info.bytes_total, 0);
    assert_eq!(info.count_current, 0);
    assert_eq!(info.bytes_current, 0);
    while root.poll_one().unwrap() != DomainPoll::Empty {}
    for id in [1, 2] {
        assert!(
            matches!(root.try_reap_domain(DomainId::new(id).unwrap()),DomainCleanupProgress::Finished(report) if report.successful_report().is_some())
        );
    }
    assert_eq!(root.phase(), MediaCleanupPhase::Complete);
}

#[test]
fn observer_abandonment_cannot_cancel_root_and_terminal_is_required_before_complete() {
    let (mut batch, terminal, mut root) = domain_escrow(vec![input(1)], 1).unwrap();
    drop(root.observer());
    assert_eq!(
        batch.try_return_domain(DomainId::new(1).unwrap()),
        DomainReturnStatus::Accepted
    );
    root.poll_one().unwrap();
    assert!(
        matches!(root.try_reap_domain(DomainId::new(1).unwrap()),DomainCleanupProgress::Finished(report) if report.successful_report().is_some())
    );
    assert_eq!(root.phase(), MediaCleanupPhase::Pending);
    finish(batch, terminal, root);
}

#[test]
fn once_only_terminal_sender_rejects_a_second_transfer_without_consuming_its_batch() {
    let (batch, mut terminal, mut root) = domain_escrow(vec![input(1)], 1).unwrap();
    terminal.try_return_stopped_batch(batch).unwrap();
    let (other, other_terminal, other_root) = domain_escrow(vec![input(2)], 1).unwrap();
    let pointer = &*other as *const CallbackDomainBatch;
    let error = terminal.try_return_stopped_batch(other).unwrap_err();
    assert_eq!(error.reason, DomainRefusal::TerminalAlreadyReturned);
    let other = error.into_batch();
    assert_eq!(&*other as *const CallbackDomainBatch, pointer);
    finish(other, other_terminal, other_root);
    assert_eq!(root.poll_one().unwrap(), DomainPoll::TerminalReturned);
    assert!(
        matches!(root.try_reap_domain(DomainId::new(1).unwrap()), DomainCleanupProgress::Finished(report) if report.successful_report().is_some())
    );
    assert_eq!(root.phase(), MediaCleanupPhase::Complete);
}

#[test]
fn headless_lifecycle_harness_keeps_lease_through_stop_failure_busy_and_cleanup() {
    // Protocol fixture only: no CPAL calls or claim of native stop guarantees.
    struct Harness {
        batch: Option<Box<CallbackDomainBatch>>,
        terminal: TerminalBatchSender,
        root: MediaDomainReaper,
        lease: bool,
        events: Vec<&'static str>,
    }
    impl Harness {
        fn retire_tick(&mut self, stop_confirmed: bool) {
            if self.batch.is_some() {
                if !stop_confirmed {
                    self.events.push("stop-failed");
                    return;
                }
                self.events.push("stop-confirmed");
                // Mock closure disposal returns the actual still-owned batch.
                let batch = self.batch.take().unwrap();
                self.events.push("closure-disposed");
                self.terminal.try_return_stopped_batch(batch).unwrap();
                assert_eq!(self.root.poll_one().unwrap(), DomainPoll::TerminalReturned);
                self.events.push("terminal-received");
            }
            match self.root.try_reap_domain(DomainId::new(1).unwrap()) {
                DomainCleanupProgress::Busy { .. } => self.events.push("busy"),
                DomainCleanupProgress::Finished(report) if report.successful_report().is_some() => {
                    self.events.push("cleanup-conserved")
                }
                other => panic!("unexpected cleanup: {other:?}"),
            }
            if self.root.phase() == MediaCleanupPhase::Complete {
                self.lease = false;
                self.events.push("lease-released");
            }
        }
    }
    let input = input(1);
    let external = input.control.catalog_lease().unwrap();
    let (batch, terminal, root) = domain_escrow(vec![input], 1).unwrap();
    let observer = root.observer();
    let mut harness = Harness {
        batch: Some(batch),
        terminal,
        root,
        lease: true,
        events: Vec::new(),
    };
    harness.retire_tick(false);
    assert!(harness.lease && harness.batch.is_some());
    assert_eq!(harness.root.phase(), MediaCleanupPhase::Pending);
    assert!(
        harness
            .batch
            .as_mut()
            .unwrap()
            .readiness_mut(DomainId::new(1).unwrap())
            .is_some()
    );
    harness.retire_tick(false);
    harness.retire_tick(true);
    assert!(harness.lease && harness.batch.is_none());
    assert_eq!(
        harness.root.retained_lease_count(DomainId::new(1).unwrap()),
        Some(1)
    );
    drop(external);
    harness.retire_tick(true);
    assert!(!harness.lease);
    assert_eq!(observer.phase(), MediaCleanupPhase::Complete);
    assert_eq!(
        harness.events,
        [
            "stop-failed",
            "stop-failed",
            "stop-confirmed",
            "closure-disposed",
            "terminal-received",
            "busy",
            "cleanup-conserved",
            "lease-released"
        ]
    );
}
