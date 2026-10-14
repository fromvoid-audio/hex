use super::*;

/// Shared exclusively among control/lifecycle owners. Callback rendering never
/// locks or clones this root; only its confirmed disposal path transfers a batch.
#[derive(Clone)]
pub(crate) struct NativeStreamRoot {
    inner: Arc<NativeRootOwner>,
    device_epoch: u64,
}
struct NativeStreamState {
    bridge: Option<StreamBridgeControl>,
    preparation: Option<crate::stream_preparation::PreparationSlot>,
    closing: bool,
    busy_incarnation: Option<u64>,
    faulted: bool,
    failure_reports: [Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>; RETIRED],
}
struct NativeRootOwner {
    ordinary: std::sync::Mutex<NativeStreamState>,
    cleanup: std::sync::Mutex<CleanupCustody>,
    tag: Arc<()>,
}
#[derive(Default)]
struct CleanupCustody {
    incarnation: u64,
    capsule: Option<CleanupCapsule>,
    faulted: bool,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
    #[cfg(test)]
    before_work: Option<Box<dyn FnMut() + Send>>,
}
struct CleanupCapsule {
    bridge: StreamBridgeControl,
    preparation: Option<crate::stream_preparation::PreparationSlot>,
    returns: Option<Consumer<GraphReturn>>,
}
pub(crate) struct NativeCleanupPermit<'a> {
    _custody: std::sync::MutexGuard<'a, CleanupCustody>,
    root: &'a NativeStreamRoot,
}
impl NativeCleanupPermit<'_> {
    pub(crate) fn fail_retirement_on_control(&mut self) {
        self._custody.faulted = true;
        self.root.latch_cleanup_fault(&self._custody);
    }
}
impl NativeStreamRoot {
    fn preparation_storage_bytes() -> Option<usize> {
        std::mem::size_of::<NativeRootOwner>()
            .checked_sub(std::mem::size_of::<std::sync::Mutex<StreamBridgeControl>>())
    }
    pub(crate) fn current_preparation_snapshot(
        &self,
    ) -> Option<crate::stream_preparation::PreparationSnapshot> {
        self.inner
            .ordinary
            .try_lock()
            .ok()?
            .preparation
            .as_ref()?
            .current_snapshot()
    }
    #[cfg(test)]
    pub(crate) fn preparation_original_addresses(
        &self,
        token: crate::stream_preparation::PreparationToken,
    ) -> Option<(usize, usize, usize, usize)> {
        self.lock().preparation.as_ref()?.original_addresses(token)
    }
    pub(crate) fn begin_preparation(
        &self,
        input: crate::stream_preparation::CandidateInput,
        rate: u32,
    ) -> Result<
        crate::stream_preparation::PreparationToken,
        crate::stream_preparation::PreparationRefusal,
    > {
        let Ok(mut root) = self.inner.ordinary.try_lock() else {
            return Err(crate::stream_preparation::PreparationRefusal {
                reason: "stream control cleanup is busy",
                input: Box::new(input),
            });
        };
        if root.closing {
            return Err(crate::stream_preparation::PreparationRefusal {
                reason: "stream preparation is closing",
                input: Box::new(input),
            });
        }
        let Some(slot) = root.preparation.as_mut() else {
            return Err(crate::stream_preparation::PreparationRefusal {
                reason: "stream preparation lifecycle is busy",
                input: Box::new(input),
            });
        };
        slot.begin(
            input,
            rate,
            self.device_epoch,
            Self::preparation_storage_bytes().unwrap_or(usize::MAX),
        )
    }
    pub(crate) fn preparation_snapshot(
        &self,
        token: crate::stream_preparation::PreparationToken,
    ) -> Option<crate::stream_preparation::PreparationSnapshot> {
        self.inner
            .ordinary
            .try_lock()
            .ok()?
            .preparation
            .as_ref()?
            .snapshot(token)
    }
    pub(crate) fn cancel_preparation(
        &self,
        token: crate::stream_preparation::PreparationToken,
    ) -> bool {
        self.inner
            .ordinary
            .try_lock()
            .ok()
            .and_then(|mut root| root.preparation.as_mut().map(|slot| slot.cancel(token)))
            .unwrap_or(false)
    }
    pub(crate) fn consume_preparation(
        &self,
        token: crate::stream_preparation::PreparationToken,
    ) -> bool {
        self.inner
            .ordinary
            .try_lock()
            .ok()
            .and_then(|mut root| root.preparation.as_mut().map(|slot| slot.consume(token)))
            .unwrap_or(false)
    }
    pub(crate) fn publish_preparation(
        &self,
        control: &mut Control,
        token: crate::stream_preparation::PreparationToken,
        current: crate::stream_preparation::CandidateIdentity,
        timeline_epoch: u64,
        policy: PublicationPolicy,
    ) -> Result<Option<u64>, &'static str> {
        let mut root = self
            .inner
            .ordinary
            .try_lock()
            .map_err(|_| "stream control cleanup is busy")?;
        if root.closing {
            return Err("stream preparation is closing");
        }
        let mut occupied = [None; super::super::RETIRED];
        root.bridge
            .as_ref()
            .ok_or("stream control cleanup is busy")?
            .reaper
            .visit_domains(|index, id| occupied[index] = Some(id));
        let NativeStreamState {
            bridge,
            preparation,
            ..
        } = &mut *root;
        let Some(slot) = preparation.as_mut() else {
            return Err("stream preparation lifecycle is busy");
        };
        let bridge = bridge.as_mut().ok_or("stream control cleanup is busy")?;
        slot.publish(
            token,
            current,
            timeline_epoch,
            policy,
            &occupied,
            |request| control.publish_streamed(bridge, request),
        )
    }
    pub(crate) fn cancel_all_preparation(&self) {
        let mut root = self.lock();
        root.closing = true;
        if let Some(bridge) = root.bridge.as_mut() {
            bridge.claim_union_closed = true;
        }
        if let Some(slot) = root.preparation.as_mut() {
            slot.close();
        }
    }
    pub(crate) fn catalog_lease(&self, id: hex_stream::DomainId) -> Option<CatalogLease> {
        let root = self.inner.ordinary.try_lock().ok()?;
        if root.closing || root.faulted {
            return None;
        }
        let bridge = root.bridge.as_ref()?;
        if bridge
            .return_requests
            .iter()
            .any(|flag| flag.load(Acquire) == id.value())
        {
            return None;
        }
        bridge.reaper.catalog_lease(id)
    }
    pub(crate) fn input_bridge_storage_bytes(&self) -> Option<usize> {
        self.inner
            .ordinary
            .try_lock()
            .ok()?
            .bridge
            .as_ref()?
            .input_bridge_storage_bytes()?
            .checked_add(
                std::mem::size_of::<NativeRootOwner>()
                    .checked_sub(std::mem::size_of::<std::sync::Mutex<StreamBridgeControl>>())?,
            )
    }
    pub(crate) fn pool_reports(
        &self,
    ) -> Option<[Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>; RETIRED]> {
        let root = self.inner.ordinary.try_lock().ok()?;
        if root.faulted {
            return Some(root.failure_reports);
        }
        Some(root.bridge.as_ref()?.pool_reports())
    }
    pub(crate) fn attach(
        control: &mut Control,
        callback: &mut Callback,
        mut bridge: StreamBridgeControl,
    ) -> Self {
        bridge.native_retired = control.retired.take();
        let device_epoch = bridge.device_epoch;
        let tag = Arc::clone(&bridge.tag); // existing identity; control only
        let root = Self {
            inner: Arc::new(NativeRootOwner {
                ordinary: std::sync::Mutex::new(NativeStreamState {
                    bridge: Some(bridge),
                    preparation: Some(crate::stream_preparation::PreparationSlot::default()),
                    closing: false,
                    busy_incarnation: None,
                    faulted: false,
                    failure_reports: [None; RETIRED],
                }),
                cleanup: std::sync::Mutex::new(CleanupCustody::default()),
                tag,
            }),
            device_epoch,
        };
        callback.native_stream_root = Some(root.clone());
        root
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, NativeStreamState> {
        self.inner
            .ordinary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn publish(
        &self,
        control: &mut Control,
        request: StreamPublicationRequest,
    ) -> Result<u64, StreamPublishError> {
        match self.inner.ordinary.try_lock() {
            Ok(root) if root.closing || root.faulted => Err(StreamPublishError {
                reason: "stream root is closing or faulted",
                retryable: false,
                request: Box::new(request),
            }),
            Ok(mut root) => match root.bridge.as_mut() {
                Some(bridge) => control.publish_streamed(bridge, request),
                None => Err(StreamPublishError {
                    reason: "stream control cleanup is busy",
                    retryable: true,
                    request: Box::new(request),
                }),
            },
            Err(_) => Err(StreamPublishError {
                reason: "stream control cleanup is busy",
                retryable: true,
                request: Box::new(request),
            }),
        }
    }
    pub(crate) fn device_epoch(&self) -> u64 {
        self.device_epoch
    }
    pub(crate) fn phase(&self) -> Option<hex_stream::MediaCleanupPhase> {
        let root = self.inner.ordinary.try_lock().ok()?;
        if root.faulted {
            return Some(hex_stream::MediaCleanupPhase::FailedIncomplete);
        }
        let Some(bridge) = root.bridge.as_ref() else {
            return Some(hex_stream::MediaCleanupPhase::Pending);
        };
        Some(combined_phase(
            root.preparation.as_ref().map(|p| p.phase()),
            bridge.cleanup_phase(),
        ))
    }
    /// Holds only control custody, never the ordinary mutex during owner disposal.
    pub(crate) fn cleanup_permit_storage_bytes() -> usize {
        std::mem::size_of::<Option<NativeCleanupPermit<'static>>>()
    }
    pub(crate) fn cleanup_permit(
        &self,
    ) -> Result<NativeCleanupPermit<'_>, hex_stream::MediaCleanupPhase> {
        let custody = match self.inner.cleanup.try_lock() {
            Ok(custody) => custody,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(hex_stream::MediaCleanupPhase::Pending);
            }
            Err(std::sync::TryLockError::Poisoned(error)) => {
                let mut custody = error.into_inner();
                custody.faulted = true;
                self.latch_cleanup_fault(&custody);
                return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
            }
        };
        if custody.faulted || custody.capsule.is_some() {
            return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
        }
        {
            let root = self
                .inner
                .ordinary
                .try_lock()
                .map_err(|_| hex_stream::MediaCleanupPhase::Pending)?;
            if root.faulted {
                return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
            }
            if root.busy_incarnation.is_some() || root.bridge.is_none() {
                return Err(hex_stream::MediaCleanupPhase::Pending);
            }
        }
        Ok(NativeCleanupPermit {
            _custody: custody,
            root: self,
        })
    }
    pub(crate) fn tick(&self, final_retirement: bool) -> hex_stream::MediaCleanupPhase {
        self.cleanup_transaction(final_retirement, false).0
    }
    pub(crate) fn take_report(
        &self,
    ) -> Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)> {
        self.cleanup_transaction(false, true).1
    }
    /// Actual lifecycle teardown uncertainty; no completion/retirement receipt.
    pub(crate) fn fail_retirement_on_control(&self) {
        let mut custody = self
            .inner
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        custody.faulted = true;
        self.latch_cleanup_fault(&custody);
    }
    fn latch_cleanup_fault(&self, custody: &CleanupCustody) {
        let reports = custody
            .capsule
            .as_ref()
            .map(|c| c.bridge.pool_reports())
            .unwrap_or([None; RETIRED]);
        let mut root = self.lock();
        root.failure_reports = if custody.capsule.is_some() {
            reports
        } else {
            root.bridge
                .as_ref()
                .map(|b| b.pool_reports())
                .unwrap_or(reports)
        };
        root.faulted = true;
    }
    fn cleanup_transaction(
        &self,
        final_retirement: bool,
        report_only: bool,
    ) -> (
        hex_stream::MediaCleanupPhase,
        Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>,
    ) {
        use hex_stream::MediaCleanupPhase as Phase;
        let mut custody = match self.inner.cleanup.try_lock() {
            Ok(custody) => custody,
            Err(std::sync::TryLockError::WouldBlock) => return (Phase::Pending, None),
            Err(std::sync::TryLockError::Poisoned(error)) => {
                let mut custody = error.into_inner();
                custody.faulted = true;
                self.latch_cleanup_fault(&custody);
                return (Phase::FailedIncomplete, None);
            }
        };
        if custody.faulted || custody.capsule.is_some() {
            return (Phase::FailedIncomplete, None);
        }
        let closing;
        {
            let Ok(mut root) = self.inner.ordinary.try_lock() else {
                return (Phase::Pending, None);
            };
            if root.faulted {
                return (Phase::FailedIncomplete, None);
            }
            let Some(bridge) = root.bridge.as_ref() else {
                return (Phase::Pending, None);
            };
            if !Arc::ptr_eq(&bridge.tag, &self.inner.tag) || root.busy_incarnation.is_some() {
                return (Phase::FailedIncomplete, None);
            }
            let Some(next) = custody.incarnation.checked_add(1) else {
                root.faulted = true;
                return (Phase::FailedIncomplete, None);
            };
            root.closing |= final_retirement;
            closing = root.closing;
            // All destinations validated. Move directly into persistent custody.
            if let Some(mut bridge) = root.bridge.take() {
                let returns = bridge.native_retired.take();
                custody.capsule = Some(CleanupCapsule {
                    bridge,
                    preparation: root.preparation.take(),
                    returns,
                });
                custody.incarnation = next;
                root.busy_incarnation = Some(next);
            }
        }
        // Only a borrow crosses unwind. Capsule/first payload remain preanchored.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            if let Some(hook) = custody.before_work.as_mut() {
                hook();
            }
            match custody.capsule.as_mut() {
                Some(capsule) => capsule.run(closing, final_retirement, report_only),
                None => (Phase::FailedIncomplete, None),
            }
        }));
        let result = match outcome {
            Ok(result) if result.0 != Phase::FailedIncomplete => result,
            Ok(_) => {
                custody.faulted = true;
                self.latch_cleanup_fault(&custody);
                return (Phase::FailedIncomplete, None);
            }
            Err(payload) => {
                custody.first_panic = Some(payload); // first attempt only; no destructive retry
                custody.faulted = true;
                self.latch_cleanup_fault(&custody);
                return (Phase::FailedIncomplete, None);
            }
        };
        {
            let mut root = self.lock();
            let valid = root.busy_incarnation == Some(custody.incarnation)
                && root.bridge.is_none()
                && root.preparation.is_none()
                && !root.faulted
                && custody
                    .capsule
                    .as_ref()
                    .is_some_and(|c| Arc::ptr_eq(&c.bridge.tag, &self.inner.tag));
            if !valid {
                root.failure_reports = custody
                    .capsule
                    .as_ref()
                    .map(|c| c.bridge.pool_reports())
                    .unwrap_or([None; RETIRED]);
                root.faulted = true;
                custody.faulted = true;
                return (Phase::FailedIncomplete, None);
            }
            if let Some(mut capsule) = custody.capsule.take() {
                capsule.bridge.claim_union_closed |= root.closing;
                if root.closing {
                    if let Some(preparation) = capsule.preparation.as_mut() {
                        preparation.close();
                    }
                }
                capsule.bridge.native_retired = capsule.returns.take();
                root.bridge = Some(capsule.bridge);
                root.preparation = capsule.preparation;
                root.busy_incarnation = None;
            }
        }
        result
    }
}
fn combined_phase(
    preparation: Option<hex_stream::MediaCleanupPhase>,
    bridge: hex_stream::MediaCleanupPhase,
) -> hex_stream::MediaCleanupPhase {
    use hex_stream::MediaCleanupPhase as Phase;
    match (preparation, bridge) {
        (Some(Phase::FailedIncomplete), _) | (_, Phase::FailedIncomplete) => {
            Phase::FailedIncomplete
        }
        (Some(Phase::Complete), Phase::Complete) => Phase::Complete,
        _ => Phase::Pending,
    }
}
impl CleanupCapsule {
    fn run(
        &mut self,
        closing: bool,
        final_retirement: bool,
        report_only: bool,
    ) -> (
        hex_stream::MediaCleanupPhase,
        Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>,
    ) {
        let mut report = None;
        if !report_only {
            self.bridge.claim_union_closed |= closing;
            if let Some(slots) = self.bridge.claim_union.as_mut() {
                if closing {
                    slots.close_unstaged();
                }
            }
            if final_retirement
                && self
                    .bridge
                    .claim_union
                    .as_ref()
                    .is_some_and(|s| s.phase() == hex_stream::MediaCleanupPhase::Complete)
            {
                drop(self.bridge.claim_union.take());
                self.bridge.claim_union_enabled = false;
            }
            if let Some(preparation) = self.preparation.as_mut() {
                preparation.tick(closing);
            }
            if let Some(returns) = self.returns.as_mut() {
                let _ = self.bridge.reclaim_receiver(returns, false);
            }
        }
        if report_only || (final_retirement && self.bridge.native_callback_ended) {
            for index in 0..RETIRED {
                if let Some((id, actual)) = self.bridge.native_reports[index] {
                    if self.bridge.reaper.take_reclaimed_report(id).is_some() {
                        self.bridge.native_reports[index] = None;
                        self.bridge.domain_contexts[index] = None;
                        self.bridge.return_requests[index].store(0, Release);
                        if report_only {
                            report = Some((id, actual));
                            break;
                        }
                    }
                }
            }
        }
        (
            combined_phase(
                self.preparation.as_ref().map(|p| p.phase()),
                self.bridge.cleanup_phase(),
            ),
            report,
        )
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        let Some(root) = self.native_stream_root.take() else {
            return;
        };
        // Native StreamOwner destroys this closure only after successful pause,
        // on lifecycle/control. Never call this disposal path from audio render.
        let Some(stream) = self.streaming.as_mut() else {
            return;
        };
        for publication in [&mut self.active, &mut self.deferred].into_iter().flatten() {
            let _installed = stream.install_publication(publication);
        }
        if let Some(binding) = self
            .held_retirement
            .as_mut()
            .and_then(GraphReturn::stream_source_mut)
        {
            let _installed = stream.install_binding(binding);
        }
        while let Ok(mut publication) = self.publications.pop() {
            let _installed = stream.install_publication(&mut publication);
            if let Some(admission) = publication
                .stream_source
                .as_mut()
                .and_then(|binding| binding.admission.take())
            {
                let mut root = root.lock();
                let Some(bridge) = root.bridge.as_mut() else {
                    root.faulted = true;
                    return;
                };
                if let Err(admission) = bridge.reaper.recover_uninstalled(admission) {
                    let Some(slot) = bridge
                        .native_failed_admissions
                        .iter_mut()
                        .find(|slot| slot.is_none())
                    else {
                        unreachable!("two domain admissions");
                    };
                    *slot = Some(admission);
                }
            }
            drop(publication); // Confirmed control-side closure disposal.
        }
        let mut root = root.lock();
        let Some(bridge) = root.bridge.as_mut() else {
            root.faulted = true;
            return;
        };
        for publication in [&mut self.active, &mut self.deferred].into_iter().flatten() {
            if let Some(admission) = publication
                .stream_source
                .as_mut()
                .and_then(|binding| binding.admission.take())
            {
                if let Err(admission) = bridge.reaper.recover_uninstalled(admission) {
                    let Some(slot) = bridge
                        .native_failed_admissions
                        .iter_mut()
                        .find(|slot| slot.is_none())
                    else {
                        unreachable!("two domain admissions");
                    };
                    *slot = Some(admission);
                }
            }
        }
        if let Some(admission) = self
            .held_retirement
            .as_mut()
            .and_then(GraphReturn::stream_source_mut)
            .and_then(|binding| binding.admission.take())
        {
            if let Err(admission) = bridge.reaper.recover_uninstalled(admission) {
                let slot = bridge
                    .native_failed_admissions
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .expect("at most two actual domain admissions");
                *slot = Some(admission);
            }
        }
        if let Some(batch) = stream.batch.take() {
            match bridge.terminal.try_return_stopped_batch(batch) {
                Ok(()) => bridge.native_callback_ended = true,
                Err(error) => {
                    bridge.native_failed_batch = Some(error.into_batch());
                }
            }
        }
    }
}

#[cfg(test)]
mod preparation_race_tests {
    use super::*;
    #[test]
    fn retirement_close_survives_slot_temporarily_owned_by_lifecycle() {
        let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        let (input, _file) = super::super::tests::facade_input(108);
        let token = root.begin_preparation(input, 48_000).unwrap();
        let mut slot = root.lock().preparation.take().unwrap();
        root.cancel_all_preparation(); // Close flag must persist when slot is out.
        let (input, _second_file) = super::super::tests::facade_input(109);
        assert_eq!(
            root.begin_preparation(input, 48_000).unwrap_err().reason,
            "stream preparation is closing"
        );
        let closing = root.lock().closing;
        assert!(closing);
        slot.tick(closing);
        assert!(slot.snapshot(token).is_none());
        root.lock().preparation = Some(slot);
        drop(control);
        drop(callback);
        root.tick(true);
        assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
    }
}

#[cfg(test)]
mod claim_storage_tests {
    use super::*;
    #[test]
    fn actual_root_busy_take_and_control_disposal_outside_media_mutex() {
        use crate::publication_claims::ClaimSlotError;
        use std::sync::{Arc, Mutex};
        let (mut control, mut callback, mut bridge) = channel_with_streaming(48_000, 9, 4).unwrap();
        let reservation = hex_engine::storage_report::ClaimReservation {
            items: 2,
            anchors: 4,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            other_retained_bytes: 0,
            other_construction_bytes: 0,
        };
        let report = bridge
            .reserve_claim_storage_on_control(reservation)
            .unwrap();
        assert!(report.slots >= 2);
        let before = bridge.input_bridge_storage_bytes().unwrap();
        assert!(before >= report.bytes);
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        let observed = Arc::new(Mutex::new(None));
        let weak = Arc::downgrade(&root.inner);
        let result = Arc::clone(&observed);
        root.lock()
            .bridge
            .as_mut()
            .unwrap()
            .claim_union
            .as_mut()
            .unwrap()
            .observe_drop(Box::new(move || {
                let actual = weak.upgrade().unwrap();
                *result.lock().unwrap() = Some((
                    std::thread::current().id(),
                    actual.ordinary.try_lock().is_ok(),
                ));
            }));
        assert!(
            root.lock()
                .bridge
                .as_mut()
                .unwrap()
                .claim_publication_open()
                .is_ok()
        );
        let token = root
            .lock()
            .bridge
            .as_mut()
            .unwrap()
            .reserve_claim_placeholder_on_control()
            .unwrap();
        assert!(
            root.phase()
                .is_some_and(|p| p == hex_stream::MediaCleanupPhase::Pending)
        );
        let held = root
            .lock()
            .bridge
            .as_mut()
            .unwrap()
            .claim_union
            .take()
            .unwrap();
        let _ = root.tick(true); // concurrent-style busy marker cannot clear/recreate root
        assert!(root.lock().bridge.as_mut().unwrap().claim_union_enabled);
        assert!(root.lock().bridge.as_mut().unwrap().claim_union.is_none());
        assert!(matches!(
            root.lock()
                .bridge
                .as_mut()
                .unwrap()
                .claim_publication_open(),
            Err(ClaimSlotError::Busy)
        ));
        assert!(observed.lock().unwrap().is_none());
        root.lock().bridge.as_mut().unwrap().claim_union = Some(held);
        let _ = root.tick(true); // only unstaged/no-owner reservation may be cancelled
        assert_eq!(
            *observed.lock().unwrap(),
            Some((std::thread::current().id(), true))
        );
        assert!(!root.lock().bridge.as_mut().unwrap().claim_union_enabled);
        assert!(root.lock().bridge.as_mut().unwrap().claim_union_closed);
        assert!(matches!(
            root.lock()
                .bridge
                .as_mut()
                .unwrap()
                .reserve_claim_storage_on_control(reservation),
            Err(ClaimSlotError::Busy)
        ));
        assert!(matches!(
            root.lock()
                .bridge
                .as_mut()
                .unwrap()
                .rollback_claim_placeholder_on_control(token),
            Err((ClaimSlotError::Busy, _))
        ));
        drop(control);
        drop(callback);
        let _ = root.tick(true);
    }
}

#[cfg(test)]
mod cleanup_custody_tests {
    use super::*;
    use std::sync::{Barrier, atomic::AtomicUsize};
    fn setup() -> (Control, Callback, NativeStreamRoot) {
        let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 19, 4).unwrap();
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        (control, callback, root)
    }
    #[test]
    fn concurrent_loan_busy_refuses_original_paths_sticky_close_and_disposal_permit() {
        let (control, callback, root) = setup();
        let entered = Arc::new(Barrier::new(2));
        let finish = Arc::new(Barrier::new(2));
        let a = Arc::clone(&entered);
        let b = Arc::clone(&finish);
        root.inner.cleanup.lock().unwrap().before_work = Some(Box::new(move || {
            a.wait();
            b.wait();
        }));
        let worker = root.clone();
        let thread = std::thread::spawn(move || worker.tick(false));
        entered.wait();
        assert!(root.inner.ordinary.try_lock().is_ok());
        assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
        assert!(matches!(
            root.cleanup_permit(),
            Err(hex_stream::MediaCleanupPhase::Pending)
        ));
        assert!(root.take_report().is_none());
        assert!(root.input_bridge_storage_bytes().is_none());
        let (input, _file) = super::super::tests::facade_input(211);
        let pointer = input.paths.as_ptr();
        let capacity = input.paths.capacity();
        let path = input.paths[0].1.as_os_str().as_encoded_bytes().as_ptr();
        let path_capacity = input.paths[0].1.capacity();
        let refused = root.begin_preparation(input, 48_000).unwrap_err();
        assert_eq!(refused.input.paths.as_ptr(), pointer);
        assert_eq!(refused.input.paths.capacity(), capacity);
        assert_eq!(
            refused.input.paths[0]
                .1
                .as_os_str()
                .as_encoded_bytes()
                .as_ptr(),
            path
        );
        assert_eq!(refused.input.paths[0].1.capacity(), path_capacity);
        drop(refused);
        root.cancel_all_preparation();
        finish.wait();
        assert_eq!(
            thread.join().unwrap(),
            hex_stream::MediaCleanupPhase::Pending
        );
        root.inner.cleanup.lock().unwrap().before_work = None;
        assert!(root.lock().closing);
        assert!(root.lock().bridge.as_ref().unwrap().claim_union_closed);
        let permit = root.cleanup_permit().unwrap();
        assert!(root.inner.ordinary.try_lock().is_ok());
        assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
        drop(callback); // confirmed headless control disposal, ordinary lock remains available
        drop(permit);
        drop(control);
        root.tick(true);
        assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
    }
    #[test]
    fn first_borrowed_panic_keeps_capsule_payload_and_forbids_destructive_retry() {
        let (control, callback, root) = setup();
        let attempts = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&attempts);
        root.inner.cleanup.lock().unwrap().before_work = Some(Box::new(move || {
            seen.fetch_add(1, Relaxed);
            std::panic::panic_any(917_u64);
        }));
        assert_eq!(
            root.tick(false),
            hex_stream::MediaCleanupPhase::FailedIncomplete
        );
        assert_eq!(
            root.tick(true),
            hex_stream::MediaCleanupPhase::FailedIncomplete
        );
        assert_eq!(attempts.load(Relaxed), 1);
        assert!(root.lock().bridge.is_none());
        assert!(root.lock().busy_incarnation.is_some());
        let custody = root.inner.cleanup.lock().unwrap();
        assert!(custody.capsule.is_some());
        assert_eq!(
            custody.first_panic.as_ref().unwrap().downcast_ref::<u64>(),
            Some(&917)
        );
        drop(custody);
        assert!(matches!(
            root.cleanup_permit(),
            Err(hex_stream::MediaCleanupPhase::FailedIncomplete)
        ));
        assert!(root.take_report().is_none());
        assert_eq!(root.pool_reports(), Some([None; RETIRED]));
        // Explicit fixture teardown only: no production/quarantine completion asserted.
        drop(control);
        drop(callback);
        assert_eq!(
            root.phase(),
            Some(hex_stream::MediaCleanupPhase::FailedIncomplete)
        );
    }
    #[test]
    fn mismatch_keeps_capsule_and_never_overwrites_destination() {
        let (control, callback, root) = setup();
        let weak = Arc::downgrade(&root.inner);
        root.inner.cleanup.lock().unwrap().before_work = Some(Box::new(move || {
            weak.upgrade()
                .unwrap()
                .ordinary
                .lock()
                .unwrap()
                .busy_incarnation = Some(999);
        }));
        assert_eq!(
            root.tick(false),
            hex_stream::MediaCleanupPhase::FailedIncomplete
        );
        assert!(root.inner.cleanup.lock().unwrap().capsule.is_some());
        assert!(root.lock().bridge.is_none());
        assert_eq!(root.lock().busy_incarnation, Some(999));
        assert!(root.lock().faulted);
        drop(control);
        drop(callback);
    }
    #[test]
    fn actual_legacy_returned_graph_deallocation_runs_outside_ordinary_mutex() {
        let (mut control, mut callback, root) = setup();
        control
            .publish(Playback::new(
                hex_engine::Project::new(48_000, Vec::new()).unwrap(),
            ))
            .unwrap();
        callback.begin_block();
        control
            .publish(Playback::new(
                hex_engine::Project::new(48_000, Vec::new()).unwrap(),
            ))
            .unwrap();
        callback.begin_block();
        let weak = Arc::downgrade(&root.inner);
        let observed = Arc::new(AtomicBool::new(false));
        let result = Arc::clone(&observed);
        root.inner.cleanup.lock().unwrap().before_work = Some(Box::new(move || {
            result.store(weak.upgrade().unwrap().ordinary.try_lock().is_ok(), Relaxed);
        }));
        let info = allocation_counter::measure(|| {
            root.tick(false);
        });
        assert!(observed.load(Relaxed));
        assert!(info.count_current < 0 && info.bytes_current < 0); // actual returned Playback Box/storage freed on this control thread
        root.inner.cleanup.lock().unwrap().before_work = None;
        let mut positive = None;
        let positive_info = allocation_counter::measure(|| {
            positive = Some(vec![9_u8; 128]);
            std::hint::black_box(&positive);
        });
        assert!(positive_info.count_total > 0);
        drop(positive);
        drop(control);
        drop(callback);
        root.tick(true);
        assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
    }
}

#[cfg(test)]
mod terminal_disposal_negative_control {
    use super::*;
    #[test]
    fn actual_native_owner_retained_payload_drop_can_unwind_past_idle_permit() {
        struct Payload(Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Relaxed);
                panic!("injected retained native payload destructor panic");
            }
        }
        let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 27, 4).unwrap();
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        let (mut guard, errors, mut terminal) =
            crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe = Arc::clone(&drops);
        assert!(
            guard
                .owner_mut()
                .run(|_| std::panic::panic_any(Payload(probe)))
                .is_err()
        );
        drop(guard);
        drop(errors);
        let permit = root.cleanup_permit().unwrap();
        let owner = terminal.try_take().unwrap();
        // Matches the actual current owner disposal call. It is deliberately
        // OUTSIDE cleanup_transaction's borrowed capsule catch boundary.
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(owner)));
        assert!(failure.is_err());
        assert_eq!(drops.load(Relaxed), 1);
        assert!(!root.lock().faulted); // negative control: the capsule did not see it
        drop(permit);
        assert!(root.cleanup_permit().is_ok()); // no terminal-disposal tombstone exists
        drop(control);
        root.tick(true);
        // Only headless fixture cleanup; this does not establish safe production
        // Retirement/lease unwind or native device reopen.
    }
}
