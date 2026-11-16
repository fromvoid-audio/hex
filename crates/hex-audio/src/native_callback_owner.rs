//! Once-only coupled custody for native callback and error guards.
//!
//! Backend guard destruction moves a carrier; only lifecycle control destroys its contents.
//! Final-active extraction requires the exact root-bound release authority, normal completion of
//! the original stream-handle destructor and release of both terminal guard anchors. Silence,
//! Pause, shutdown acknowledgment or merely receiving a carrier establishes none of those
//! conditions. Destruction failure retains Pending and exclusive output custody without retry.
//! This proves nonrenderability through the supported application callback paths, not
//! independently observed AudioUnit stop/uninitialize/dispose success.
use crate::prepared::{Callback, Status};
#[cfg(all(test, feature = "allocation-probe"))]
mod endpoint_tests;
mod resources;
pub(crate) use resources::{
    ChannelStorage, NativeCandidateBoundary, worker_boundary_storage_quote,
};
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
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn borrowed_status(&self) -> &Status {
        &self.status
    }
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
pub(crate) struct DropProbe(pub(crate) Box<dyn FnMut() + Send>);
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
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
enum GuardState<T> {
    Live {
        owner: Box<T>,
        cell: Arc<OnceLock<Mutex<Box<T>>>>,
    },
    Published(Arc<OnceLock<Mutex<Box<T>>>>),
    Closed,
}
pub(crate) struct Guard<T> {
    state: GuardState<T>,
}
pub(crate) struct Terminal<T> {
    cell: Option<Arc<OnceLock<Mutex<Box<T>>>>>,
}
pub(crate) fn reserve<T>(value: T) -> (Guard<T>, Terminal<T>) {
    let cell = Arc::new(OnceLock::new());
    (
        Guard {
            state: GuardState::Live {
                owner: Box::new(value),
                cell: Arc::clone(&cell),
            },
        },
        Terminal { cell: Some(cell) },
    )
}
impl<T> Guard<T> {
    pub(crate) fn owner_ref(&self) -> Option<&T> {
        match &self.state {
            GuardState::Live { owner, .. } => Some(owner.as_ref()),
            GuardState::Published(_) | GuardState::Closed => None,
        }
    }
    pub(crate) fn owner_mut(&mut self) -> Option<&mut T> {
        match &mut self.state {
            GuardState::Live { owner, .. } => Some(owner.as_mut()),
            GuardState::Published(_) | GuardState::Closed => None,
        }
    }
    pub(crate) fn shared_cell(&self) -> Option<&Arc<OnceLock<Mutex<Box<T>>>>> {
        match &self.state {
            GuardState::Live { cell, .. } | GuardState::Published(cell) => Some(cell),
            GuardState::Closed => None,
        }
    }
    /// Nonblocking ownership transfer, leaving no live endpoint or anchor here.
    /// Only the constructor-paired terminal may later destroy the whole bundle.
    pub(crate) fn close(&mut self) {
        self.publish_for_drop();
        self.state = GuardState::Closed;
    }
    fn publish_for_drop(&mut self) {
        self.state = match std::mem::replace(&mut self.state, GuardState::Closed) {
            GuardState::Live { owner, cell } => {
                // Private fresh cell and sole initializer. Mutex construction
                // moves the existing Box; it neither allocates nor drops T.
                cell.get_or_init(|| Mutex::new(owner));
                GuardState::Published(cell)
            }
            other => other,
        };
    }
}
impl<T> Drop for Guard<T> {
    fn drop(&mut self) {
        self.close();
    }
}
impl<T> Terminal<T> {
    pub(crate) fn ready(&mut self) -> bool {
        self.cell
            .as_mut()
            .is_some_and(|cell| cell.get().is_some() && Arc::get_mut(cell).is_some())
    }
    pub(crate) fn take(&mut self) -> Option<Box<T>> {
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
    endpoints: Option<crate::prepared::EndpointTerminals>,
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
    let (mut data, mut errors, terminals) = reserve_carriers(callback, status, None);
    if let Some(boundary) = NativeCandidateBoundary::from_reserved(&root, &mut data, &mut errors) {
        root.record_native_boundary(boundary);
    }
    (data, errors, terminals)
}
pub(crate) fn reserve_native_worker(
    control: crate::prepared::Control,
    callback: Callback,
    reader: Option<crate::spectrum_worker::SpectrumReader>,
) -> (
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeTerminals,
    crate::prepared::Client,
    crate::prepared::WorkerControl,
) {
    let root = control.native_candidate_root();
    let status = Arc::clone(&control.status);
    let (client, publication, endpoints) = control.reserve_worker_endpoints(reader);
    let (mut data, mut errors, terminals) = reserve_carriers(callback, status, Some(endpoints));
    if let Some(boundary) = NativeCandidateBoundary::from_reserved(&root, &mut data, &mut errors)
        .and_then(|boundary| boundary.with_worker_endpoints(&client, &publication))
    {
        root.record_native_boundary(boundary);
    }
    (data, errors, terminals, client, publication)
}
fn reserve_carriers(
    callback: Callback,
    status: Arc<Status>,
    endpoints: Option<crate::prepared::EndpointTerminals>,
) -> (
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeTerminals,
) {
    let (data, data_terminal) = reserve(NativeCallbackOwner {
        callback,
        status: Arc::clone(&status),
        #[cfg(test)]
        probe: None,
    });
    let (errors, error_terminal) = reserve(ErrorCallbackOwner {
        status,
        #[cfg(test)]
        probe: None,
    });
    (
        data,
        errors,
        NativeTerminals {
            data: data_terminal,
            errors: error_terminal,
            endpoints,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn explicit_endpoint_close_is_inert_and_only_terminal_control_destroys_actual_box() {
        struct Endpoint {
            drops: Arc<Mutex<Vec<std::thread::ThreadId>>>,
        }
        impl Drop for Endpoint {
            fn drop(&mut self) {
                self.drops.lock().unwrap().push(std::thread::current().id());
            }
        }
        let drops = Arc::new(Mutex::new(Vec::new()));
        let (mut guard, mut terminal) = reserve(Endpoint {
            drops: Arc::clone(&drops),
        });
        let original = guard.owner_ref().unwrap() as *const Endpoint as usize;
        let measured = allocation_counter::measure(|| {
            guard.close();
            guard.close();
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
        assert!(guard.owner_ref().is_none() && guard.owner_mut().is_none());
        assert!(drops.lock().unwrap().is_empty());
        // The inert UI token is still alive while the actual endpoint Box is
        // extracted and destroyed elsewhere; it cannot strand a replacement.
        assert!(terminal.ready());
        let owner = terminal.take().unwrap();
        assert_eq!(owner.as_ref() as *const Endpoint as usize, original);
        let control_thread = std::thread::spawn(move || {
            let id = std::thread::current().id();
            drop(owner);
            id
        })
        .join()
        .unwrap();
        assert_eq!(*drops.lock().unwrap(), vec![control_thread]);
        drop(guard);
        assert_eq!(drops.lock().unwrap().len(), 1);
    }
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
    endpoint_states: [Disposal; 2],
    client: Option<Box<crate::prepared::ClientEndpoints>>,
    publication: Option<Box<crate::prepared::PublicationControl>>,
    completed_stream_handle: bool,
    scalar_release_status: Option<usize>,
    domain_pages: [Option<crate::candidate_custody::DomainEnvelope>; 2],
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    native_sources: [Option<crate::candidate_custody::NativeEndpointEnvelope>;
        hex_lattice::MAX_STATEFUL_PROCESSORS],
    final_active: Option<crate::candidate_custody::FinalActiveReturn>,
    root: crate::candidate_custody::CandidateRoot,
}
impl NativeRetirement {
    pub(crate) fn new(
        terminals: NativeTerminals,
        root: crate::candidate_custody::CandidateRoot,
    ) -> Self {
        root.anchor_native_boundary(&terminals);
        let endpoint_states = if terminals.endpoints.is_some() {
            [Disposal::Unreceived; 2]
        } else {
            [Disposal::Complete; 2]
        };
        Self {
            terminals: Some(terminals),
            data: None,
            errors: None,
            states: [Disposal::Unreceived; 2],
            endpoint_states,
            client: None,
            publication: None,
            completed_stream_handle: false,
            scalar_release_status: None,
            domain_pages: std::array::from_fn(|_| None),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_sources: std::array::from_fn(|_| None),
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
    #[cfg(all(
        test,
        feature = "allocation-probe",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    pub(crate) fn test_native_source_carriers_returned(&self) -> bool {
        self.states[0] == Disposal::Complete && self.native_sources.iter().all(Option::is_none)
    }
    #[cfg(all(
        test,
        feature = "allocation-probe",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    pub(crate) fn test_native_source_carriers_pending(&self) -> usize {
        self.native_sources
            .iter()
            .filter(|source| source.is_some())
            .count()
    }
    pub(crate) fn bind_spectrum_reaper(
        &self,
        reaper: Option<&crate::spectrum_worker::SpectrumReaper>,
    ) {
        self.root
            .anchor_native_spectrum(reaper.and_then(|reaper| reaper.native_identity()));
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
    fn advance_endpoints(&mut self) -> Result<(), ()> {
        if self.endpoint_states == [Disposal::Complete; 2] {
            return Ok(());
        }
        let terminals = self
            .terminals
            .as_mut()
            .and_then(|t| t.endpoints.as_mut())
            .ok_or(())?;
        // Worker publication can disappear independently of the UI endpoint;
        // its actual Drop abandons return custody before graph cleanup below.
        if self.endpoint_states[1] == Disposal::Unreceived && terminals.publication.ready() {
            self.publication = terminals.publication.take();
            if self.publication.is_none() {
                return Err(());
            }
            self.endpoint_states[1] = Disposal::Received;
        }
        if self.endpoint_states[0] == Disposal::Unreceived && terminals.client.ready() {
            self.client = terminals.client.take();
            if self.client.is_none() {
                return Err(());
            }
            self.endpoint_states[0] = Disposal::Received;
        }
        Ok(())
    }
    /// Independent from possibly poisoned endpoint/callback owners. First publication wins.
    pub(crate) fn fail_retained_receipts(&self) {
        if let Some(endpoints) = self.terminals.as_ref().and_then(|t| t.endpoints.as_ref()) {
            endpoints.receipts.fail();
            if let Some(signal) = endpoints.action_failure.as_ref() {
                signal.retirement_unavailable();
            }
        }
    }
    fn dispose_endpoints(&mut self) {
        // Abandon worker publication before any graph cleanup, preserving existing custody.
        if self.endpoint_states[1] == Disposal::Received {
            self.endpoint_states[1] = Disposal::Pending;
            drop(self.publication.take());
            self.endpoint_states[1] = Disposal::Complete;
        }
        if self.endpoint_states[0] == Disposal::Received {
            self.endpoint_states[0] = Disposal::Pending;
            drop(self.client.take());
            self.endpoint_states[0] = Disposal::Complete;
        }
    }
    pub(crate) fn advance(&mut self) -> Result<bool, ()> {
        if self.states.contains(&Disposal::Pending)
            || self.endpoint_states.contains(&Disposal::Pending)
        {
            return Err(());
        }
        self.advance_endpoints()?;
        if self.states[0] == Disposal::Unreceived {
            if self.data.is_some() || self.errors.is_some() {
                return Err(());
            }
            let terminals = self.terminals.as_mut().ok_or(())?;
            if terminals.receive_into(&mut self.data, &mut self.errors)? {
                let data = self.data.as_ref().ok_or(())?;
                let errors = self.errors.as_ref().ok_or(())?;
                if !data.callback.belongs_to_candidate_root(&self.root)
                    || !data.callback.matches_native_status(&data.status)
                    || !Arc::ptr_eq(&data.status, &errors.status)
                {
                    return Err(());
                }
                self.scalar_release_status = Some(Arc::as_ptr(&data.status) as usize);
                self.states = [Disposal::Received; 2];
            }
        }
        let retaining = self
            .client
            .as_ref()
            .is_some_and(|c| c.retained_receipt_target.is_some());
        if retaining {
            if self.scalar_release_status.is_none() || !self.completed_stream_handle {
                return Ok(false);
            }
            let client = self.client.as_mut().ok_or(())?;
            if !self.scalar_release_status.is_some_and(|identity| {
                client.matches_retired_scalar_output(identity, self.root.root_id())
            }) {
                return Err(());
            }
            // No destructor precedes the copied result for a selected retired client.
            self.terminals
                .as_ref()
                .and_then(|t| t.endpoints.as_ref())
                .ok_or(())?
                .receipts
                .collect(client);
        }
        let action_outcomes = self
            .client
            .as_ref()
            .is_some_and(|c| c.has_action_admission())
            || self
                .data
                .as_ref()
                .is_some_and(|data| data.callback.has_action_writer());
        if action_outcomes {
            if !self.completed_stream_handle
                || self.scalar_release_status.is_none()
                || self.client.is_none()
                || self.data.is_none()
            {
                return Ok(false);
            }
            let client = self.client.as_mut().ok_or(())?;
            let data = self.data.as_mut().ok_or(())?;
            data.callback.retire_action_outcomes_on_control(
                client,
                &FinalReleaseAuthority {
                    root: self.root.root_id(),
                },
            )?;
        }
        self.dispose_endpoints();
        if self.states[0] == Disposal::Unreceived {
            return Ok(false);
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
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if data.callback.has_native_source_endpoints() {
                if !self.completed_stream_handle {
                    return Ok(false);
                }
                if !data
                    .callback
                    .extract_native_source_endpoints_on_control(&mut self.native_sources)
                {
                    return Err(());
                }
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
            if data.callback.has_active_ordinary_streams() {
                if !self.completed_stream_handle {
                    return Ok(false);
                }
                if !data.callback.release_final_ordinary_streams_on_control(
                    &FinalReleaseAuthority {
                        root: self.root.root_id(),
                    },
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
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        for index in 0..self.native_sources.len() {
            if let Some(endpoint) = self.native_sources[index].take()
                && let Err(endpoint) = self.root.ingest_native_endpoint(endpoint)
            {
                self.native_sources[index] = Some(endpoint);
                return Ok(false);
            }
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
        if let Some(returned) = self.final_active.take()
            && let Err(returned) = self.root.ingest_final(returned)
        {
            self.final_active = Some(returned);
            return Err(());
        }
        // Graph/domain work progresses even while the UI client remains live.
        // Only complete normal endpoint destruction can release the whole
        // native resource extent and its exclusive device lease.
        let graphs_complete = crate::lifecycle::finish_retirement_cleanup(&self.root);
        let endpoints_complete = self.endpoint_states == [Disposal::Complete; 2];
        if endpoints_complete {
            self.terminals = None;
        }
        Ok(graphs_complete && endpoints_complete)
    }
}
