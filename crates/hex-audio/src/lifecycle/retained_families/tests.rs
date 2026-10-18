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
