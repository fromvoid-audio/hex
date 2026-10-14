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
    graph_teardown: Option<ReturnedGraphTeardown>,
    #[cfg(test)]
    before_work: Option<Box<dyn FnMut() + Send>>,
}
// Preallocated inline in the existing cleanup custodian before construction.
// Entire envelope is persistent before splitting; metadata never rides unwind.
struct ReturnedGraphTeardown {
    root_tag: Arc<()>,
    extraction_incarnation: u64,
    envelope: Option<GraphReturn>,
    owner: Option<hex_engine::storage_report::ControlPlaybackTeardown>,
    tracked_publication: Option<Publication>,
    tracked_owner: Option<hex_engine::storage_report::OwnerTrackedTeardown>,
    tracked_proof: Option<hex_engine::storage_report::OwnerGroupsDisposed>,
    association: Option<crate::publication_claims::ClaimAssociation>,
    stream_source: Option<StreamBinding>,
    cancellation: Option<Arc<FreshRuntimeCancellation>>,
    proof: Option<hex_engine::storage_report::RuntimeDisposed>,
    complete: bool,
    incomplete: bool,
}
impl ReturnedGraphTeardown {
    fn stage(returned: GraphReturn, root_tag: Arc<()>, extraction_incarnation: u64) -> Self {
        Self {
            root_tag,
            extraction_incarnation,
            envelope: Some(returned),
            owner: None,
            tracked_publication: None,
            tracked_owner: None,
            tracked_proof: None,
            association: None,
            stream_source: None,
            cancellation: None,
            proof: None,
            complete: false,
            incomplete: false,
        }
    }
    fn split(&mut self) -> bool {
        let Some(envelope) = self.envelope.take() else {
            return false;
        };
        match envelope {
            GraphReturn::Tracked(publication) => {
                self.tracked_publication = Some(publication); // persistent BEFORE splitting
                let publication = self
                    .tracked_publication
                    .as_mut()
                    .expect("staged tracked envelope");
                if let Some(owner) = publication.playback.tracked.take() {
                    self.tracked_owner = Some(owner.into_control_teardown());
                } else if let Some(owner) = publication.playback.tracked_retired.take() {
                    self.tracked_owner = Some(owner.into_control_teardown());
                } else {
                    self.incomplete = true;
                    return false;
                }
                true
            }
            GraphReturn::Claimed(ClaimedGraphReturn {
                owner,
                association,
                stream_source,
                cancellation,
            }) => {
                self.owner = Some(owner.into_control_teardown());
                self.association = Some(association);
                self.stream_source = stream_source;
                self.cancellation = cancellation;
                true
            }
            other => {
                self.envelope = Some(other);
                self.incomplete = true;
                false
            }
        }
    }
    fn preflight_tracked(
        publication: &Publication,
        slots: &crate::publication_claims::slots::ClaimUnionSlots,
    ) -> Result<(), crate::publication_claims::ClaimSlotError> {
        let claim = publication
            .tracked_claim
            .as_ref()
            .ok_or(crate::publication_claims::ClaimSlotError::PairMismatch)?;
        let identity = publication
            .playback
            .tracked_identity()
            .ok_or(crate::publication_claims::ClaimSlotError::PairMismatch)?;
        let anchors = slots.tracked_anchors(&claim.association)?;
        if claim.final_transfer.is_some() != publication.playback.manifest.is_some() {
            return Err(crate::publication_claims::ClaimSlotError::PairMismatch);
        }
        slots.preflight_tracked_return(crate::publication_claims::slots::TrackedReturnPreflight {
            association: &claim.association,
            identity,
            revision: publication.revision,
            context_generation: publication.control_context_generation,
            pair_matches: publication.playback.tracked_matches_anchors(anchors),
            installation: claim.installed.as_ref(),
            transfer: claim
                .final_transfer
                .as_ref()
                .zip(publication.playback.manifest.as_ref()),
        })
    }

    fn commit_tracked_facts(
        &mut self,
        slots: &mut crate::publication_claims::slots::ClaimUnionSlots,
    ) {
        if let Some(publication) = self.tracked_publication.as_mut()
            && let Some(claim) = publication.tracked_claim.as_mut()
        {
            let transfer = claim
                .final_transfer
                .take()
                .zip(publication.playback.manifest.take());
            slots.retain_tracked_return_facts(&claim.association, claim.installed.take(), transfer);
            slots.retain_tracked_return_metadata(
                &claim.association,
                publication.stream_source.take(),
                publication.fresh_runtime_cancellation.take(),
            );
        }
    }
    fn dispose(&mut self, slots: &mut crate::publication_claims::slots::ClaimUnionSlots) -> bool {
        if self.incomplete {
            return false;
        }
        if self.complete {
            return true;
        }
        if self.tracked_publication.is_some() {
            let (Some(publication), Some(owner)) = (
                self.tracked_publication.as_mut(),
                self.tracked_owner.as_mut(),
            ) else {
                self.incomplete = true;
                return false;
            };
            let Some(claim) = publication.tracked_claim.as_ref() else {
                self.incomplete = true;
                return false;
            };
            if self.tracked_proof.is_none() {
                match owner.dispose_on_control() {
                    Ok(proof) => self.tracked_proof = Some(proof),
                    Err(_) => {
                        self.incomplete = true;
                        slots.disposal_incomplete(&claim.association);
                        return false;
                    }
                }
            }
            match slots.ingest_tracked_disposal(
                &claim.association,
                self.tracked_proof
                    .as_ref()
                    .expect("successful actual disposal"),
            ) {
                Ok(()) => {
                    match slots
                        .retain_tracked_disposal_proof(&claim.association, &mut self.tracked_proof)
                    {
                        Ok(()) => {
                            self.complete = true;
                            return true;
                        }
                        Err(_) => {
                            self.incomplete = true;
                            slots.disposal_incomplete(&claim.association);
                            return false;
                        }
                    }
                }
                Err(_) => {
                    self.incomplete = true;
                    slots.disposal_incomplete(&claim.association);
                    return false;
                }
            }
        }
        let (Some(owner), Some(association)) = (self.owner.as_mut(), self.association.as_ref())
        else {
            self.incomplete = true;
            return false;
        };
        match owner.dispose_on_control() {
            Ok(proof) => self.proof = Some(proof),
            Err(_) => {
                self.incomplete = true;
                slots.disposal_incomplete(association);
                return false;
            }
        }
        let Some(proof) = self.proof.take() else {
            self.incomplete = true;
            return false;
        };
        match slots.disposed(association, proof) {
            Ok(()) => {
                self.complete = true;
                true
            }
            Err((_, proof)) => {
                self.proof = Some(proof);
                self.incomplete = true;
                slots.disposal_incomplete(association);
                false
            }
        }
    }
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
    pub(crate) fn stage_held_claimed_on_control(
        &mut self,
        data: &mut crate::native_callback_owner::NativeCallbackOwner,
    ) -> Result<(), hex_stream::MediaCleanupPhase> {
        // Existing held custody has precedence over the publication frontier.
        // Otherwise an old claimed owner can block the very branch that moves it.
        if data.held_claimed_on_control().is_some() {
            self.stage_old_held_claimed_on_control(data)?;
        }
        if !data.expose_terminal_tracked_on_control() {
            return Err(hex_stream::MediaCleanupPhase::Pending);
        }
        if let Some((publication, location)) = data.peek_tracked_on_control() {
            if self._custody.graph_teardown.is_some() {
                return Err(hex_stream::MediaCleanupPhase::Pending);
            }
            let mut root = self.root.lock();
            let Some(bridge) = root.bridge.as_mut() else {
                return Err(hex_stream::MediaCleanupPhase::Pending);
            };
            if !Arc::ptr_eq(&bridge.tag, &self.root.inner.tag) {
                return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
            }
            let Some(next) = self._custody.incarnation.checked_add(1) else {
                return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
            };
            let root_tag = Arc::clone(&bridge.tag); // control preflight only
            let slots = bridge
                .claim_union
                .as_mut()
                .ok_or(hex_stream::MediaCleanupPhase::Pending)?;
            ReturnedGraphTeardown::preflight_tracked(publication, slots)
                .map_err(|_| hex_stream::MediaCleanupPhase::Pending)?;
            if data.callback_fault_latched() {
                if let Some(claim) = publication.tracked_claim.as_ref() {
                    slots
                        .fault(&claim.association, true)
                        .map_err(|_| hex_stream::MediaCleanupPhase::FailedIncomplete)?;
                }
            }
            let Some(returned) = data.take_tracked_on_control(location) else {
                return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
            };
            self._custody.graph_teardown =
                Some(ReturnedGraphTeardown::stage(returned, root_tag, next));
            self._custody.incarnation = next;
            if let Some(teardown) = self._custody.graph_teardown.as_mut() {
                if !teardown.split() {
                    return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
                }
                teardown.commit_tracked_facts(slots);
            }
            return Err(hex_stream::MediaCleanupPhase::Pending); // persist capsule; retry terminal disposal only after control work
        }
        Ok(())
    }
    fn stage_old_held_claimed_on_control(
        &mut self,
        data: &mut crate::native_callback_owner::NativeCallbackOwner,
    ) -> Result<(), hex_stream::MediaCleanupPhase> {
        let Some((owner, a, stream_bound)) = data.held_claimed_on_control() else {
            return Ok(());
        };
        if self._custody.graph_teardown.is_some() || stream_bound {
            return Err(hex_stream::MediaCleanupPhase::Pending);
        }
        let mut root = self.root.lock();
        let Some(bridge) = root.bridge.as_mut() else {
            return Err(hex_stream::MediaCleanupPhase::Pending);
        };
        if !Arc::ptr_eq(&bridge.tag, &self.root.inner.tag) {
            return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
        }
        let Some(next) = self._custody.incarnation.checked_add(1) else {
            return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
        };
        let root_tag = Arc::clone(&bridge.tag); // control preflight anchor
        let slots = bridge
            .claim_union
            .as_mut()
            .ok_or(hex_stream::MediaCleanupPhase::Pending)?;
        let permit = slots
            .preflight_returned_disposal(a, owner)
            .map_err(|_| hex_stream::MediaCleanupPhase::Pending)?;
        let Some(returned) = data.take_held_claimed_on_control() else {
            return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
        };
        self._custody.graph_teardown = Some(ReturnedGraphTeardown::stage(
            returned.into(),
            root_tag,
            next,
        ));
        self._custody.incarnation = next;
        if let Some(teardown) = self._custody.graph_teardown.as_mut() {
            if !teardown.split() {
                return Err(hex_stream::MediaCleanupPhase::FailedIncomplete);
            }
        }
        permit.commit(); // all owning destinations validated before the move
        Ok(()) // no graph destructor while ordinary state is locked
    }
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
            let CleanupCustody {
                capsule,
                graph_teardown,
                incarnation,
                ..
            } = &mut *custody;
            match capsule.as_mut() {
                Some(capsule) => capsule.run(
                    closing,
                    final_retirement,
                    report_only,
                    graph_teardown,
                    *incarnation,
                ),
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
        graph_teardown: &mut Option<ReturnedGraphTeardown>,
        extraction_incarnation: u64,
    ) -> (
        hex_stream::MediaCleanupPhase,
        Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>,
    ) {
        let mut report = None;
        if !report_only {
            if let Some(teardown) = graph_teardown.as_mut() {
                if !Arc::ptr_eq(&teardown.root_tag, &self.bridge.tag)
                    || teardown.extraction_incarnation > extraction_incarnation
                {
                    teardown.incomplete = true;
                    return (hex_stream::MediaCleanupPhase::FailedIncomplete, None);
                }
                let Some(slots) = self.bridge.claim_union.as_mut() else {
                    return (hex_stream::MediaCleanupPhase::FailedIncomplete, None);
                };
                if !teardown.dispose(slots) {
                    return (hex_stream::MediaCleanupPhase::FailedIncomplete, None);
                }
            }
            if graph_teardown
                .as_ref()
                .is_some_and(|teardown| teardown.complete && teardown.tracked_publication.is_some())
            {
                drop(graph_teardown.take());
            }
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
                if let Ok(GraphReturn::Tracked(publication)) = returns.peek() {
                    if graph_teardown.is_some() {
                        return (hex_stream::MediaCleanupPhase::Pending, None);
                    }
                    let Some(slots) = self.bridge.claim_union.as_mut() else {
                        return (hex_stream::MediaCleanupPhase::Pending, None);
                    };
                    if ReturnedGraphTeardown::preflight_tracked(publication, slots).is_err() {
                        return (hex_stream::MediaCleanupPhase::Pending, None);
                    }
                    let root_tag = Arc::clone(&self.bridge.tag); // control preflight only
                    let Ok(returned) = returns.pop() else {
                        return (hex_stream::MediaCleanupPhase::Pending, None);
                    };
                    *graph_teardown = Some(ReturnedGraphTeardown::stage(
                        returned,
                        root_tag,
                        extraction_incarnation,
                    ));
                    if let Some(teardown) = graph_teardown.as_mut() {
                        if !teardown.split() {
                            return (hex_stream::MediaCleanupPhase::FailedIncomplete, None);
                        }
                        teardown.commit_tracked_facts(slots); // fixed moves; same borrowed preflight
                        if !teardown.dispose(slots) {
                            return (hex_stream::MediaCleanupPhase::FailedIncomplete, None);
                        }
                    }
                }
                if let Ok(GraphReturn::Claimed(returned)) = returns.peek() {
                    // Stream-domain association transfer is not enabled. Keep
                    // any stream-bound claimed envelope intact until that exact
                    // catalog/domain proof is available; never infer it here.
                    if returned.stream_source.is_some() {
                        return (hex_stream::MediaCleanupPhase::Pending, None);
                    }
                    if graph_teardown.is_some() {
                        return (hex_stream::MediaCleanupPhase::Pending, None);
                    }
                    let Some(slots) = self.bridge.claim_union.as_mut() else {
                        return (hex_stream::MediaCleanupPhase::Pending, None);
                    };
                    // Actual root-slot-runtime pairing AND vacant destination
                    // before pop. Sole consumer cannot change the peeked item.
                    let permit = match slots
                        .preflight_returned_disposal(&returned.association, &returned.owner)
                    {
                        Ok(permit) => permit,
                        Err(_) => return (hex_stream::MediaCleanupPhase::Pending, None),
                    };
                    let root_tag = Arc::clone(&self.bridge.tag); // control preflight anchor
                    let Ok(returned) = returns.pop() else {
                        return (hex_stream::MediaCleanupPhase::Pending, None);
                    };
                    *graph_teardown = Some(ReturnedGraphTeardown::stage(
                        returned,
                        root_tag,
                        extraction_incarnation,
                    ));
                    if let Some(teardown) = graph_teardown.as_mut() {
                        if !teardown.split() {
                            return (hex_stream::MediaCleanupPhase::FailedIncomplete, None);
                        }
                        permit.commit(); // borrowed validated slot, infallible
                        if !Arc::ptr_eq(&teardown.root_tag, &self.bridge.tag)
                            || teardown.extraction_incarnation != extraction_incarnation
                        {
                            teardown.incomplete = true;
                            return (hex_stream::MediaCleanupPhase::FailedIncomplete, None);
                        }
                        if !teardown.dispose(slots) {
                            return (hex_stream::MediaCleanupPhase::FailedIncomplete, None);
                        }
                    }
                }
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

#[cfg(all(test, feature = "allocation-probe"))]
mod claimed_disposal_tests {
    use super::*;
    use crate::publication_claims::PublicationAnchors;
    use hex_engine::storage_report::{ClaimReservation, incomplete_owned_pair_for_test};
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering::Relaxed},
        },
        time::Duration,
    };

    fn prepared() -> hex_engine::OwnedPreparedRevision {
        hex_engine::PreparedRevision::prepare(
            &hex_project::Project::new(48_000).unwrap(),
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared()
    }
    fn setup() -> (
        Control,
        Callback,
        NativeStreamRoot,
        hex_stream::StreamControl,
        hex_stream::StreamCallbackPages,
    ) {
        let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 91, 4).unwrap();
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        root.lock()
            .bridge
            .as_mut()
            .unwrap()
            .reserve_claim_storage_on_control(ClaimReservation {
                items: 8,
                anchors: 32,
                max_prepared_data_bytes: usize::MAX,
                max_construction_data_bytes: usize::MAX,
                other_retained_bytes: 0,
                other_construction_bytes: 0,
            })
            .unwrap();
        let mut runtime =
            hex_stream::StreamRuntime::new(hex_stream::StreamConfig::with_pool_pages(4), vec![])
                .unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        let (stream, pages) = runtime.split().ok().unwrap();
        (control, callback, root, stream, pages)
    }
    fn returned(
        root: &NativeStreamRoot,
        stream: &hex_stream::StreamControl,
        revision: hex_engine::OwnedPreparedRevision,
    ) -> ClaimedGraphReturn {
        let verifier_root = revision.clone_on_control();
        let (owner, anchors) = incomplete_owned_pair_for_test(revision).ok().unwrap();
        assert!(owner.is_incomplete_fixture());
        let verified = hex_stream::VerifiedOwnedStreamRevision::new(
            verifier_root,
            stream.catalog_lease().unwrap(),
        )
        .unwrap();
        let a = root
            .lock()
            .bridge
            .as_mut()
            .unwrap()
            .claim_union
            .as_mut()
            .unwrap()
            .install_incomplete_pair_for_test(PublicationAnchors {
                claims: anchors,
                verified,
            });
        ClaimedGraphReturn {
            owner,
            association: a,
            stream_source: None,
            cancellation: Some(Arc::new(FreshRuntimeCancellation {
                revision: 901,
                decision: AtomicU32::new(FRESH_PUBLICATION_PENDING),
            })),
        }
    }
    fn tracked_publication(
        root: &NativeStreamRoot,
        stream: &hex_stream::StreamControl,
        revision: hex_engine::OwnedPreparedRevision,
        number: u64,
    ) -> Publication {
        let verifier_root = revision.clone_on_control();
        let (mut owner, anchors) = incomplete_owned_pair_for_test(revision).ok().unwrap();
        owner.prepare_owner_groups_for_test().unwrap();
        let reservation = ClaimReservation {
            items: 32_768,
            anchors: 16,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            other_retained_bytes: 0,
            other_construction_bytes: 0,
        };
        let (owner, archive, report) = owner
            .try_acquire_owner_groups_on_control(&anchors, reservation)
            .ok()
            .unwrap();
        assert!(report.incomplete_source);
        let verified = hex_stream::VerifiedOwnedStreamRevision::new(
            verifier_root,
            stream.catalog_lease().unwrap(),
        )
        .unwrap();
        let association = root
            .lock()
            .bridge
            .as_mut()
            .unwrap()
            .claim_union
            .as_mut()
            .unwrap()
            .install_tracked_incomplete_for_test(
                &owner,
                archive,
                PublicationAnchors {
                    claims: anchors,
                    verified,
                },
            );
        Publication {
            stream_source: None,
            revision: number,
            control_context_generation: 0,
            policy: PublicationPolicy::InheritRuntime,
            fresh_runtime_cancellation: None,
            playback: PublicationPlayback::from_tracked(owner),
            tracked_claim: Some(TrackedPublicationClaims {
                association,
                installed: None,
                final_transfer: None,
            }),
        }
    }
    #[test]
    fn tracked_queue_acceptance_is_not_installation_and_pressure_keeps_owner() {
        let (mut control, mut callback, root, stream, pages) = setup();
        let publication = tracked_publication(&root, &stream, prepared(), 901);
        assert!(
            publication
                .tracked_claim
                .as_ref()
                .unwrap()
                .installed
                .is_none()
        );
        control.publications.push(publication).ok().unwrap();
        callback.begin_block();
        let active = callback.active.as_ref().unwrap();
        assert!(
            active
                .tracked_claim
                .as_ref()
                .unwrap()
                .installed
                .as_ref()
                .unwrap()
                .matches(
                    &active.tracked_claim.as_ref().unwrap().association,
                    active.playback.tracked_identity().unwrap(),
                    901,
                    0
                )
        );
        assert!(
            active
                .tracked_claim
                .as_ref()
                .unwrap()
                .final_transfer
                .is_none()
        );
        // No final host buffer was rendered/committed by begin_block.
        let queued = tracked_publication(&root, &stream, prepared(), 902);
        control.publications.push(queued).ok().unwrap();
        // An occupied final-fact destination must refuse before queue pop.
        callback
            .active
            .as_mut()
            .unwrap()
            .tracked_claim
            .as_mut()
            .unwrap()
            .installed = None;
        callback.begin_block();
        assert!(callback.publications.peek().is_ok());
        assert_eq!(callback.active.as_ref().unwrap().revision, 901);
        drop(callback);
        drop(control);
        drop(root);
        assert!(stream.shutdown(pages).unwrap().accounting_complete);
    }
    #[test]
    fn actual_tracked_install_handoff_return_and_disposal_keep_claims_incomplete() {
        let (mut control, mut callback, root, stream, pages) = setup();
        let revision = prepared();
        control
            .publications
            .push(tracked_publication(
                &root,
                &stream,
                revision.clone_on_control(),
                901,
            ))
            .ok()
            .unwrap();
        callback.begin_block();
        control
            .publications
            .push(tracked_publication(&root, &stream, revision, 902))
            .ok()
            .unwrap();
        callback.begin_block();
        assert_eq!(callback.active.as_ref().unwrap().revision, 902);
        {
            let mut state = root.lock();
            let bridge = state.bridge.as_mut().unwrap();
            let returned = bridge.native_retired.as_mut().unwrap().peek().unwrap();
            let GraphReturn::Tracked(publication) = returned else {
                panic!("sealed return");
            };
            let claim = publication.tracked_claim.as_ref().unwrap();
            assert!(claim.installed.is_some());
            assert!(claim.final_transfer.as_ref().unwrap().matches(
                &claim.association,
                publication.playback.manifest.as_ref().unwrap()
            ));
            assert!(publication.playback.tracked.is_none());
            assert!(publication.playback.tracked_retired.is_some());
        }
        assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
        // Actual owner disposal and archive ingestion never upgrade fixture
        // reports or establish role/catalog/aggregate admission authority.
        assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
        let refusal = stream.shutdown(pages).err().unwrap();
        assert!(matches!(refusal, hex_stream::ShutdownError::Busy(_)));
        let (stream, pages) = refusal.into_parts().unwrap();
        drop(callback);
        drop(control);
        drop(root);
        assert!(stream.shutdown(pages).unwrap().accounting_complete);
    }
    #[test]
    fn actual_retirement_execution_extracts_every_tracked_location_before_callback_drop() {
        for location in 0..5 {
            let (mut control, mut callback, root, stream, pages) = setup();
            // A larger fixture-only queue proves a legacy head is not negative
            // evidence. Ordinary publication capacity remains unchanged.
            let (producer, consumer) = RingBuffer::new(3);
            control.publications = producer;
            callback.publications = consumer;
            let target = tracked_publication(&root, &stream, prepared(), 910);
            match location {
                0 => callback.active = Some(target),
                1 => callback.deferred = Some(target),
                2 => {
                    control.publications.push(target).ok().unwrap();
                }
                3 => callback.held_retirement = Some(target.into_return()),
                _ => {
                    control
                        .publications
                        .push(Publication {
                            stream_source: None,
                            revision: 909,
                            control_context_generation: 0,
                            policy: PublicationPolicy::InstallFreshRuntime,
                            fresh_runtime_cancellation: None,
                            tracked_claim: None,
                            playback: Box::new(Playback::new(
                                hex_engine::Project::new(48_000, Vec::new()).unwrap(),
                            ))
                            .into(),
                        })
                        .ok()
                        .unwrap();
                    control.publications.push(target).ok().unwrap();
                }
            }
            let companion = tracked_publication(&root, &stream, prepared(), 911);
            let companion_identity = companion.playback.tracked_identity().unwrap();
            control.publications.push(companion).ok().unwrap();
            crate::lifecycle::tracked_retirement_execution_for_test(
                callback,
                Arc::clone(&control.status),
                &root,
                |data, _| {
                    // First extraction stages target. Occupied capsule refuses
                    // the companion without owner loss or callback destruction.
                    let (remaining, _) = data.peek_tracked_on_control().unwrap();
                    assert_eq!(
                        remaining.playback.tracked_identity(),
                        Some(companion_identity)
                    );
                },
                |_| {},
            );
            {
                let custody = root.inner.cleanup.lock().unwrap();
                let teardown = custody.graph_teardown.as_ref().unwrap();
                assert!(!teardown.complete);
                assert!(teardown.tracked_owner.is_some());
                assert!(teardown.tracked_proof.is_none());
            }
            assert_eq!(
                root.tick(false),
                hex_stream::MediaCleanupPhase::FailedIncomplete
            );
            // Actual latched panic prevents archive closure, not just release.
            {
                let custody = root.inner.cleanup.lock().unwrap();
                assert!(custody.graph_teardown.as_ref().unwrap().incomplete);
            }
            drain_fixture(&root);
            drop(control);
            drop(root);
            assert!(stream.shutdown(pages).unwrap().accounting_complete);
        }
    }
    #[test]
    fn actual_retirement_execution_progresses_held_frontiers_after_return_pressure() {
        for old_claimed in [false, true] {
            let (mut control, mut callback, root, stream, pages) = setup();
            let legacy = |number| Publication {
                stream_source: None,
                revision: number,
                control_context_generation: 0,
                policy: PublicationPolicy::InstallFreshRuntime,
                fresh_runtime_cancellation: None,
                tracked_claim: None,
                playback: Box::new(Playback::new(
                    hex_engine::Project::new(48_000, Vec::new()).unwrap(),
                ))
                .into(),
            };
            if old_claimed {
                callback.held_retirement = Some(returned(&root, &stream, prepared()).into());
            } else {
                for number in 1..=RETIRED {
                    callback
                        .retired
                        .push(legacy(number as u64).into_return())
                        .ok()
                        .unwrap();
                }
                callback.held_retirement = Some(legacy(900).into_return());
            }
            let queued = tracked_publication(&root, &stream, prepared(), 920);
            let queued_identity = queued.playback.tracked_identity();
            control.publications.push(queued).ok().unwrap();
            crate::lifecycle::tracked_retirement_execution_for_test(
                callback,
                Arc::clone(&control.status),
                &root,
                |data, step| {
                    if old_claimed {
                        // Existing claimed custody is staged first despite a
                        // queued tracked owner. Capsule pressure keeps queue intact.
                        assert!(data.held_claimed_on_control().is_none());
                        assert_eq!(
                            data.peek_tracked_on_control()
                                .unwrap()
                                .0
                                .playback
                                .tracked_identity(),
                            queued_identity
                        );
                    } else if step == 0 {
                        // Full normal ring: exact queued owner remains untouched.
                        assert_eq!(
                            data.peek_tracked_on_control()
                                .unwrap()
                                .0
                                .playback
                                .tracked_identity(),
                            queued_identity
                        );
                    } else {
                        // After capacity returns, legacy held owner is handed to
                        // normal returns and queued tracked custody is staged.
                        assert!(data.peek_tracked_on_control().is_none());
                    }
                },
                |step| {
                    if step == 0 && !old_claimed {
                        drain_fixture(&root);
                    }
                },
            );
            {
                let custody = root.inner.cleanup.lock().unwrap();
                let teardown = custody.graph_teardown.as_ref().unwrap();
                if old_claimed {
                    assert!(teardown.owner.is_some());
                    assert!(teardown.tracked_owner.is_none());
                } else {
                    assert!(teardown.tracked_owner.is_some());
                }
                assert!(!teardown.complete);
            }
            drain_fixture(&root);
            drop(control);
            drop(root);
            assert!(stream.shutdown(pages).unwrap().accounting_complete);
        }
    }
    fn drain_fixture(root: &NativeStreamRoot) {
        loop {
            let popped = root
                .lock()
                .bridge
                .as_mut()
                .and_then(|b| b.native_retired.as_mut())
                .and_then(|q| q.pop().ok());
            let Some(owner) = popped else { break };
            drop(owner); // explicit fixture cleanup, outside ordinary mutex
        }
    }
    #[test]
    fn actual_box_queue_disposal_proof_stays_incomplete_and_consumes_only_once() {
        let (control, mut callback, root, stream, pages) = setup();
        let mut returned = returned(&root, &stream, prepared());
        let identity = returned.owner.identity();
        let cancellation = Arc::clone(returned.cancellation.as_ref().unwrap());
        let cancellation_count = Arc::strong_count(&cancellation);
        let drops = Arc::new(Mutex::new(Vec::new()));
        let probe = Arc::clone(&drops);
        let weak = Arc::downgrade(&root.inner);
        returned.owner.observe_disposal_for_test(move |address| {
            let root = weak.upgrade().unwrap();
            assert!(root.ordinary.try_lock().is_ok());
            assert!(root.cleanup.try_lock().is_err()); // borrowed persistent custodian
            probe
                .lock()
                .unwrap()
                .push((address, std::thread::current().id()));
        });
        callback.return_owner(returned.into());
        assert!(callback.held_retirement.is_none());
        assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
        let control_thread = std::thread::current().id();
        assert_eq!(drops.lock().unwrap().len(), 1);
        assert_eq!(drops.lock().unwrap()[0].1, control_thread);
        {
            let mut custody = root.inner.cleanup.lock().unwrap();
            let teardown = custody.graph_teardown.as_mut().unwrap();
            assert!(teardown.complete);
            assert!(!teardown.incomplete);
            assert!(Arc::ptr_eq(
                teardown.cancellation.as_ref().unwrap(),
                &cancellation
            ));
            assert_eq!(Arc::strong_count(&cancellation), cancellation_count);
            assert_eq!(teardown.owner.as_ref().unwrap().identity(), identity);
            assert!(matches!(
                teardown.owner.as_mut().unwrap().dispose_on_control(),
                Err(hex_engine::storage_report::ControlTeardownError::Consumed)
            ));
            let a = teardown.association.as_ref().unwrap();
            let state = root.lock();
            let slots = state.bridge.as_ref().unwrap().claim_union.as_ref().unwrap();
            assert_eq!(
                slots.disposal_fixture_snapshot(a),
                (true, true, false, true)
            );
        }
        root.tick(false);
        assert_eq!(drops.lock().unwrap().len(), 1);
        let refusal = stream.shutdown(pages).err().unwrap();
        assert!(matches!(refusal, hex_stream::ShutdownError::Busy(_)));
        let (stream, pages) = refusal.into_parts().unwrap(); // anchors still own actual lease
        drop(callback);
        drop(control);
        drop(root);
        let report = stream.shutdown(pages).unwrap();
        assert_eq!(report.page_buffers, 4);
        assert!(report.worker_joined && report.accounting_complete);
    }
    #[test]
    fn actual_box_destructor_panic_retains_anchors_payload_and_never_retries() {
        let (control, mut callback, root, stream, pages) = setup();
        let mut returned = returned(&root, &stream, prepared());
        let drops = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&drops);
        let weak = Arc::downgrade(&root.inner);
        returned.owner.observe_disposal_for_test(move |_| {
            assert!(weak.upgrade().unwrap().ordinary.try_lock().is_ok());
            observed.fetch_add(1, Relaxed);
            panic!("actual sealed-owner disposal probe");
        });
        callback.return_owner(returned.into());
        assert_eq!(
            root.tick(false),
            hex_stream::MediaCleanupPhase::FailedIncomplete
        );
        assert_eq!(drops.load(Relaxed), 1);
        {
            let mut custody = root.inner.cleanup.lock().unwrap();
            let CleanupCustody {
                graph_teardown,
                capsule,
                ..
            } = &mut *custody;
            let teardown = graph_teardown.as_mut().unwrap();
            assert!(teardown.incomplete);
            assert!(matches!(
                teardown.owner.as_mut().unwrap().dispose_on_control(),
                Err(hex_engine::storage_report::ControlTeardownError::Incomplete)
            ));
            let a = teardown.association.as_ref().unwrap();
            let capsule = capsule.as_ref().unwrap();
            assert_eq!(
                capsule
                    .bridge
                    .claim_union
                    .as_ref()
                    .unwrap()
                    .disposal_fixture_snapshot(a),
                (false, true, false, false)
            );
        }
        assert_eq!(
            root.tick(true),
            hex_stream::MediaCleanupPhase::FailedIncomplete
        );
        assert_eq!(drops.load(Relaxed), 1);
        drop(callback);
        drop(control);
        drop(root);
        let report = stream.shutdown(pages).unwrap(); // fixture disposal, not production recovery
        assert_eq!(report.page_buffers, 4);
        assert!(report.worker_joined && report.accounting_complete);
    }
    #[test]
    fn twin_runtime_mismatch_never_pops_actual_owner_and_pressure_preserves_held_return() {
        let (control, mut callback, root, stream, pages) = setup();
        let revision = prepared();
        let twin = revision.clone_on_control();
        let mut wrong = returned(&root, &stream, revision);
        let (other_owner, other_anchors) = incomplete_owned_pair_for_test(twin).ok().unwrap();
        assert_ne!(wrong.owner.identity(), other_owner.identity());
        let original = std::mem::replace(&mut wrong.owner, other_owner);
        let identity = wrong.owner.identity();
        callback.return_owner(wrong.into());
        assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
        assert!(root.inner.cleanup.lock().unwrap().graph_teardown.is_none());
        {
            let state = root.lock();
            let q = state
                .bridge
                .as_ref()
                .unwrap()
                .native_retired
                .as_ref()
                .unwrap();
            assert!(
                matches!(q.peek(), Ok(GraphReturn::Claimed(r)) if r.owner.identity() == identity)
            );
        }
        drain_fixture(&root);
        drop(original);
        drop(other_anchors);
        // Three actual sealed owners through the existing capacity-two ring and
        // held return. No synthetic receipt or increased return/domain capacity.
        for _ in 0..3 {
            let r = returned(&root, &stream, prepared());
            let measure = allocation_counter::measure(|| callback.return_owner(r.into()));
            assert_eq!(
                (
                    measure.count_total,
                    measure.bytes_total,
                    measure.count_current,
                    measure.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        let positive = allocation_counter::measure(|| {
            std::hint::black_box(Vec::<u8>::with_capacity(4096));
        });
        assert!(positive.count_total > 0 && positive.bytes_total >= 4096);
        let held = match callback.held_retirement.as_ref().unwrap() {
            GraphReturn::Claimed(r) => r.owner.identity(),
            _ => panic!("typed held return"),
        };
        root.tick(false); // first owner occupies the single persistent capsule
        callback.begin_block(); // return held owner into the newly available cell
        assert!(callback.held_retirement.is_none());
        root.tick(false); // occupied capsule: no next claimed pop
        {
            let state = root.lock();
            let q = state
                .bridge
                .as_ref()
                .unwrap()
                .native_retired
                .as_ref()
                .unwrap();
            assert_eq!(q.slots(), 2);
        }
        // Actual held owner survives; inspect after moving queue entries outside lock.
        let first = root
            .lock()
            .bridge
            .as_mut()
            .unwrap()
            .native_retired
            .as_mut()
            .unwrap()
            .pop()
            .unwrap();
        let second = root
            .lock()
            .bridge
            .as_mut()
            .unwrap()
            .native_retired
            .as_mut()
            .unwrap()
            .pop()
            .unwrap();
        assert!(matches!(&second, GraphReturn::Claimed(r) if r.owner.identity() == held));
        drop(first);
        drop(second);
        assert!(
            root.lock()
                .bridge
                .as_ref()
                .unwrap()
                .claim_union
                .as_ref()
                .unwrap()
                .phase()
                != hex_stream::MediaCleanupPhase::Complete
        );
        // No completed role or graph release was synthesized by fixture acceptance.
        assert_ne!(root.tick(true), hex_stream::MediaCleanupPhase::Complete);
        drop(callback);
        drop(control);
        drop(root);
        stream.shutdown(pages).unwrap();
    }
    #[test]
    fn actual_disposal_proof_cannot_cross_roots_or_substitute_for_returned_custody() {
        let (control, callback, root, stream, pages) = setup();
        let (other_control, other_callback, other_root, other_stream, other_pages) = setup();
        let foreign = returned(&other_root, &other_stream, prepared());
        let target = returned(&root, &stream, prepared());
        let foreign_identity = foreign.owner.identity();
        let mut capsule = foreign.owner.into_control_teardown();
        let proof = capsule.dispose_on_control().unwrap();
        assert!(proof.is_incomplete_fixture());
        let (error, proof) = root
            .lock()
            .bridge
            .as_mut()
            .unwrap()
            .claim_union
            .as_mut()
            .unwrap()
            .disposed(&foreign.association, proof)
            .err()
            .unwrap();
        assert_eq!(error, crate::publication_claims::ClaimSlotError::Stale);
        assert_eq!(proof.identity(), foreign_identity);
        let (error, proof) = other_root
            .lock()
            .bridge
            .as_mut()
            .unwrap()
            .claim_union
            .as_mut()
            .unwrap()
            .disposed(&foreign.association, proof)
            .err()
            .unwrap();
        assert_eq!(error, crate::publication_claims::ClaimSlotError::Busy);
        assert_eq!(proof.identity(), foreign_identity); // original fact recovered
        assert_eq!(
            root.lock()
                .bridge
                .as_ref()
                .unwrap()
                .claim_union
                .as_ref()
                .unwrap()
                .disposal_fixture_snapshot(&target.association),
            (false, true, false, false)
        );
        assert_eq!(
            other_root
                .lock()
                .bridge
                .as_ref()
                .unwrap()
                .claim_union
                .as_ref()
                .unwrap()
                .disposal_fixture_snapshot(&foreign.association),
            (false, true, false, false)
        );
        let _unused_fact = proof; // scalar fact cannot release any owner/charge
        drop(capsule);
        drop(target);
        drop(callback);
        drop(control);
        drop(root);
        let report = stream.shutdown(pages).unwrap();
        assert!(report.accounting_complete && report.worker_joined);
        drop(other_callback);
        drop(other_control);
        drop(other_root);
        let report = other_stream.shutdown(other_pages).unwrap();
        assert!(report.accounting_complete && report.worker_joined);
    }
    #[test]
    fn actual_native_terminal_stages_held_claim_before_callback_destruction() {
        let (control, mut callback, root, stream, pages) = setup();
        let mut r = returned(&root, &stream, prepared());
        let drops = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&drops);
        let weak = Arc::downgrade(&root.inner);
        r.owner.observe_disposal_for_test(move |_| {
            assert!(weak.upgrade().unwrap().ordinary.try_lock().is_ok());
            count.fetch_add(1, Relaxed);
        });
        callback.held_retirement = Some(r.into()); // exact persistent held owner
        let (data, errors, mut terminals) =
            crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
        std::thread::spawn(move || {
            drop(data);
            drop(errors);
        })
        .join()
        .unwrap();
        let (mut data, errors) = terminals.try_take().unwrap();
        let mut permit = root.cleanup_permit().unwrap();
        permit.stage_held_claimed_on_control(&mut data).unwrap();
        assert!(data.held_claimed_on_control().is_none());
        assert_eq!(drops.load(Relaxed), 0);
        drop(data);
        drop(errors); // actual boxed callback; graph stays in capsule
        assert_eq!(drops.load(Relaxed), 0);
        drop(permit);
        assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
        assert_eq!(drops.load(Relaxed), 1);
        let custody = root.inner.cleanup.lock().unwrap();
        assert!(custody.graph_teardown.as_ref().unwrap().complete);
        drop(custody);
        drop(control);
        drop(root);
        let report = stream.shutdown(pages).unwrap();
        assert!(report.accounting_complete && report.worker_joined);
    }
    #[test]
    fn occupied_capsule_refuses_terminal_transfer_without_losing_held_actual_owner() {
        let (control, mut callback, root, stream, pages) = setup();
        callback.return_owner(returned(&root, &stream, prepared()).into());
        root.tick(false); // actual first owner disposed; anchors/capsule occupied
        let r = returned(&root, &stream, prepared());
        let identity = r.owner.identity();
        callback.held_retirement = Some(r.into());
        let (data, errors, mut terminals) =
            crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
        drop(data);
        drop(errors);
        let (mut data, errors) = terminals.try_take().unwrap();
        let mut permit = root.cleanup_permit().unwrap();
        assert_eq!(
            permit.stage_held_claimed_on_control(&mut data),
            Err(hex_stream::MediaCleanupPhase::Pending)
        );
        assert_eq!(
            data.held_claimed_on_control().unwrap().0.identity(),
            identity
        );
        assert_eq!(
            permit.stage_held_claimed_on_control(&mut data),
            Err(hex_stream::MediaCleanupPhase::Pending)
        );
        assert_eq!(
            data.held_claimed_on_control().unwrap().0.identity(),
            identity
        );
        // Explicit fixture recovery, not production disposal/role completion.
        let held = data.take_held_claimed_on_control().unwrap();
        drop(permit);
        drop(held);
        drop(data);
        drop(errors);
        drop(control);
        drop(root);
        let report = stream.shutdown(pages).unwrap();
        assert!(report.accounting_complete && report.worker_joined);
    }
}
