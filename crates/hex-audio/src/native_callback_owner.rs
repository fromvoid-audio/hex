//! Private, control-anchored, once-only native closure ownership transfer.
//!
//! Only Guard::drop initializes a fresh cell. Control never initializes/waits,
//! and cannot reclaim while the guard retains its Arc. The initializer only
//! moves an existing Mutex carrier. OnceLock's uncontended path is not a
//! formal hard-real-time bound; panic hooks/abort are outside this guarantee.
use crate::prepared::Callback;
use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

pub(crate) struct NativeCallbackOwner {
    callback: Callback,
    first_panic: Option<Box<dyn Any + Send>>,
    fault_latched: bool,
}
impl NativeCallbackOwner {
    pub(crate) fn new(callback: Callback) -> Self {
        Self {
            callback,
            first_panic: None,
            fault_latched: false,
        }
    }
    /// Quarantine rather than resume possibly staged/partially committed state.
    pub(crate) fn run<R>(&mut self, operation: impl FnOnce(&mut Callback) -> R) -> Result<R, ()> {
        if self.fault_latched {
            return Err(());
        }
        match catch_unwind(AssertUnwindSafe(|| operation(&mut self.callback))) {
            Ok(result) => Ok(result),
            Err(payload) => {
                self.fault_latched = true;
                self.first_panic = Some(payload);
                self.callback.latch_native_unwind();
                Err(())
            }
        }
    }
}

// No Clone, raw cell access, reset, wait, set, or second initializer operation.
// Mutex supplies Sync for a Send-but-not-Sync owner; callback never locks it.
pub(crate) struct Guard<T> {
    owner: Option<Mutex<Box<T>>>,
    terminal: Arc<OnceLock<Mutex<Box<T>>>>,
}
pub(crate) struct Terminal<T> {
    cell: Option<Arc<OnceLock<Mutex<Box<T>>>>>,
}
pub(crate) fn reserve<T>(owner: Box<T>) -> (Guard<T>, Terminal<T>) {
    let cell = Arc::new(OnceLock::new());
    (
        Guard {
            owner: Some(Mutex::new(owner)),
            terminal: Arc::clone(&cell),
        },
        Terminal { cell: Some(cell) },
    )
}
impl<T> Guard<T> {
    pub(crate) fn owner_mut(&mut self) -> &mut T {
        self.owner
            .as_mut()
            .expect("live sealed guard retains its owner")
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
    }
}
impl<T> Guard<T> {
    fn publish_for_drop(&mut self) {
        if let Some(carrier) = self.owner.take() {
            // Fresh cell, sole initializer, no reset; initializer is a move only.
            self.terminal.get_or_init(|| carrier);
        }
    }
}
impl<T> Drop for Guard<T> {
    fn drop(&mut self) {
        self.publish_for_drop();
        // Control custody still retains the other Arc: no last-root free here.
    }
}
impl<T> Terminal<T> {
    pub(crate) fn received(&self) -> bool {
        self.cell.as_ref().is_some_and(|cell| cell.get().is_some())
    }
    /// None means no receipt OR the sealed guard has not released its Arc yet.
    /// Caller separately proves native quiescence before calling this method.
    pub(crate) fn try_take(&mut self) -> Option<Box<T>> {
        let cell = self.cell.as_mut()?;
        if cell.get().is_none() || Arc::get_mut(cell).is_none() {
            return None;
        }
        // There are no other Arc/Weak creators. Successful exclusive get_mut
        // proves guard destruction ended; consume the cell, never reset/reuse.
        let cell = self.cell.take()?;
        match Arc::try_unwrap(cell) {
            Ok(cell) => cell
                .into_inner()
                .map(|carrier| carrier.into_inner().unwrap_or_else(PoisonError::into_inner)),
            Err(cell) => {
                self.cell = Some(cell);
                None
            }
        }
    }
}

/// Fixed carrier/root data reservations, excluding Callback already charged by
/// the prepared channel, allocator/CPAL overhead and exceptional panic payload.
pub(crate) fn additional_storage_bytes() -> Option<usize> {
    type Cell = OnceLock<Mutex<Box<NativeCallbackOwner>>>;
    let align = std::mem::align_of::<Cell>();
    let header = (2 * std::mem::size_of::<usize>()).next_multiple_of(align);
    std::mem::size_of::<NativeCallbackOwner>()
        .checked_sub(std::mem::size_of::<Callback>())?
        .checked_add(std::mem::size_of::<Guard<NativeCallbackOwner>>())?
        .checked_add(std::mem::size_of::<Terminal<NativeCallbackOwner>>())?
        .checked_add(std::mem::size_of::<Cell>())?
        .checked_add(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };
    struct Probe {
        id: usize,
        events: Arc<Mutex<Vec<(usize, thread::ThreadId)>>>,
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            self.events
                .lock()
                .unwrap()
                .push((self.id, thread::current().id()));
        }
    }
    #[test]
    fn background_guard_disposal_returns_exact_box_without_destructor_or_allocation() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let owner = Box::new(Probe {
            id: 17,
            events: Arc::clone(&events),
        });
        eprintln!(
            "native carrier additional fixed data bytes={}",
            additional_storage_bytes().unwrap()
        );
        let address = owner.as_ref() as *const Probe as usize;
        let (guard, mut terminal) = reserve(owner);
        assert!(!terminal.received());
        assert!(terminal.try_take().is_none());
        let measurement = thread::spawn(move || allocation_counter::measure(|| drop(guard)))
            .join()
            .unwrap();
        assert_eq!(
            (
                measurement.count_total,
                measurement.count_current,
                measurement.bytes_total,
                measurement.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(events.lock().unwrap().is_empty());
        assert!(terminal.received());
        let owner = terminal.try_take().unwrap();
        assert_eq!(owner.as_ref() as *const Probe as usize, address);
        drop(owner);
        assert_eq!(*events.lock().unwrap(), [(17, thread::current().id())]);
        assert!(terminal.try_take().is_none());
    }
    #[test]
    fn first_payload_is_retained_and_latch_excludes_second_provider_call() {
        let (control, callback) = crate::prepared::channel(48_000).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let (mut guard, mut terminal) = reserve(Box::new(NativeCallbackOwner::new(callback)));
        let calls = AtomicUsize::new(0);
        assert!(
            guard
                .owner_mut()
                .run(|_| {
                    calls.fetch_add(1, Ordering::Relaxed);
                    std::panic::panic_any(Probe {
                        id: 91,
                        events: Arc::clone(&events),
                    });
                })
                .is_err()
        );
        assert!(
            guard
                .owner_mut()
                .run(|_| {
                    calls.fetch_add(1, Ordering::Relaxed);
                })
                .is_err()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(control.status.render_fault.load(Ordering::Acquire));
        assert!(!terminal.received());
        assert!(events.lock().unwrap().is_empty());
        drop(control); // Ordinary handle is not terminal custody.
        thread::spawn(move || drop(guard)).join().unwrap();
        assert!(events.lock().unwrap().is_empty());
        drop(terminal.try_take().unwrap());
        assert_eq!(*events.lock().unwrap(), [(91, thread::current().id())]);
    }
    #[test]
    fn published_receipt_cannot_be_taken_before_guard_final_release() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (mut guard, mut terminal) = reserve(Box::new(Probe {
            id: 33,
            events: Arc::clone(&events),
        }));
        // Model the interval within Drop after initialization but before its
        // final Arc field release; not an operation exposed to driver/control.
        guard.publish_for_drop();
        assert!(terminal.received());
        assert!(terminal.try_take().is_none());
        assert!(events.lock().unwrap().is_empty());
        drop(guard);
        drop(terminal.try_take().unwrap());
        assert_eq!(*events.lock().unwrap(), [(33, thread::current().id())]);
    }
    #[test]
    fn saturated_normal_return_channel_cannot_fill_terminal_cell() {
        let (mut normal, _normal_receiver) = rtrb::RingBuffer::new(2);
        normal.push(1).unwrap();
        normal.push(2).unwrap();
        assert!(normal.is_full());
        let (guard, mut terminal) = reserve(Box::new(55_u64));
        thread::spawn(move || drop(guard)).join().unwrap();
        assert_eq!(*terminal.try_take().unwrap(), 55);
        assert!(normal.is_full());
    }
}
