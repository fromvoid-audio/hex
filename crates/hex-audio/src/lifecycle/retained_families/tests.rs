use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize},
};

struct Payload {
    disposed: Arc<AtomicUsize>,
    ready: AtomicBool,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.disposed.fetch_add(1, Ordering::SeqCst);
    }
}
fn step(value: &mut Payload) -> Result<RetainedFamilyProgress, RetainedFamilyError> {
    Ok(if value.ready.load(Ordering::Acquire) {
        RetainedFamilyProgress::Disposed
    } else {
        RetainedFamilyProgress::Pending
    })
}
fn table() -> RetainedFamilies {
    RetainedFamilies::new(RetainedFamilyReservation {
        family_slots: 2,
        max_table_bytes: 16_384,
    })
    .unwrap()
}
fn source(drops: &Arc<AtomicUsize>) -> Option<Payload> {
    Some(Payload {
        disposed: Arc::clone(drops),
        ready: AtomicBool::new(false),
    })
}
#[test]
fn distinct_families_reserve_before_escape_and_refuse_next_without_losing_pending_owner() {
    let table = table();
    let drops = Arc::new(AtomicUsize::new(0));
    let first = table.reserve().unwrap();
    let second = table.reserve().unwrap();
    assert!(matches!(
        table.reserve(),
        Err(RetainedFamilyError::Reservation)
    ));
    let mut pending = source(&drops);
    assert!(matches!(
        table.register(&first, &mut pending, 0, step),
        Err(RetainedFamilyError::Reservation)
    ));
    assert!(pending.is_some());
    let first_id = table
        .register(&first, &mut pending, usize::MAX, step)
        .unwrap();
    assert!(pending.is_none());
    let mut next = source(&drops);
    let second_id = table
        .register(&second, &mut next, usize::MAX, step)
        .unwrap();
    assert!(matches!(
        table.cancel(&first),
        Err(RetainedFamilyError::Stale)
    ));
    for _ in 0..4 {
        table.advance_one();
    }
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    table
        .read(first_id, |value| value.ready.store(true, Ordering::Release))
        .unwrap();
    for _ in 0..2 {
        table.advance_one();
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(matches!(
        table.read(first_id, |_| ()),
        Err(RetainedFamilyError::Stale)
    ));
    table
        .read(second_id, |value| {
            value.ready.store(true, Ordering::Release)
        })
        .unwrap();
    for _ in 0..2 {
        table.advance_one();
    }
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    let reused = table.reserve().unwrap();
    assert!(reused.generation > first.generation);
    assert!(matches!(
        table.cancel(&first),
        Err(RetainedFamilyError::Stale)
    ));
    table.cancel(&reused).unwrap();
}
#[test]
fn stalled_family_read_skips_cleanup_and_does_not_occupy_an_unrelated_slot() {
    let table = table();
    let drops = Arc::new(AtomicUsize::new(0));
    let token = table.reserve().unwrap();
    let mut pending = source(&drops);
    let id = table
        .register(&token, &mut pending, usize::MAX, step)
        .unwrap();
    table
        .read(id, |value| {
            value.ready.store(true, Ordering::Release);
            // The actual cleanup path cannot wait for this per-family read guard.
            for _ in 0..4 {
                table.advance_one();
            }
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            let unrelated = table.reserve().unwrap();
            table.cancel(&unrelated).unwrap();
            // Shared queries/release bookkeeping may run beside another reader.
            assert!(table.read(id, |_| ()).is_ok());
        })
        .unwrap();
    for _ in 0..2 {
        table.advance_one();
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[test]
fn failed_cleanup_retains_its_exact_owner_and_does_not_block_another_family() {
    fn fails(_: &mut Payload) -> Result<RetainedFamilyProgress, RetainedFamilyError> {
        Err(RetainedFamilyError::Failed)
    }
    let table = table();
    let drops = Arc::new(AtomicUsize::new(0));
    let first = table.reserve().unwrap();
    let second = table.reserve().unwrap();
    let mut pending = source(&drops);
    let failed = table
        .register(&first, &mut pending, usize::MAX, fails)
        .unwrap();
    let mut pending = source(&drops);
    let live = table
        .register(&second, &mut pending, usize::MAX, step)
        .unwrap();
    table
        .read(live, |value| value.ready.store(true, Ordering::Release))
        .unwrap();
    for _ in 0..4 {
        table.advance_one();
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(matches!(
        table.read(failed, |_| ()),
        Err(RetainedFamilyError::Failed)
    ));
    assert!(table.slots[first.index].read().unwrap().job.is_some());
    let replacement = table.reserve().unwrap();
    assert_eq!(replacement.index, second.index);
    table.cancel(&replacement).unwrap();
    // Local harness teardown only; production's table remains in static custody.
}
#[test]
fn cleanup_panic_is_latched_without_retry_or_global_failure() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn panics(_: &mut Payload) -> Result<RetainedFamilyProgress, RetainedFamilyError> {
        CALLS.fetch_add(1, Ordering::SeqCst);
        panic!("intentional control cleanup fault")
    }
    let table = table();
    let drops = Arc::new(AtomicUsize::new(0));
    let token = table.reserve().unwrap();
    let mut pending = source(&drops);
    let id = table
        .register(&token, &mut pending, usize::MAX, panics)
        .unwrap();
    for _ in 0..6 {
        table.advance_one();
    }
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert!(matches!(
        table.read(id, |_| ()),
        Err(RetainedFamilyError::Failed)
    ));
    let slot = table.slots[token.index].read().unwrap();
    assert!(slot.first_panic.is_some());
    assert!(slot.job.is_some());
}
#[test]
fn table_identity_generation_and_component_budgets_are_checked() {
    let table = table();
    let other = self::table();
    let token = table.reserve().unwrap();
    assert_eq!(other.cancel(&token), Err(RetainedFamilyError::Stale));
    let minimum = RetainedFamilies::quote(RetainedFamilyReservation {
        family_slots: 2,
        max_table_bytes: usize::MAX,
    })
    .unwrap();
    assert!(matches!(
        RetainedFamilies::new(RetainedFamilyReservation {
            family_slots: 2,
            max_table_bytes: minimum - 1
        }),
        Err(RetainedFamilyError::Reservation)
    ));
    assert!(
        RetainedFamilies::quote(RetainedFamilyReservation {
            family_slots: usize::MAX,
            max_table_bytes: usize::MAX
        })
        .is_err()
    );
    assert!(
        RetainedFamilies::quote(RetainedFamilyReservation {
            family_slots: 0,
            max_table_bytes: usize::MAX
        })
        .is_err()
    );
}

#[test]
fn lifecycle_retire_and_reopen_dispatch_complete_while_a_family_read_is_held() {
    use super::super::{
        LifecycleMessage, OpenState, enqueue_open, run_lifecycle_with_control_jobs,
    };
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    let table = Arc::new(table());
    let drops = Arc::new(AtomicUsize::new(0));
    let token = table.reserve().unwrap();
    let mut pending = source(&drops);
    let id = table
        .register(&token, &mut pending, usize::MAX, step)
        .unwrap();
    let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<(), ()>>(3);
    let state = Arc::new(OpenState::new());
    let mut open = enqueue_open(&sender, &state, None).unwrap();
    sender.try_send(LifecycleMessage::Retire((), None)).unwrap();
    let occupied = Arc::new(AtomicBool::new(true));
    let observed = Arc::clone(&occupied);
    let opening = Arc::clone(&occupied);
    let scanning = Arc::clone(&table);
    table
        .read(id, |value| {
            value.ready.store(true, Ordering::Release);
            let worker = std::thread::spawn(move || {
                run_lifecycle_with_control_jobs(
                    receiver,
                    || observed.load(Ordering::Acquire),
                    |_, _| (!opening.load(Ordering::Acquire)).then_some(Ok(())),
                    |_| occupied.store(false, Ordering::Release),
                    || scanning.advance_one(),
                );
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(result) = open.try_take().unwrap() {
                    result.unwrap();
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "native dispatch waited for family reader"
                );
                std::thread::yield_now();
            }
            assert_eq!(
                drops.load(Ordering::SeqCst),
                0,
                "the protected family must still exist"
            );
            drop(open);
            drop(sender);
            worker.join().unwrap();
        })
        .unwrap();
    for _ in 0..2 {
        table.advance_one();
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn final_acknowledgment_runs_beside_render_reads_without_losing_release_to_busy() {
    let table = table();
    let drops = Arc::new(AtomicUsize::new(0));
    let token = table.reserve().unwrap();
    let mut pending = source(&drops);
    let id = table
        .register(&token, &mut pending, usize::MAX, step)
        .unwrap();
    table
        .read(id, |_| {
            table
                .acknowledge(id, |value| {
                    value.ready.store(true, Ordering::Release);
                    Ok(())
                })
                .unwrap();
            for _ in 0..2 {
                table.advance_one();
            }
            assert_eq!(drops.load(Ordering::SeqCst), 0);
        })
        .unwrap();
    for _ in 0..2 {
        table.advance_one();
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        table.acknowledge(id, |_| panic!("stale ack must not execute")),
        Err(RetainedFamilyError::Stale)
    );
}

#[test]
fn review_correction_once_construction_serializes_actual_backing_and_conflicting_configuration() {
    use std::sync::Barrier;
    let persistent = Arc::new(OnceLock::new());
    let readiness = Arc::new(super::super::ServiceReadiness::new());
    let constructs = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let overlap = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(8));
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for caller in 0..8 {
            let (persistent, readiness, constructs, active, overlap, starts, barrier) = (
                Arc::clone(&persistent),
                Arc::clone(&readiness),
                Arc::clone(&constructs),
                Arc::clone(&active),
                Arc::clone(&overlap),
                Arc::clone(&starts),
                Arc::clone(&barrier),
            );
            workers.push(scope.spawn(move || {
                barrier.wait();
                configure_table(
                    &persistent,
                    &readiness,
                    || {
                        constructs.fetch_add(1, Ordering::SeqCst);
                        let in_construction = active.fetch_add(1, Ordering::SeqCst) + 1;
                        overlap.fetch_max(in_construction, Ordering::SeqCst);
                        let result = RetainedFamilies::new(RetainedFamilyReservation {
                            family_slots: 2 + caller % 2,
                            max_table_bytes: 16_384,
                        });
                        active.fetch_sub(1, Ordering::SeqCst);
                        result
                    },
                    || {
                        starts.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                )
            }));
        }
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(RetainedFamilyError::Reservation))
                .count(),
            7
        );
    });
    assert_eq!(constructs.load(Ordering::SeqCst), 1);
    assert_eq!(overlap.load(Ordering::SeqCst), 1);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(readiness.ready());
    assert!([2, 3].contains(&persistent_table(&persistent).unwrap().slots.len()));
    assert_eq!(
        configure_table(
            &persistent,
            &readiness,
            || panic!("conflict must not construct"),
            || panic!("conflict must not start")
        ),
        Err(RetainedFamilyError::Reservation)
    );
}

#[test]
fn review_correction_public_facade_sequence_refuses_new_work_after_startup_failure_and_worker_exit()
{
    use super::super::{ServiceExitGuard, ServiceReadiness};
    let persistent = OnceLock::new();
    let failed_start = ServiceReadiness::new();
    assert_eq!(
        configure_table(
            &persistent,
            &failed_start,
            || Ok(table()),
            || Err(RetainedFamilyError::Failed)
        ),
        Err(RetainedFamilyError::Failed)
    );
    assert!(
        persistent_table(&persistent).is_ok(),
        "persistent custody differs from worker readiness"
    );
    assert!(matches!(
        reserve_slot(&persistent, &failed_start),
        Err(RetainedFamilyError::Failed)
    ));
    assert!(!failed_start.ready());

    let live = OnceLock::new();
    let status = ServiceReadiness::new();
    configure_table(&live, &status, || Ok(table()), || Ok(())).unwrap();
    let first = reserve_slot(&live, &status).unwrap();
    let second = reserve_slot(&live, &status).unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let mut existing = source(&drops);
    let id = register_slot(&live, &status, &first, &mut existing, usize::MAX, step).unwrap();
    assert!(existing.is_none());
    // This is the same guard that is the first local in the actual worker.
    drop(ServiceExitGuard(&status));
    assert!(!status.ready());
    status.spawned();
    assert!(
        !status.ready(),
        "a late spawn receipt must not resurrect a dead worker"
    );
    assert!(matches!(
        reserve_slot(&live, &status),
        Err(RetainedFamilyError::Failed)
    ));
    let mut pending = source(&drops);
    assert!(matches!(
        register_slot(&live, &status, &second, &mut pending, usize::MAX, step),
        Err(RetainedFamilyError::Failed)
    ));
    assert!(pending.is_some());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    persistent_table(&live).unwrap().cancel(&second).unwrap();
    assert!(persistent_table(&live).unwrap().read(id, |_| ()).is_ok());
    assert!(
        persistent_table(&live).unwrap().slots[first.index]
            .read()
            .unwrap()
            .job
            .is_some()
    );
}

#[test]
fn review_correction_final_payload_drop_panic_retains_failed_slot_and_native_dispatch_progress() {
    use super::super::{
        LifecycleMessage, OpenState, enqueue_open, run_lifecycle_with_control_jobs,
    };
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    struct FinalPanic(Arc<AtomicUsize>);
    impl Drop for FinalPanic {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("intentional final payload destructor fault");
        }
    }
    fn disposed(_: &mut FinalPanic) -> Result<RetainedFamilyProgress, RetainedFamilyError> {
        Ok(RetainedFamilyProgress::Disposed)
    }
    let table = Arc::new(table());
    let calls = Arc::new(AtomicUsize::new(0));
    let first = table.reserve().unwrap();
    let mut source = Some(FinalPanic(Arc::clone(&calls)));
    let failed = table
        .register(&first, &mut source, usize::MAX, disposed)
        .unwrap();
    let original_body = table
        .inspect_on_control(failed, |job, _| job.actual_body_layout_on_control().size())
        .unwrap();
    table.advance_one();
    assert_eq!(
        table.unresolved_body_reservation(failed),
        Ok(Some(original_body))
    );
    assert!(matches!(
        table.inspect_on_control(failed, |_, _| panic!(
            "consumed object must not be inspected"
        )),
        Err(RetainedFamilyError::Failed)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    {
        let slot = table.slots[first.index].read().unwrap();
        assert!(matches!(slot.phase, Phase::Failed));
        assert!(
            slot.job.is_none(),
            "partially destroyed owner is not retried/relabelled known"
        );
        assert!(slot.first_panic.is_some());
    }
    let second = table.reserve().unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let mut pending = self::source(&drops);
    let live = table
        .register(&second, &mut pending, usize::MAX, step)
        .unwrap();
    table
        .read(live, |value| value.ready.store(true, Ordering::Release))
        .unwrap();
    let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<(), ()>>(3);
    let state = Arc::new(OpenState::new());
    let mut open = enqueue_open(&sender, &state, None).unwrap();
    assert!(sender.try_send(LifecycleMessage::Retire((), None)).is_ok());
    let occupied = Arc::new(AtomicBool::new(true));
    let observing = Arc::clone(&occupied);
    let opening = Arc::clone(&occupied);
    let scanning = Arc::clone(&table);
    let worker = std::thread::spawn(move || {
        run_lifecycle_with_control_jobs(
            receiver,
            || observing.load(Ordering::Acquire),
            |_, _| (!opening.load(Ordering::Acquire)).then_some(Ok(())),
            |_| occupied.store(false, Ordering::Release),
            || scanning.advance_one(),
        )
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(result) = open.try_take().unwrap() {
            result.unwrap();
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(open);
    drop(sender);
    worker.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(matches!(
        table.read(failed, |_| ()),
        Err(RetainedFamilyError::Failed)
    ));
    let available = table.reserve().unwrap();
    assert_eq!(available.index, second.index);
    assert!(
        matches!(table.reserve(), Err(RetainedFamilyError::Reservation)),
        "failed final Drop must remain charged/occupied"
    );
}

#[test]
fn retained_job_failed_family_keeps_actual_body_available_for_control_inspection() {
    fn refuses(_: &mut Payload) -> Result<RetainedFamilyProgress, RetainedFamilyError> {
        Err(RetainedFamilyError::Failed)
    }
    let table = table();
    let drops = Arc::new(AtomicUsize::new(0));
    let token = table.reserve().unwrap();
    let mut pending = source(&drops);
    let id = table
        .register(&token, &mut pending, usize::MAX, refuses)
        .unwrap();
    table.advance_one();
    assert!(matches!(
        table.read(id, |_| ()),
        Err(RetainedFamilyError::Failed)
    ));
    table
        .inspect_on_control(id, |actual, payload| {
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            assert!(!payload.ready.load(Ordering::Acquire));
            let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 8,
                    handle_rows: 8,
                    root_rows: 2,
                    max_retained_bytes: 1_000_000,
                    max_construction_peak_bytes: 1_000_000,
                },
            )
            .unwrap();
            if hex_engine::retained_control_job_box_layout_supported() {
                eprintln!("SUPPORTED retained-job failed family actual outer accounting executed");
                actual.visit_outer_storage_on_control(&mut ledger).unwrap();
                assert_eq!(
                    ledger.summary().unwrap().data_bytes,
                    actual.actual_body_layout_on_control().size()
                );
            } else {
                assert!(actual.visit_outer_storage_on_control(&mut ledger).is_err());
                eprintln!("unsupported retained-job outer accounting refused");
            }
        })
        .unwrap();
    for _ in 0..4 {
        table.advance_one();
    }
    assert_eq!(drops.load(Ordering::SeqCst), 0);
}

#[test]
fn neutral_route_lookup_revalidates_actual_replacement_job_and_refuses_stale_hint_or_type() {
    let table = table();
    let drops = Arc::new(AtomicUsize::new(0));
    let slot = table.reserve().unwrap();
    let first = table
        .register(&slot, &mut source(&drops), usize::MAX, step)
        .unwrap();
    let route = table.registered_route(first).unwrap();
    assert_eq!(
        table.read_route::<Payload, _>(route, |value| value.ready.load(Ordering::Acquire)),
        Ok(false)
    );
    assert_eq!(
        table.read_route::<String, _>(route, |_| ()),
        Err(RetainedFamilyError::Stale)
    );
    table
        .acknowledge_route::<Payload>(route, |value| {
            value.ready.store(true, Ordering::Release);
            Ok(())
        })
        .unwrap();
    table.advance_one();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        table.read_route::<Payload, _>(route, |_| ()),
        Err(RetainedFamilyError::Stale)
    );
    let replacement_slot = table.reserve().unwrap();
    assert_eq!(replacement_slot.index, slot.index);
    let replacement = table
        .register(&replacement_slot, &mut source(&drops), usize::MAX, step)
        .unwrap();
    let replacement_route = table.registered_route(replacement).unwrap();
    assert_ne!(route, replacement_route);
    // A stale/misleading hint cannot validate the replacement's actual holder.
    table.slots[replacement_slot.index].hint.publish(route);
    assert_eq!(
        table.read_route::<Payload, _>(route, |_| panic!("stale dispatch")),
        Err(RetainedFamilyError::Stale)
    );
    assert_eq!(
        table.acknowledge_route::<Payload>(route, |_| panic!("stale ACK")),
        Err(RetainedFamilyError::Stale)
    );
    assert!(
        !table
            .read(replacement, |value| value.ready.load(Ordering::Acquire))
            .unwrap()
    );
    table.slots[replacement_slot.index]
        .hint
        .publish(replacement_route);
    assert_eq!(
        table.read_route::<Payload, _>(replacement_route, |_| 17),
        Ok(17)
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn neutral_route_ack_uses_shared_guard_without_worker_readiness_and_failed_jobs_refuse() {
    let table = table();
    let drops = Arc::new(AtomicUsize::new(0));
    let slot = table.reserve().unwrap();
    let id = table
        .register(&slot, &mut source(&drops), usize::MAX, step)
        .unwrap();
    let route = table.registered_route(id).unwrap();
    table
        .read_route::<Payload, _>(route, |_| {
            // The actual outer shared guard is held. ACK acquires another shared
            // guard, never a try-write, owning enqueue or native/device barrier.
            table
                .acknowledge_route::<Payload>(route, |value| {
                    value.ready.store(true, Ordering::Release);
                    Ok(())
                })
                .unwrap();
        })
        .unwrap();
    // Same production lookup/ACK kernels intentionally have no readiness input.
    table.slots[slot.index].write().unwrap().phase = Phase::Failed;
    assert_eq!(
        table.acknowledge_route::<Payload>(route, |_| panic!("failed ACK")),
        Err(RetainedFamilyError::Failed)
    );
    assert_eq!(
        table.read_route::<Payload, _>(route, |_| panic!("failed read")),
        Err(RetainedFamilyError::Failed)
    );
    assert!(table.slots[slot.index].read().unwrap().job.is_some());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
}
