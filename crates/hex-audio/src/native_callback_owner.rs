//! Private, control-anchored, once-only native closure ownership transfer.
//!
//! Only Guard::drop initializes a fresh cell. Control never initializes/waits,
//! and cannot reclaim while the guard retains its Arc. The initializer only
//! moves an existing Mutex carrier. OnceLock's uncontended path is not a
//! formal hard-real-time bound; panic hooks/abort are outside this guarantee.
//! Production exposes only a coupled data/error receiver. Both guard Arcs must
//! be gone before either boxed owner can be reclaimed; receipt alone is not
//! finalrelease. CPAL closures contain only their guard and Copy configuration.
use crate::prepared::{Callback, Status};
use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

pub(crate) struct NativeCallbackOwner {
    callback: Callback,
    first_panic: Option<Box<dyn Any + Send>>,
    fault_latched: bool,
    #[cfg(test)]
    disposal_probe: Option<Box<dyn FnMut() + Send>>,
}
impl NativeCallbackOwner {
    pub(crate) fn new(callback: Callback) -> Self {
        Self {
            callback,
            first_panic: None,
            fault_latched: false,
            #[cfg(test)]
            disposal_probe: None,
        }
    }
    /// Control retirement only: detach before callback destruction so an
    /// independent payload destructor cannot run during its unwinding cleanup.
    /// Moves the same allocation; no clone/allocation or callback operation.
    pub(crate) fn take_retained_panic_on_control(&mut self) -> Option<Box<dyn Any + Send>> {
        self.first_panic.take()
    }
    #[cfg(test)]
    pub(crate) fn retained_panic_identity(&self) -> Option<usize> {
        self.first_panic
            .as_ref()
            .map(|p| p.as_ref() as *const dyn Any as *const () as usize)
    }
    #[cfg(test)]
    pub(crate) fn observe_disposal(&mut self, probe: impl FnMut() + Send + 'static) {
        self.disposal_probe = Some(Box::new(probe));
    }
    pub(crate) fn held_claimed_on_control(
        &self,
    ) -> Option<(
        &hex_engine::storage_report::ClaimedPlaybackOwner,
        &crate::publication_claims::ClaimAssociation,
        bool,
    )> {
        self.callback.held_claimed_on_control()
    }
    pub(crate) fn take_held_claimed_on_control(
        &mut self,
    ) -> Option<crate::prepared::ClaimedGraphReturn> {
        self.callback.take_held_claimed_on_control()
    }
    pub(crate) fn expose_terminal_tracked_on_control(&mut self) -> bool {
        self.callback.expose_terminal_tracked_on_control()
    }
    pub(crate) fn peek_tracked_on_control(
        &self,
    ) -> Option<(
        &crate::prepared::Publication,
        crate::prepared::ControlTrackedLocation,
    )> {
        self.callback.peek_tracked_on_control()
    }
    pub(crate) fn take_tracked_on_control(
        &mut self,
        location: crate::prepared::ControlTrackedLocation,
    ) -> Option<crate::prepared::GraphReturn> {
        self.callback.take_tracked_on_control(location)
    }
    pub(crate) fn callback_fault_latched(&self) -> bool {
        self.fault_latched
    }
    pub(crate) fn status(&self) -> &Status {
        self.callback.native_status()
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

/// Separate CPAL error closure owner: no Status Arc escapes the two guards.
pub(crate) struct ErrorCallbackOwner {
    status: Arc<Status>,
    #[cfg(test)]
    disposal_probe: Option<Box<dyn FnMut() + Send>>,
}
impl ErrorCallbackOwner {
    #[cfg(test)]
    pub(crate) fn observe_disposal(&mut self, probe: impl FnMut() + Send + 'static) {
        self.disposal_probe = Some(Box::new(probe));
    }
    pub(crate) fn report(&self) {
        self.status
            .device_fault
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
// Test instrumentation runs on the actual boxed owner, not a substitute
// retirement model. Production has no hook or added destructor behavior.
#[cfg(test)]
impl Drop for NativeCallbackOwner {
    fn drop(&mut self) {
        if let Some(probe) = self.disposal_probe.as_mut() {
            probe();
        }
    }
}
#[cfg(test)]
impl Drop for ErrorCallbackOwner {
    fn drop(&mut self) {
        if let Some(probe) = self.disposal_probe.as_mut() {
            probe();
        }
    }
}
/// Both backend closures must finish finalrelease before either owner is taken.
/// No raw receiver exposure, clone or independently consumable receipt.
pub(crate) struct NativeTerminals {
    data: Terminal<NativeCallbackOwner>,
    errors: Terminal<ErrorCallbackOwner>,
}
pub(crate) fn reserve_native(
    callback: Callback,
    status: Arc<Status>,
) -> (
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeTerminals,
) {
    let (data_guard, data) = reserve(Box::new(NativeCallbackOwner::new(callback)));
    let (error_guard, errors) = reserve(Box::new(ErrorCallbackOwner {
        status,
        #[cfg(test)]
        disposal_probe: None,
    }));
    (data_guard, error_guard, NativeTerminals { data, errors })
}
impl NativeTerminals {
    pub(crate) fn try_take(
        &mut self,
    ) -> Option<(Box<NativeCallbackOwner>, Box<ErrorCallbackOwner>)> {
        // Readiness cannot regress: guards are nonclone, with no other Arc or
        // Weak minting. Both receivers stay anchored through this preflight.
        if !self.data.can_reclaim() || !self.errors.can_reclaim() {
            return None;
        }
        Some((self.data.try_take()?, self.errors.try_take()?))
    }
}

// No Clone, raw cell access, reset, wait, set, or second initializer operation.
// Mutex supplies Sync for a Send-but-not-Sync owner; callback never locks it.
pub(crate) struct Guard<T> {
    owner: Option<Mutex<Box<T>>>,
    terminal: Arc<OnceLock<Mutex<Box<T>>>>,
}
struct Terminal<T> {
    cell: Option<Arc<OnceLock<Mutex<Box<T>>>>>,
}
fn reserve<T>(owner: Box<T>) -> (Guard<T>, Terminal<T>) {
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
    #[cfg(test)]
    fn received(&self) -> bool {
        self.cell.as_ref().is_some_and(|cell| cell.get().is_some())
    }
    fn can_reclaim(&mut self) -> bool {
        self.cell
            .as_mut()
            .is_some_and(|cell| cell.get().is_some() && Arc::get_mut(cell).is_some())
    }
    /// None means no receipt OR the sealed guard has not released its Arc yet.
    /// Caller separately proves native quiescence before calling this method.
    fn try_take(&mut self) -> Option<Box<T>> {
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
/// the prepared channel (including shared Status storage), allocator/CPAL
/// overhead and exceptional panic payload. Both closure carriers are charged.
pub(crate) fn additional_storage_bytes() -> Option<usize> {
    fn carrier<T>() -> Option<usize> {
        let align = std::mem::align_of::<OnceLock<Mutex<Box<T>>>>();
        let header = (2 * std::mem::size_of::<usize>()).next_multiple_of(align);
        std::mem::size_of::<Guard<T>>()
            .checked_add(std::mem::size_of::<Terminal<T>>())?
            .checked_add(std::mem::size_of::<OnceLock<Mutex<Box<T>>>>())?
            .checked_add(header)
    }
    std::mem::size_of::<NativeCallbackOwner>()
        .checked_sub(std::mem::size_of::<Callback>())?
        .checked_add(carrier::<NativeCallbackOwner>()?)?
        .checked_add(std::mem::size_of::<ErrorCallbackOwner>())?
        .checked_add(carrier::<ErrorCallbackOwner>()?)?
        .checked_add(crate::lifecycle::retirement_execution_storage_bytes()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };
    #[test]
    fn construction_failure_and_either_closure_release_order_keep_pair_atomic() {
        for error_first in [false, true] {
            let (control, callback) = crate::prepared::channel(48_000).unwrap();
            let status_address = Arc::as_ptr(&control.status) as usize;
            let (data, errors, mut terminals) =
                reserve_native(callback, Arc::clone(&control.status));
            drop(control);
            crate::prepared::take_status_disposal();
            // Build refusal drops closures on the caller, or a backend may
            // destroy the error closure before the data closure. Neither order
            // permits partial consumption or Status destruction.
            if error_first {
                drop(errors);
                assert!(terminals.try_take().is_none());
                drop(data);
            } else {
                drop(data);
                assert!(terminals.try_take().is_none());
                drop(errors);
            }
            assert_eq!(crate::prepared::take_status_disposal(), None);
            drop(terminals.try_take().unwrap());
            assert_eq!(
                crate::prepared::take_status_disposal(),
                Some((status_address, thread::current().id()))
            );
            assert!(terminals.try_take().is_none());
        }
    }
    #[test]
    fn former_split_capture_topology_frees_actual_status_on_backend_negative_control() {
        let (control, callback) = crate::prepared::channel(48_000).unwrap();
        let errors = Arc::clone(&control.status);
        let address = Arc::as_ptr(&errors) as usize;
        let (guard, mut terminal) = reserve(Box::new(NativeCallbackOwner::new(callback)));
        drop(control);
        thread::spawn(move || drop(guard)).join().unwrap();
        drop(terminal.try_take().unwrap());
        assert_eq!(Arc::strong_count(&errors), 1);
        let (measurement, disposal, backend) = thread::spawn(move || {
            let backend = thread::current().id();
            crate::prepared::take_status_disposal();
            let measurement = allocation_counter::measure(|| drop(errors));
            (
                measurement,
                crate::prepared::take_status_disposal(),
                backend,
            )
        })
        .join()
        .unwrap();
        assert_eq!(disposal, Some((address, backend)));
        assert!(measurement.bytes_current < 0);
        eprintln!(
            "former no-backend-free invariant FAILED as expected: Status address={address}, destructor={backend:?}, freed={} bytes",
            -measurement.bytes_current
        );
    }
    #[test]
    fn delayed_error_closure_prevents_both_owner_consumption_and_backend_status_free() {
        let (control, callback) = crate::prepared::channel(48_000).unwrap();
        let status_address = Arc::as_ptr(&control.status) as usize;
        let (mut data, mut errors, mut terminals) =
            reserve_native(callback, Arc::clone(&control.status));
        let data_address = data.owner_mut() as *const _ as usize;
        let error_address = errors.owner_mut() as *const _ as usize;
        drop(control);
        let measurement = thread::spawn(move || {
            let _id = thread::current().id();
            allocation_counter::measure(|| drop(data))
        })
        .join()
        .unwrap();
        assert_eq!(
            (
                measurement.count_total,
                measurement.count_current,
                measurement.bytes_current
            ),
            (0, 0, 0)
        );
        assert!(terminals.try_take().is_none());
        assert!(terminals.data.received());
        errors.owner_mut().report();
        assert!(
            errors
                .owner_mut()
                .status
                .device_fault
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        // Receipt alone does not authorize consumption while the error guard
        // still owns its Arc, including the guard's field-destruction interval.
        errors.publish_for_drop();
        assert!(terminals.errors.received());
        assert!(terminals.try_take().is_none());
        let (measurement, disposal) = thread::spawn(move || {
            let _id = thread::current().id();
            crate::prepared::take_status_disposal();
            let measurement = allocation_counter::measure(|| drop(errors));
            (measurement, crate::prepared::take_status_disposal())
        })
        .join()
        .unwrap();
        assert_eq!(
            (
                measurement.count_total,
                measurement.count_current,
                measurement.bytes_current
            ),
            (0, 0, 0)
        );
        assert_eq!(disposal, None);
        let (data, errors) = terminals.try_take().unwrap();
        assert_eq!(data.as_ref() as *const _ as usize, data_address);
        assert_eq!(errors.as_ref() as *const _ as usize, error_address);
        assert_eq!(Arc::strong_count(&errors.status), 2);
        crate::prepared::take_status_disposal();
        let measurement = allocation_counter::measure(|| {
            drop(data);
            drop(errors);
        });
        assert!(measurement.bytes_current < 0);
        assert_eq!(
            crate::prepared::take_status_disposal(),
            Some((status_address, thread::current().id()))
        );
        assert!(terminals.try_take().is_none());
    }
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
