//! Once-only, coupled native closure custody, adapted from original177
//! 8d444c06. Backend guard destruction moves a carrier; only lifecycle control
//! destroys its contents. Receiving a carrier is not final-active quiescence.
use crate::prepared::{Callback, Status};
mod resources;
pub(crate) use resources::NativeCandidateBoundary;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Sealed application-callback nonrenderability authority. It is not an OS
/// stop/dispose-success observation and never authorizes publication.
pub(crate) struct FinalReleaseAuthority {
    root: u64,
}
impl FinalReleaseAuthority {
    pub(crate) fn matches(&self, root: u64) -> bool {
        self.root == root
    }
}
pub(crate) struct NativeCallbackOwner {
    pub(crate) callback: Callback,
    status: Arc<Status>,
    #[cfg(test)]
    probe: Option<DropProbe>,
}
impl NativeCallbackOwner {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_drop_probe(&mut self, probe: impl FnMut() + Send + 'static) {
        self.probe = Some(DropProbe(Box::new(probe)));
    }
    pub(crate) fn run(&mut self, call: impl FnOnce(&mut Callback, &Status)) {
        // Preserve ordinary Result-based fault handling. No callback unwind
        // recovery is introduced at the native FFI boundary.
        call(&mut self.callback, &self.status);
    }
}
pub(crate) struct ErrorCallbackOwner {
    status: Arc<Status>,
    #[cfg(test)]
    probe: Option<DropProbe>,
}
#[cfg(test)]
struct DropProbe(Box<dyn FnMut() + Send>);
#[cfg(test)]
impl Drop for DropProbe {
    fn drop(&mut self) {
        (self.0)();
    }
}
impl ErrorCallbackOwner {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_drop_probe(&mut self, probe: impl FnMut() + Send + 'static) {
        self.probe = Some(DropProbe(Box::new(probe)));
    }
    pub(crate) fn report(&self) {
        self.status
            .device_fault
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
pub(crate) struct Guard<T> {
    owner: Option<Mutex<Box<T>>>,
    cell: Arc<OnceLock<Mutex<Box<T>>>>,
}
struct Terminal<T> {
    cell: Option<Arc<OnceLock<Mutex<Box<T>>>>>,
}
fn reserve<T>(value: T) -> (Guard<T>, Terminal<T>) {
    let cell = Arc::new(OnceLock::new());
    (
        Guard {
            owner: Some(Mutex::new(Box::new(value))),
            cell: Arc::clone(&cell),
        },
        Terminal { cell: Some(cell) },
    )
}
impl<T> Guard<T> {
    pub(crate) fn owner_mut(&mut self) -> Option<&mut T> {
        self.owner.as_mut().map(|owner| {
            owner
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .as_mut()
        })
    }
}
impl<T> Drop for Guard<T> {
    fn drop(&mut self) {
        self.publish_for_drop();
    }
}
impl<T> Guard<T> {
    fn publish_for_drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            // Private fresh cell, sole initializer; no provider or destructor.
            self.cell.get_or_init(|| owner);
        }
        // The anchored terminal still owns a strong reference. This Drop never
        // frees the root or its Box on a backend thread.
    }
}
impl<T> Terminal<T> {
    fn ready(&mut self) -> bool {
        self.cell
            .as_mut()
            .is_some_and(|cell| cell.get().is_some() && Arc::get_mut(cell).is_some())
    }
    fn take(&mut self) -> Option<Box<T>> {
        if !self.ready() {
            return None;
        }
        let cell = self.cell.take()?;
        match Arc::try_unwrap(cell) {
            Ok(cell) => cell
                .into_inner()
                .map(|owner| owner.into_inner().unwrap_or_else(PoisonError::into_inner)),
            Err(cell) => {
                self.cell = Some(cell);
                None
            }
        }
    }
}
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
    let root = callback.candidate_root_for_native_boundary();
    let (mut data_guard, data) = reserve(NativeCallbackOwner {
        callback,
        status: Arc::clone(&status),
        #[cfg(test)]
        probe: None,
    });
    let (mut error_guard, errors) = reserve(ErrorCallbackOwner {
        status,
        #[cfg(test)]
        probe: None,
    });
    if let Some(boundary) =
        NativeCandidateBoundary::from_reserved(&root, &mut data_guard, &mut error_guard)
    {
        root.record_native_boundary(boundary);
    }
    (data_guard, error_guard, NativeTerminals { data, errors })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn both_guards_must_fully_release_after_publication_and_exact_boxes_drop_only_on_control() {
        let _serial = crate::prepared::candidate_tests::serial();
        for errors_first in [false, true] {
            let (control, mut data, mut errors, mut retirement) =
                crate::prepared::candidate_tests::anchored();
            let data_address = data.owner_mut().unwrap() as *mut NativeCallbackOwner as usize;
            let error_address = errors.owner_mut().unwrap() as *mut ErrorCallbackOwner as usize;
            let records = Arc::new(Mutex::new(Vec::new()));
            let data_records = records.clone();
            let error_records = records.clone();
            data.owner_mut().unwrap().probe = Some(DropProbe(Box::new(move || {
                data_records
                    .lock()
                    .unwrap()
                    .push((data_address, std::thread::current().id()))
            })));
            errors.owner_mut().unwrap().probe = Some(DropProbe(Box::new(move || {
                error_records
                    .lock()
                    .unwrap()
                    .push((error_address, std::thread::current().id()))
            })));
            data.publish_for_drop();
            errors.publish_for_drop();
            assert!(!retirement.advance().unwrap()); // published is not released
            if errors_first {
                std::thread::spawn(move || drop(errors)).join().unwrap();
                assert!(!retirement.advance().unwrap());
                std::thread::spawn(move || drop(data)).join().unwrap();
            } else {
                std::thread::spawn(move || drop(data)).join().unwrap();
                assert!(!retirement.advance().unwrap());
                std::thread::spawn(move || drop(errors)).join().unwrap();
            }
            assert!(records.lock().unwrap().is_empty());
            assert!(retirement.advance().unwrap());
            assert_eq!(
                *records.lock().unwrap(),
                [
                    (data_address, std::thread::current().id()),
                    (error_address, std::thread::current().id())
                ]
            );
            drop(control);
        }
    }
    #[test]
    fn actual_data_destructor_panic_keeps_error_owner_and_first_control_payload_without_retry() {
        let _serial = crate::prepared::candidate_tests::serial();
        use std::{
            any::Any,
            panic::{AssertUnwindSafe, catch_unwind},
        };
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let (control, mut data, errors, mut retirement) =
            crate::prepared::candidate_tests::anchored();
        let payload_drops = Arc::new(AtomicUsize::new(0));
        let payload = payload_drops.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        data.owner_mut().unwrap().probe = Some(DropProbe(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            std::panic::panic_any(Payload(payload.clone()));
        })));
        drop(data);
        drop(errors);
        // Same outer control-side catch as lifecycle retirement; the persistent
        // retirement fields outlive the actual nested destructor unwind.
        let first = catch_unwind(AssertUnwindSafe(|| retirement.advance()))
            .err()
            .unwrap();
        let original = first.as_ref() as *const dyn Any as *const ();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(payload_drops.load(Ordering::SeqCst), 0);
        assert!(retirement.errors.is_some());
        assert!(retirement.advance().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.as_ref() as *const dyn Any as *const (), original);
        drop(retirement);
        drop(control); // explicit test control teardown
        assert_eq!(payload_drops.load(Ordering::SeqCst), 0);
        drop(first);
        assert_eq!(payload_drops.load(Ordering::SeqCst), 1);
    }
}
impl NativeTerminals {
    fn receive_into(
        &mut self,
        data: &mut Option<Box<NativeCallbackOwner>>,
        errors: &mut Option<Box<ErrorCallbackOwner>>,
    ) -> Result<bool, ()> {
        if data.is_some() || errors.is_some() {
            return Err(());
        }
        // No clone/Weak/reset/raw cell API exists; readiness cannot regress while
        // both actual terminal anchors remain borrowed here. Still secure each
        // extracted owner in persistent storage before the next checked step.
        if !self.data.ready() || !self.errors.ready() {
            return Ok(false);
        }
        *data = self.data.take();
        if data.is_none() {
            return Err(());
        }
        *errors = self.errors.take();
        if errors.is_none() {
            return Err(());
        }
        Ok(true)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Disposal {
    Unreceived,
    Received,
    Pending,
    Complete,
}
/// Persistent lifecycle fields, always outside the execution unwind boundary.
pub(crate) struct NativeRetirement {
    terminals: Option<NativeTerminals>,
    data: Option<Box<NativeCallbackOwner>>,
    errors: Option<Box<ErrorCallbackOwner>>,
    states: [Disposal; 2],
    completed_stream_handle: bool,
    domain_pages: [Option<crate::candidate_custody::DomainEnvelope>; 2],
    final_active: Option<crate::candidate_custody::FinalActiveReturn>,
    root: crate::candidate_custody::CandidateRoot,
}
impl NativeRetirement {
    pub(crate) fn new(
        terminals: NativeTerminals,
        root: crate::candidate_custody::CandidateRoot,
    ) -> Self {
        root.anchor_native_boundary(&terminals);
        Self {
            terminals: Some(terminals),
            data: None,
            errors: None,
            states: [Disposal::Unreceived; 2],
            completed_stream_handle: false,
            domain_pages: std::array::from_fn(|_| None),
            final_active: None,
            root,
        }
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_extracted_identity(&self) -> Option<hex_engine::AudioCandidateIdentity> {
        self.final_active.as_ref().map(|value| value.identity())
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_terminal_domain_count(&self) -> usize {
        self.domain_pages
            .iter()
            .filter(|pages| pages.is_some())
            .count()
    }
    pub(crate) fn root(&self) -> crate::candidate_custody::CandidateRoot {
        self.root.clone()
    }
    /// Supply the exact completed-handle frontier from the owning lifecycle
    /// bundle. This plus both final guard releases permits application-only
    /// nonrenderability; native stop/dispose-success is never inferred.
    pub(crate) fn advance_after_stream_disposal(
        &mut self,
        completed: &crate::lifecycle::CompletedStreamHandle,
    ) -> Result<bool, ()> {
        if !completed.matches(self.root.root_id()) {
            return Err(());
        }
        self.completed_stream_handle = true;
        self.advance()
    }
    pub(crate) fn advance(&mut self) -> Result<bool, ()> {
        if self.states.contains(&Disposal::Pending) {
            return Err(());
        }
        if self.states[0] == Disposal::Unreceived {
            if self.data.is_some() || self.errors.is_some() {
                return Err(());
            }
            let Some(terminals) = self.terminals.as_mut() else {
                return Err(());
            };
            if !terminals.receive_into(&mut self.data, &mut self.errors)? {
                return Ok(false);
            }
            self.states = [Disposal::Received; 2];
        }
        if self.states[0] == Disposal::Received {
            let Some(data) = self.data.as_mut() else {
                return Err(());
            };
            if !data.callback.belongs_to_candidate_root(&self.root) {
                return Err(());
            }
            if !data.callback.drain_candidate_custody_on_control() {
                return Ok(false);
            }
            if data.callback.has_domain_pages() {
                if !self.completed_stream_handle {
                    return Ok(false);
                }
                if !data
                    .callback
                    .extract_domain_pages_on_control(&mut self.domain_pages)
                {
                    return Err(());
                }
            }
            if data.callback.has_active_candidate() {
                // Exact carrier extraction plus completed stream-handle custody
                // closes every valid application callback path under CPAL's safe
                // dependency contract. It does not attest native Dispose success.
                if !self.completed_stream_handle {
                    return Ok(false);
                }
                if !data.callback.extract_final_candidate_on_control(
                    &FinalReleaseAuthority {
                        root: self.root.root_id(),
                    },
                    &mut self.final_active,
                ) {
                    return Err(());
                }
            }
            self.states[0] = Disposal::Pending;
            drop(self.data.take());
            self.states[0] = Disposal::Complete;
        }
        if self.states[1] == Disposal::Received {
            if self.errors.is_none() {
                return Err(());
            }
            self.states[1] = Disposal::Pending;
            drop(self.errors.take());
            self.states[1] = Disposal::Complete;
        }
        if self.states != [Disposal::Complete; 2] {
            return Err(());
        }
        for index in 0..self.domain_pages.len() {
            if let Some(pages) = self.domain_pages[index].take()
                && let Err(pages) = self.root.ingest_pages(pages)
            {
                self.domain_pages[index] = Some(pages);
                return Err(());
            }
        }
        // The constructor owner stays here through both actual carrier
        // destructors. A failure before this point cannot publish READY/reuse.
        if let Some(returned) = self.final_active.take() {
            if let Err(returned) = self.root.ingest_final(returned) {
                self.final_active = Some(returned);
                return Err(());
            }
        }
        self.terminals = None;
        Ok(self.root.finish_on_control())
    }
}
