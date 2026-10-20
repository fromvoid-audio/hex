//! Once-only, coupled native closure custody, adapted from original177
//! 8d444c06. Backend guard destruction moves a carrier; only lifecycle control
//! destroys its contents. Receiving a carrier is not final-active quiescence.
use crate::prepared::{
    Callback, Status,
    status_owner::{self, StatusHandle},
};
mod allocations;
mod registered;
pub(crate) use registered::{carrier_constructor_bound, reserve_for_root};
pub(crate) mod atomic_signal;
#[cfg(all(test, feature = "allocation-probe"))]
mod endpoint_tests;
mod resources;
mod ring_storage;
#[cfg(all(test, feature = "allocation-probe"))]
mod storage_tests;
pub(crate) use resources::{ChannelStorage, NativeCandidateBoundary};
pub(crate) use ring_storage::transport::{
    NativePreparedConsumer, NativePreparedProducer, native_prepared_ring, registered_prepared_ring,
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
    status: StatusHandle,
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
    status: StatusHandle,
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
            .store(true, std::sync::atomic::Ordering::Relaxed);
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
    storage: Option<hex_engine::sampler_host_fields::NativeCarrierStorageLease>,
}
pub(crate) struct Terminal<T> {
    cell: Option<Arc<OnceLock<Mutex<Box<T>>>>>,
    cell_storage: Option<hex_engine::sampler_host_fields::NativeCarrierStorageLease>,
    body_storage: Option<hex_engine::sampler_host_fields::NativeCarrierStorageLease>,
}
pub(crate) fn reserve<T>(value: T) -> (Guard<T>, Terminal<T>) {
    let cell = Arc::new(OnceLock::new());
    (
        Guard {
            state: GuardState::Live {
                owner: Box::new(value),
                cell: Arc::clone(&cell),
            },
            storage: None,
        },
        Terminal {
            cell: Some(cell),
            cell_storage: None,
            body_storage: None,
        },
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
    fn shared_cell(&self) -> Option<&Arc<OnceLock<Mutex<Box<T>>>>> {
        match &self.state {
            GuardState::Live { cell, .. } | GuardState::Published(cell) => Some(cell),
            GuardState::Closed => None,
        }
    }
    /// Nonblocking ownership transfer, leaving no live endpoint or anchor here.
    /// Only the constructor-paired terminal may later destroy the whole bundle.
    pub(crate) fn close(&mut self) {
        self.publish_for_drop();
        let mut storage = self.storage.take();
        if let Some(storage) = storage.as_mut() {
            // No wait, allocation or payload destruction is added to close.
            // A failed internal transition stays sticky in the Root record.
            let _ = storage.begin_actual_field_drop_on_control();
        }
        self.state = GuardState::Closed;
        if let Some(storage) = storage.as_mut() {
            let _ = storage.complete_actual_field_drop_on_control();
        }
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
        if let Some(storage) = self.cell_storage.as_mut() {
            // Actual unique terminal cell, BEFORE its Arc allocation is freed.
            if storage.begin_actual_field_drop_on_control().is_err() {
                self.cell = Some(cell);
                return None;
            }
        }
        match Arc::try_unwrap(cell) {
            Ok(cell) => {
                let owner = cell
                    .into_inner()
                    .map(|owner| owner.into_inner().unwrap_or_else(PoisonError::into_inner));
                if let Some(storage) = self.cell_storage.as_mut() {
                    let _ = storage.complete_actual_field_drop_on_control();
                }
                drop(self.cell_storage.take());
                owner
            }
            Err(cell) => {
                self.cell = Some(cell);
                None
            }
        }
    }
    fn begin_body_drop(&mut self) -> Result<(), ()> {
        self.body_storage.as_mut().map_or(Ok(()), |storage| {
            storage.begin_actual_field_drop_on_control().map_err(|_| ())
        })
    }
    fn complete_body_drop(&mut self) -> Result<(), ()> {
        if let Some(storage) = self.body_storage.as_mut() {
            storage
                .complete_actual_field_drop_on_control()
                .map_err(|_| ())?;
        }
        drop(self.body_storage.take());
        Ok(())
    }
}
pub(crate) struct NativeTerminals {
    data: Terminal<NativeCallbackOwner>,
    errors: Terminal<ErrorCallbackOwner>,
    endpoints: Option<crate::prepared::EndpointTerminals>,
}
pub(crate) fn reserve_native(
    callback: Callback,
    status: &StatusHandle,
) -> Result<
    (
        Guard<NativeCallbackOwner>,
        Guard<ErrorCallbackOwner>,
        NativeTerminals,
    ),
    hex_engine::Invalid,
> {
    let root = callback.candidate_root_for_native_boundary();
    if root.native_carrier_birth_limits().is_some() {
        registered::validate_birth_limits(&root)?;
        if !callback.matches_constructor_channel_fields_on_control(&root) {
            return Err(hex_engine::Invalid("native prepared queue fields differ"));
        }
    }
    // Every fallible role reservation precedes ANY native carrier allocation.
    let data_status = root.duplicate_status(status, status_owner::NATIVE_DATA)?;
    let error_status = root.duplicate_status(status, status_owner::NATIVE_ERROR)?;
    let (mut data, mut errors, terminals) =
        reserve_carriers(callback, data_status, error_status, None, &root)?;
    if let Some(boundary) = NativeCandidateBoundary::from_reserved(&root, &mut data, &mut errors) {
        root.record_native_boundary(boundary);
    }
    Ok((data, errors, terminals))
}
type NativeWorkerEndpoints = (
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeTerminals,
    crate::prepared::Client,
    crate::prepared::WorkerControl,
);
pub(crate) fn reserve_native_worker(
    control: crate::prepared::Control,
    callback: Callback,
    reader: Option<crate::spectrum_worker::SpectrumReader>,
) -> Result<NativeWorkerEndpoints, hex_engine::Invalid> {
    let root = control.native_candidate_root();
    if !root.same_constructor_as(&callback.candidate_root_for_native_boundary()) {
        return Err(hex_engine::Invalid("native worker callback source differs"));
    }
    // The two input owners are still borrowed here. Validate BOTH before the
    // first client/publication Box, not later when wrapping the data callback.
    if root.native_carrier_birth_limits().is_some() {
        registered::validate_birth_limits(&root)?;
        if !control.matches_fresh_registered_payload_on_control(&root)
            || !callback.matches_fresh_registered_payload_on_control(&root)
        {
            return Err(hex_engine::Invalid("native prepared queue fields differ"));
        }
    }
    let data_status = root.duplicate_status(&control.status, status_owner::NATIVE_DATA)?;
    let error_status = root.duplicate_status(&control.status, status_owner::NATIVE_ERROR)?;
    // Client role is also reserved before either endpoint carrier is born.
    let (client, publication, endpoints) = control.reserve_worker_endpoints(reader)?;
    let (mut data, mut errors, terminals) =
        reserve_carriers(callback, data_status, error_status, Some(endpoints), &root)?;
    if let Some(boundary) = NativeCandidateBoundary::from_reserved(&root, &mut data, &mut errors)
        .and_then(|boundary| boundary.with_worker_endpoints(&client, &publication))
    {
        root.record_native_boundary(boundary);
    }
    Ok((data, errors, terminals, client, publication))
}
fn reserve_carriers(
    callback: Callback,
    data_status: StatusHandle,
    error_status: StatusHandle,
    endpoints: Option<crate::prepared::EndpointTerminals>,
    root: &crate::candidate_custody::CandidateRoot,
) -> Result<
    (
        Guard<NativeCallbackOwner>,
        Guard<ErrorCallbackOwner>,
        NativeTerminals,
    ),
    hex_engine::Invalid,
> {
    let (data, data_terminal) = reserve_for_root(
        root,
        NativeCallbackOwner {
            callback,
            status: data_status,
            #[cfg(test)]
            probe: None,
        },
    )?;
    let (errors, error_terminal) = reserve_for_root(
        root,
        ErrorCallbackOwner {
            status: error_status,
            #[cfg(test)]
            probe: None,
        },
    )?;
    Ok((
        data,
        errors,
        NativeTerminals {
            data: data_terminal,
            errors: error_terminal,
            endpoints,
        },
    ))
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConstructionPhase {
    Preparing,
    CarriersStored,
    Installed,
    Aborted,
    FailedIncomplete,
}
pub(crate) struct NativeRetirement {
    construction: ConstructionPhase,
    terminals: Option<NativeTerminals>,
    data: Option<Box<NativeCallbackOwner>>,
    errors: Option<Box<ErrorCallbackOwner>>,
    states: [Disposal; 2],
    endpoint_states: [Disposal; 2],
    client: Option<Box<crate::prepared::ClientEndpoints>>,
    publication: Option<Box<crate::prepared::PublicationControl>>,
    completed_stream_handle: bool,
    domain_pages: [Option<crate::candidate_custody::DomainEnvelope>; 2],
    final_active: Option<crate::candidate_custody::FinalActiveReturn>,
    root: crate::candidate_custody::RootActor,
}
impl NativeRetirement {
    // This only inspects this actual parent's completed native fields. The
    // lifecycle caller must ALSO have joined/dropped its actual Reaper before
    // moving this DATA-only tail; no caller-supplied frontier flag is accepted.
    pub(crate) fn captured_data_tail_matches_on_control(
        &self,
        root: &crate::candidate_custody::CandidateRoot,
    ) -> bool {
        self.completed_stream_handle
            && self.states == [Disposal::Complete; 2]
            && self.data.is_none()
            && self.errors.is_none()
            && self.domain_pages.iter().all(Option::is_none)
            && self.final_active.is_none()
            && self.root.is_retirement_field()
            && self.root.same_root(root)
    }
    pub(crate) fn advance_captured_data_endpoints_on_control(&mut self) -> Result<bool, ()> {
        if !self.completed_stream_handle
            || self.states != [Disposal::Complete; 2]
            || self.data.is_some()
            || self.errors.is_some()
            || self.domain_pages.iter().any(Option::is_some)
            || self.final_active.is_some()
        {
            return Err(());
        }
        self.advance_endpoints()?;
        let complete = self.endpoint_states == [Disposal::Complete; 2];
        if complete {
            self.terminals = None;
        }
        Ok(complete)
    }
    #[cfg(any(test, feature = "allocation-probe"))]
    pub(crate) fn new(
        terminals: NativeTerminals,
        root: crate::candidate_custody::CandidateRoot,
    ) -> Self {
        let mut owner = Self::begin_construction(root);
        let _installed = owner.install_constructed_carriers(&mut Some(terminals));
        owner
    }
    pub(crate) fn begin_construction(root: crate::candidate_custody::CandidateRoot) -> Self {
        let root = root.retirement_root_field();
        Self {
            construction: ConstructionPhase::Preparing,
            terminals: None,
            data: None,
            errors: None,
            states: [Disposal::Unreceived; 2],
            endpoint_states: [Disposal::Unreceived; 2],
            client: None,
            publication: None,
            completed_stream_handle: false,
            domain_pages: std::array::from_fn(|_| None),
            final_active: None,
            root,
        }
    }
    pub(crate) fn install_constructed_carriers(
        &mut self,
        incoming: &mut Option<NativeTerminals>,
    ) -> Result<(), ()> {
        if self.construction != ConstructionPhase::Preparing || self.terminals.is_some() {
            return Err(());
        }
        self.terminals = Some(incoming.take().ok_or(())?);
        self.construction = ConstructionPhase::CarriersStored;
        let terminals = self.terminals.as_ref().ok_or(())?;
        if !self.root.anchor_native_boundary(terminals) {
            self.construction = ConstructionPhase::FailedIncomplete;
            return Err(());
        }
        self.endpoint_states = if terminals.endpoints.is_some() {
            [Disposal::Unreceived; 2]
        } else {
            [Disposal::Complete; 2]
        };
        self.construction = ConstructionPhase::Installed;
        Ok(())
    }
    // Only the constructor's proven pre-carrier failure route calls this after
    // its actual unsplit channel/capture values have been normally destroyed.
    pub(crate) fn abort_before_carriers(&mut self) -> Result<(), ()> {
        if self.construction != ConstructionPhase::Preparing || self.terminals.is_some() {
            return Err(());
        }
        self.construction = ConstructionPhase::Aborted;
        self.states = [Disposal::Complete; 2];
        self.endpoint_states = [Disposal::Complete; 2];
        Ok(())
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
    pub(crate) fn bind_spectrum_reaper(
        &self,
        reaper: Option<&crate::spectrum_worker::SpectrumReaper>,
    ) {
        self.root
            .anchor_native_spectrum(reaper.and_then(|reaper| reaper.native_identity()));
    }
    pub(crate) fn pair_spectrum_packet_on_control(
        &self,
        index: usize,
        fields: hex_engine::sampler_host_fields::NativeSpectrumPacketFields<'_>,
    ) -> Result<
        hex_engine::sampler_host_fields::NativeSpectrumPacketLease,
        hex_engine::AudioStorageError,
    > {
        self.root.pair_spectrum_packet_on_control(index, fields)
    }
    pub(crate) fn pair_spectrum_result_on_control(
        &self,
        index: usize,
        fields: hex_engine::sampler_host_fields::NativeSpectrumResultFields<'_>,
    ) -> Result<
        (
            hex_engine::sampler_host_fields::NativeSpectrumResultLease,
            hex_engine::sampler_host_fields::NativeSpectrumResultHome,
        ),
        hex_engine::AudioStorageError,
    > {
        self.root.pair_spectrum_result_on_control(index, fields)
    }
    pub(crate) fn spectrum_stop_pair_on_control(
        &self,
    ) -> Result<
        [hex_engine::sampler_host_fields::NativeAtomicSignal; 2],
        hex_engine::AudioStorageError,
    > {
        self.root.spectrum_stop_pair_on_control()
    }
    pub(crate) fn matches_spectrum_stop(
        &self,
        stop: &crate::spectrum_worker::SpectrumStop,
    ) -> bool {
        self.root.matches_spectrum_stop(stop)
    }
    pub(crate) fn spectrum_project_ring_on_control<T>(
        &self,
        index: usize,
        capacity: usize,
    ) -> Result<
        (
            crate::native_callback_owner::NativePreparedProducer<T>,
            crate::native_callback_owner::NativePreparedConsumer<T>,
        ),
        hex_engine::AudioStorageError,
    > {
        self.root.spectrum_project_ring_on_control(index, capacity)
    }
    pub(crate) fn spectrum_capture_ring_on_control(
        &self,
    ) -> Result<
        (
            NativePreparedProducer<crate::spectrum_worker::CaptureBatch>,
            NativePreparedConsumer<crate::spectrum_worker::CaptureBatch>,
        ),
        hex_engine::AudioStorageError,
    > {
        self.root.spectrum_capture_ring_on_control()
    }
    pub(crate) fn spectrum_result_ring_on_control(
        &self,
    ) -> Result<
        (
            NativePreparedProducer<crate::spectrum_worker::PublishedSpectra>,
            NativePreparedConsumer<crate::spectrum_worker::PublishedSpectra>,
        ),
        hex_engine::AudioStorageError,
    > {
        self.root.spectrum_result_ring_on_control()
    }
    pub(crate) fn pair_spectrum_capture_on_control(
        &self,
        fields: hex_engine::sampler_host_fields::NativeSpectrumCaptureFields<'_>,
    ) -> Result<
        (
            hex_engine::sampler_host_fields::NativeSpectrumCaptureLease,
            hex_engine::sampler_host_fields::NativeSpectrumJoinLease,
        ),
        hex_engine::AudioStorageError,
    > {
        self.root.pair_spectrum_capture_on_control(fields)
    }
    pub(crate) fn finish_spectrum_capture_on_control(
        &self,
        join: &mut hex_engine::sampler_host_fields::NativeSpectrumJoinLease,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.root.finish_spectrum_capture_on_control(join)
    }
    pub(crate) fn abort_spectrum_capture_on_control(
        &self,
        join: &mut hex_engine::sampler_host_fields::NativeSpectrumJoinLease,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.root.abort_spectrum_capture_on_control(join)
    }
    pub(crate) fn root(&self) -> crate::candidate_custody::CandidateRoot {
        self.root.clone()
    }
    /// Supply the exact completed-handle frontier from the owning lifecycle
    /// bundle. This plus both final guard releases permits application-only
    /// nonrenderability; native stop/dispose-success is never inferred.
    #[cfg(all(test, feature = "allocation-probe"))]
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
    /// Retire only the actual native callback/capture/error paths. Control
    /// endpoints deliberately remain untouched until the owning parent joins
    /// its analyzer; independently live UI/producer fields cannot block join.
    pub(crate) fn advance_native_fields_after_stream_disposal(
        &mut self,
        completed: &crate::lifecycle::CompletedStreamHandle,
    ) -> Result<bool, ()> {
        if !completed.matches(self.root.root_id()) {
            return Err(());
        }
        self.completed_stream_handle = true;
        self.advance_native_fields()
    }
    pub(crate) fn finish_root_after_native_fields_on_control(&mut self) -> Result<bool, ()> {
        if self.states != [Disposal::Complete; 2] {
            return Err(());
        }
        self.advance_endpoints()?;
        self.finish_root_on_control()
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
        if self.endpoint_states[1] == Disposal::Received {
            terminals.publication.begin_body_drop()?;
            self.endpoint_states[1] = Disposal::Pending;
            drop(self.publication.take());
            terminals.publication.complete_body_drop()?;
            self.endpoint_states[1] = Disposal::Complete;
        }
        if self.endpoint_states[0] == Disposal::Unreceived && terminals.client.ready() {
            self.client = terminals.client.take();
            if self.client.is_none() {
                return Err(());
            }
            self.endpoint_states[0] = Disposal::Received;
        }
        if self.endpoint_states[0] == Disposal::Received {
            terminals.client.begin_body_drop()?;
            self.endpoint_states[0] = Disposal::Pending;
            drop(self.client.take());
            terminals.client.complete_body_drop()?;
            self.endpoint_states[0] = Disposal::Complete;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn advance(&mut self) -> Result<bool, ()> {
        // Standalone no-analyzer fixture progress never becomes a join bypass.
        // This negative gate grants no positive resource/identity authority.
        if self.root.test_requires_analyzer_parent() {
            return Err(());
        }
        if !self.root.is_retirement_field() {
            return Err(());
        }
        if self.states.contains(&Disposal::Pending)
            || self.endpoint_states.contains(&Disposal::Pending)
        {
            return Err(());
        }
        self.advance_endpoints()?;
        if !self.advance_native_fields()? {
            return Ok(false);
        }
        self.finish_root_on_control()
    }
    fn advance_native_fields(&mut self) -> Result<bool, ()> {
        if !matches!(
            self.construction,
            ConstructionPhase::Installed | ConstructionPhase::Aborted
        ) || !self.root.is_retirement_field()
            || self.states.contains(&Disposal::Pending)
            || self.endpoint_states.contains(&Disposal::Pending)
        {
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
            self.terminals.as_mut().ok_or(())?.data.begin_body_drop()?;
            self.states[0] = Disposal::Pending;
            drop(self.data.take());
            self.terminals
                .as_mut()
                .ok_or(())?
                .data
                .complete_body_drop()?;
            self.states[0] = Disposal::Complete;
        }
        if self.states[1] == Disposal::Received {
            if self.errors.is_none() {
                return Err(());
            }
            self.terminals
                .as_mut()
                .ok_or(())?
                .errors
                .begin_body_drop()?;
            self.states[1] = Disposal::Pending;
            drop(self.errors.take());
            self.terminals
                .as_mut()
                .ok_or(())?
                .errors
                .complete_body_drop()?;
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
        Ok(true)
    }
    fn finish_root_on_control(&mut self) -> Result<bool, ()> {
        // Graph/domain work progresses even while the UI client remains live.
        // Only complete normal endpoint destruction can release the whole
        // native resource extent and its exclusive device lease.
        let graphs_complete = self.root.finish_on_control();
        let endpoints_complete = self.endpoint_states == [Disposal::Complete; 2];
        if endpoints_complete {
            self.terminals = None;
        }
        Ok(graphs_complete && endpoints_complete)
    }
}
