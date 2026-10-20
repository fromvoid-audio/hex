use super::*;
use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[test]
fn inline_send_only_payload_first_repeat_and_contended_try_do_not_allocate() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<CustodyMutex<Cell<u32>>>();
    let value = CustodyMutex::new(Cell::new(0));
    let measured = allocation_counter::measure(|| {
        for n in 0..128 {
            let guard = value.try_lock().unwrap();
            guard.set(n);
            assert!(matches!(
                value.try_lock(),
                Err(CustodyTryLockError::Contended)
            ));
            drop(guard);
            let guard = value.lock_recover_on_control();
            assert_eq!(guard.get(), n);
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
    eprintln!(
        "SUPPORTED inline Send-only Cell: first+repeat/contended one-try/control recovery/release allocation/free0 without prewarm"
    );
}
#[test]
fn escaping_panic_poison_is_sticky_and_control_recovery_keeps_payload() {
    let value = CustodyMutex::new(17);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let mut guard = value.try_lock().unwrap();
        *guard = 23;
        std::panic::panic_any("first custody failure");
    }))
    .unwrap_err();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"first custody failure"));
    assert!(matches!(
        value.try_lock(),
        Err(CustodyTryLockError::Poisoned)
    ));
    {
        let mut guard = value.lock_recover_on_control();
        assert_eq!(*guard, 23);
        *guard = 29;
    }
    assert!(matches!(
        value.try_lock(),
        Err(CustodyTryLockError::Poisoned)
    ));
    assert_eq!(*value.lock_recover_on_control(), 29);
}
#[test]
fn caught_panic_while_guard_lives_does_not_poison() {
    let value = CustodyMutex::new(7);
    let mut guard = value.try_lock().unwrap();
    assert!(catch_unwind(|| panic!("caught before guard drop")).is_err());
    *guard = 9;
    drop(guard);
    assert_eq!(*value.try_lock().unwrap(), 9);
}
#[test]
fn acquired_during_unwind_does_not_add_poison() {
    struct AcquireOnDrop<'a>(&'a CustodyMutex<u32>);
    impl Drop for AcquireOnDrop<'_> {
        fn drop(&mut self) {
            assert!(std::thread::panicking());
            *self.0.lock_recover_on_control() = 31;
        }
    }
    let value = CustodyMutex::new(0);
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _drop = AcquireOnDrop(&value);
            panic!("already unwinding");
        }))
        .is_err()
    );
    assert_eq!(*value.try_lock().unwrap(), 31);
}

#[test]
fn poisoned_try_releases_without_allocating_or_destroying_payload() {
    use std::sync::atomic::AtomicUsize;
    struct Payload<'a>(&'a AtomicUsize);
    impl Drop for Payload<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let drops = AtomicUsize::new(0);
    let value = CustodyMutex::new(Payload(&drops));
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = value.try_lock().unwrap();
            panic!("poison without payload destruction");
        }))
        .is_err()
    );
    let measured = allocation_counter::measure(|| {
        for _ in 0..128 {
            assert!(matches!(
                value.try_lock(),
                Err(CustodyTryLockError::Poisoned)
            ));
            assert_eq!(drops.load(Ordering::Relaxed), 0);
            drop(value.lock_recover_on_control());
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
    drop(value);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn control_progress_after_other_thread_release_preserves_send_only_cell() {
    use std::sync::{Arc, mpsc};
    let value = Arc::new(CustodyMutex::new(Cell::new(0)));
    let held = value.try_lock().unwrap();
    let (entered_tx, entered_rx) = mpsc::sync_channel(0);
    let other = value.clone();
    let child = std::thread::spawn(move || {
        entered_tx.send(()).unwrap();
        let guard = other.lock_recover_on_control();
        assert_eq!(guard.get(), 17);
        guard.set(29);
    });
    entered_rx.recv().unwrap();
    held.set(17);
    assert!(matches!(
        value.try_lock(),
        Err(CustodyTryLockError::Contended)
    ));
    drop(held);
    child.join().unwrap(); // eventual progress; no fairness or timing bound
    assert_eq!(value.try_lock().unwrap().get(), 29);
}
