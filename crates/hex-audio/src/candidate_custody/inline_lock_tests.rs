use super::*;

pub(super) fn held_on_other_thread<T: Send>(value: &CustodyMutex<T>, inspect: impl FnOnce()) {
    std::thread::scope(|scope| {
        let (held_tx, held_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let holder = scope.spawn(move || {
            let guard = lock(value);
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        });
        held_rx.recv().unwrap();
        inspect(); // release cannot happen until this nonblocking consumer returns
        release_tx.send(()).unwrap();
        holder.join().unwrap();
    });
}

#[test]
fn actual_five_field_first_acquisitions_and_root_drop_need_no_warmup_allocations() {
    let mut root = None;
    let birth = allocation_counter::measure(|| root = Some(CandidateRoot::new(2).unwrap()));
    // Actual payload allocations are separate: slots Vec, Inner Arc and the
    // two existing DomainSignal Arcs. No heap child belongs to these locks.
    assert_eq!(birth.count_total, 4);
    assert_eq!(birth.count_current, 4);
    let root = root.take().unwrap();
    let measured = allocation_counter::measure(|| {
        for _ in 0..128 {
            root.0.bank.try_lock().map(drop).unwrap();
            root.0.execution.try_lock().map(drop).unwrap();
            root.0.returns.try_lock().map(drop).unwrap();
            drop(lock(&root.0.bank));
            drop(lock(&root.0.execution));
            drop(lock(&root.0.returns));
            root.0.eligible.test_inline_native_exclusions();
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let destruction = allocation_counter::measure(|| drop(root));
    assert_eq!(destruction.count_total, 0);
    assert_eq!(destruction.count_current, -4);
    assert_eq!(destruction.bytes_current, -(birth.bytes_total as i64));
    eprintln!(
        "SUPPORTED actual five inline fields: constructor payload4 allocations/{}bytes separated; first+128repeat try/control acquisitions/releases0; finaldrop frees exactly original4; actual Inner{} guard{} (future whole-operation scratch still required)",
        birth.bytes_total,
        std::mem::size_of::<Inner>(),
        std::mem::size_of::<CustodyMutexGuard<'_, Bank>>()
    );
}

#[test]
fn actual_three_outer_exclusions_refuse_nonblocking_consumers_keep_returned_owner() {
    for which in 0..3 {
        let Some(owner) = crate::prepared::candidate_tests::candidate() else {
            return;
        };
        let identity = owner.identity();
        let root = CandidateRoot::new(2).unwrap();
        let (queued, ticket) = root
            .stage(owner, vec![], Ok::<_, QueuedCandidate>)
            .ok()
            .unwrap();
        root.ingest(queued.reject_before_install()).ok().unwrap();
        let inspect = || {
            let measured = allocation_counter::measure(|| match which {
                0 => assert!(root.recover(&ticket).is_none()),
                1 => root.advance_on_control(),
                _ => assert!(!root.drain_abandoned_on_control()),
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        };
        match which {
            0 => held_on_other_thread(&root.0.bank, inspect),
            1 => held_on_other_thread(&root.0.execution, inspect),
            _ => held_on_other_thread(&root.0.returns, inspect),
        }
        {
            let bank = lock(&root.0.bank);
            assert_eq!(
                bank.slots[ticket.stamp.slot]
                    .returned
                    .as_ref()
                    .unwrap()
                    .owner
                    .identity(),
                identity
            );
            assert_eq!(bank.slots[ticket.stamp.slot].phase, Phase::Returned);
        }
        root.advance_on_control();
        let recovered = root
            .recover(&ticket)
            .expect("control progresses after actual exclusion release");
        assert_eq!(recovered.disposed.identity(), identity);
    }
    eprintln!(
        "SUPPORTED actual outer bank/execution/returns other-thread contention: real recovery/cleanup/drain returns before release; allocation/free0 and SAME returned owner retained then retired/recovered"
    );
}
