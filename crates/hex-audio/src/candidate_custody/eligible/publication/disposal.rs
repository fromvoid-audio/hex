//! Persistent control-side disposal; no ordinary bank lock spans destruction.
//!
//! Installation permission may be revoked without revoking the original physical return owner.
//! Exact root/runtime/incarnation checks precede closure. Runtime and remaining-origin
//! destruction, preparation cleanup, catalog release, stream join and page conservation must
//! actually finish before the original bank and disposal fact become recoverable. Busy leases
//! retain occupancy; irreversible failure keeps the first payload and remaining owners without
//! retry or scalar completion.
use super::*;
impl Claims {
    pub(super) fn matches(
        &self,
        root: &CandidateRoot,
        runtime: &ClaimedAudioCandidateRuntime,
    ) -> bool {
        root.eligible_claims_open() && self.matches_custody(root, runtime)
    }
    // Revoking installation never revokes the original physical return owner.
    fn matches_custody(
        &self,
        root: &CandidateRoot,
        runtime: &ClaimedAudioCandidateRuntime,
    ) -> bool {
        self.root.same_root(root)
            && self.resources.covers(self.stamp.root)
            && self.stamp.owner == runtime.identity()
            && self.life.live.load(Ordering::Acquire)
            && self.life.incarnation.load(Ordering::Relaxed) == self.stamp.incarnation
    }
}
impl Custody {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(in crate::candidate_custody) fn test_return_metadata_dropped(
        &self,
        ticket: &AudioCandidateTicket,
    ) -> bool {
        let Ok(work) = self.work.try_lock() else {
            return false;
        };
        work.publication
            .executions
            .get(ticket.stamp.slot)
            .and_then(Option::as_ref)
            .is_some_and(|execution| {
                execution.returned.stamp == ticket.stamp
                    && execution.proof_recorded
                    && execution.returned.state.is_none()
                    && !execution.failed
                    && !execution.pending
            })
    }

    pub(in crate::candidate_custody) fn cancel_publication(
        &self,
        ticket: &AudioCandidateTicket,
    ) -> AudioCandidateCancellationResult {
        let Ok(bank) = self.bank.try_lock() else {
            return AudioCandidateCancellationResult::Unavailable;
        };
        let stamp = ticket.stamp;
        let Some(slot) = bank.publication.slots.get(stamp.slot) else {
            return AudioCandidateCancellationResult::Unavailable;
        };
        if bank.root != stamp.root
            || slot.incarnation != stamp.incarnation
            || slot.identity != Some(stamp.owner)
            || slot.phase == Phase::Vacant
        {
            return AudioCandidateCancellationResult::Unavailable;
        }
        match slot.life.cancellation.compare_exchange(
            PENDING,
            CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(CANCELLED) => AudioCandidateCancellationResult::Cancelled,
            Err(APPLYING) => AudioCandidateCancellationResult::Applying,
            _ => AudioCandidateCancellationResult::Complete,
        }
    }

    pub(in crate::candidate_custody) fn ingest_publication(
        &self,
        root: &CandidateRoot,
        returned: Returned,
    ) -> Result<(), Returned> {
        let stamp = returned.stamp;
        let mut bank = lock(&self.bank);
        if stamp.root != bank.root {
            return Err(returned);
        }
        let Some(slot) = bank.publication.slots.get_mut(stamp.slot) else {
            return Err(returned);
        };
        let valid = returned.state.as_ref().is_some_and(|state| {
            state.stamp == stamp
                && state.domain_refs == slot.domain_refs
                && state
                    .streaming
                    .as_ref()
                    .is_none_or(streaming::Workspace::valid)
                && state.context_generation == slot.context_generation
                && Some(state.policy) == slot.policy
                && state.runtime.as_ref().is_some_and(|runtime| {
                    runtime.identity() == stamp.owner
                        && state
                            .claims
                            .as_ref()
                            .is_none_or(|c| c.matches_custody(root, runtime))
                })
                && state.revision == slot.life.revision.load(Ordering::Acquire)
                && state.retired_runtime.is_none()
                && match returned.kind {
                    AudioCandidateReturnKind::NeverInstalled
                    | AudioCandidateReturnKind::FinalActive => {
                        state.manifest.is_none() && state.covering_successor.is_none()
                    }
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    AudioCandidateReturnKind::StoppedForNativeEpoch => {
                        state.manifest.is_none()
                            && state.covering_successor.is_none()
                            && state
                                .runtime
                                .as_ref()
                                .and_then(|r| r.status())
                                .is_some_and(|s| !s.playing)
                    }
                    AudioCandidateReturnKind::Replaced => state
                        .manifest
                        .as_ref()
                        .zip(state.covering_successor.as_ref())
                        .is_some_and(|(manifest, covering)| {
                            manifest.predecessor() == stamp.owner
                                && manifest.successor() == covering.stamp.owner
                                && covering.root.same_root(root)
                                && covering.successor_dependency
                                && covering.life.live.load(Ordering::Acquire)
                                && covering.life.incarnation.load(Ordering::Acquire)
                                    == covering.stamp.incarnation
                        }),
                }
                && state
                    .spare_successor
                    .as_ref()
                    .is_none_or(|s| s.stamp == stamp)
        });
        if !valid
            || slot.phase != Phase::Accepted
            || slot.incarnation != stamp.incarnation
            || slot.identity != Some(stamp.owner)
            || slot.returned.is_some()
            || slot.ready.is_some()
        {
            return Err(returned);
        }
        slot.returned = Some(returned);
        slot.phase = Phase::Returned;
        Ok(())
    }
    pub(in crate::candidate_custody) fn advance_publications_on_control(
        &self,
        root: &CandidateRoot,
        abandoned: bool,
    ) {
        let Ok(mut work_guard) = self.work.try_lock() else {
            return;
        };
        let work = &mut work_guard.publication;
        self.advance_runtime_preparations(work, abandoned);
        {
            let mut bank_guard = lock(&self.bank);
            let bank = &mut bank_guard.publication;
            let mut failed = false;
            for (index, slot) in bank.slots.iter_mut().enumerate() {
                if slot.anchors.is_some() {
                    let Some(destination) =
                        work.anchors.get_mut(slot.archive).filter(|d| d.is_none())
                    else {
                        slot.phase = Phase::Failed;
                        failed = true;
                        continue;
                    };
                    *destination = slot.anchors.take();
                }
                if slot.phase == Phase::Returned
                    && work.executions[index].is_none()
                    && let Some(returned) = slot.returned.take()
                {
                    work.executions[index] = Some(Execution {
                        returned,
                        proof: None,
                        proof_recorded: false,
                        disposed: None,
                        inputs: None,
                        pending: false,
                        failed: false,
                        first_panic: None,
                        metadata_tokens: Some(std::mem::take(&mut slot.domains)),
                    });
                    slot.phase = Phase::Cleaning;
                }
            }
            if failed {
                self.mark_closing(&mut bank_guard);
            }
        }
        let Work {
            anchors,
            executions,
            failed_payloads,
            banks: _,
            preparations: _,
        } = work;
        for index in 0..executions.len() {
            let Some(execution) = executions[index].as_mut() else {
                continue;
            };
            if execution.pending || execution.failed || failed_payloads[index].is_some() {
                continue;
            }
            let stamp = execution.returned.stamp;
            let result = catch_unwind(AssertUnwindSafe(|| -> Result<bool, ()> {
                if !execution.proof_recorded {
                    if execution.proof.is_none() {
                        execution.pending = true;
                        execution.proof = Some(
                            execution
                                .returned
                                .state
                                .as_mut()
                                .and_then(|s| s.runtime.as_mut())
                                .ok_or(())?
                                .dispose_on_control()
                                .map_err(|_| ())?,
                        );
                        execution.pending = false;
                    }
                    execution
                        .proof
                        .as_ref()
                        .ok_or(())?
                        .record_anchor_cells_on_control(anchors)
                        .map_err(|_| ())?;
                    execution.proof_recorded = true;
                    // Actual reserved backing returns to its original slot before
                    // State metadata destruction, even while origins stay open.
                    let mut bank = lock(&self.bank);
                    let slot = bank.publication.slots.get_mut(stamp.slot).ok_or(())?;
                    let state = execution.returned.state.as_mut().ok_or(())?;
                    if slot.streaming.is_some() && state.streaming.is_some() {
                        return Err(());
                    }
                    if let Some(mut workspace) = state.streaming.take() {
                        workspace.clear();
                        slot.streaming = Some(workspace);
                    }
                    drop(bank);
                    execution.pending = true;
                    drop(execution.returned.state.take());
                    drop(execution.proof.take());
                    drop(execution.metadata_tokens.take());
                    execution.pending = false;
                }
                let archive = {
                    let bank = lock(&self.bank);
                    let slot = bank.publication.slots.get(stamp.slot).ok_or(())?;
                    if slot.life.successor_dependencies.load(Ordering::Acquire) != 0 {
                        return Ok(false);
                    }
                    slot.archive
                };
                let anchor = anchors
                    .get_mut(archive)
                    .and_then(Option::as_mut)
                    .ok_or(())?;
                if !anchor.all_origins_closed() {
                    return Ok(false);
                }
                if execution.disposed.is_none() {
                    execution.pending = true;
                    execution.disposed = Some(anchor.finish_disposal_on_control().map_err(|_| ())?);
                    execution.pending = false;
                }
                if execution.inputs.is_none() {
                    execution.inputs = anchor.recover_inputs_on_control();
                }
                execution.inputs.as_ref().ok_or(())?;
                execution.pending = true;
                drop(anchors[archive].take());
                execution.pending = false;
                Ok(true)
            }));
            let complete = match result {
                Ok(Ok(complete)) => complete,
                Ok(Err(())) => {
                    execution.failed = true;
                    false
                }
                Err(payload) => {
                    execution.first_panic = Some(payload);
                    execution.failed = true;
                    false
                }
            };
            if execution.failed {
                let mut bank = lock(&self.bank);
                self.mark_closing(&mut bank);
                bank.publication.slots[stamp.slot].phase = Phase::Failed;
                continue;
            }
            if !complete {
                continue;
            }
            {
                let mut bank = lock(&self.bank);
                let slot = &mut bank.publication.slots[stamp.slot];
                if slot.identity != Some(stamp.owner)
                    || slot.incarnation != stamp.incarnation
                    || slot.ready.is_some()
                    || execution.inputs.is_none()
                    || execution.disposed.is_none()
                {
                    execution.failed = true;
                    slot.phase = Phase::Failed;
                    self.mark_closing(&mut bank);
                    continue;
                }
                if let (Some(inputs), Some(disposed)) =
                    (execution.inputs.take(), execution.disposed.take())
                {
                    slot.ready = Some(AudioCandidateRecovery {
                        inputs,
                        disposed,
                        return_kind: execution.returned.kind,
                    });
                }
                slot.life.live.store(false, Ordering::Release);
            }
            // Pending remains in the bank while the empty execution metadata
            // normally drops. No ordinary bank lock spans that destruction.
            match catch_unwind(AssertUnwindSafe(|| drop(executions[index].take()))) {
                Ok(()) => {
                    let mut bank = lock(&self.bank);
                    bank.publication.slots[stamp.slot].phase = Phase::Complete;
                }
                Err(payload) => {
                    failed_payloads[index] = Some(payload);
                    let mut bank = lock(&self.bank);
                    self.mark_closing(&mut bank);
                    bank.publication.slots[stamp.slot].phase = Phase::Failed;
                }
            }
        }
        if abandoned {
            let work = &mut work_guard.publication;
            for index in 0..work.banks.len() {
                {
                    let mut bank = lock(&self.bank);
                    let slot = &mut bank.publication.slots[index];
                    if slot.phase == Phase::Complete
                        && work.banks[index].is_none()
                        && let Some(recovery) = slot.ready.take()
                    {
                        work.banks[index] = Some(BankDrop {
                            stamp: Stamp {
                                root: root.root_id(),
                                slot: index,
                                incarnation: slot.incarnation,
                                owner: recovery.disposed.identity(),
                            },
                            recovery: Some(recovery),
                            pending: false,
                            first_panic: None,
                        });
                        slot.phase = Phase::Cleaning;
                    }
                }
                let Some(drop_work) = work.banks[index].as_mut() else {
                    continue;
                };
                if drop_work.pending || drop_work.first_panic.is_some() {
                    continue;
                }
                drop_work.pending = true;
                if let Err(payload) =
                    catch_unwind(AssertUnwindSafe(|| drop(drop_work.recovery.take())))
                {
                    drop_work.first_panic = Some(payload);
                    let mut bank = lock(&self.bank);
                    self.mark_closing(&mut bank);
                    bank.publication.slots[index].phase = Phase::Failed;
                    continue;
                }
                let stamp = drop_work.stamp;
                let mut bank = lock(&self.bank);
                if !release_slot(&mut bank, stamp, Phase::Cleaning) {
                    self.mark_closing(&mut bank);
                    bank.publication.slots[index].phase = Phase::Failed;
                    continue;
                }
                // Empty scalar-only work metadata; all original inputs normally
                // finished destruction before domain/cell/report reuse.
                drop(bank);
                work.banks[index] = None;
            }
        }
    }
    pub(in crate::candidate_custody) fn recover_publication(
        &self,
        ticket: &AudioCandidateTicket,
    ) -> Option<AudioCandidateRecovery> {
        let mut bank = self.bank.try_lock().ok()?;
        let stamp = ticket.stamp;
        if stamp.root != bank.root {
            return None;
        }
        if !can_release_slot(&bank, stamp, Phase::Complete) {
            return None;
        }
        let ready = bank.publication.slots[stamp.slot].ready.take()?;
        let _released = release_slot(&mut bank, stamp, Phase::Complete);
        Some(ready)
    }
}

fn can_release_slot(bank: &NativeBank, stamp: Stamp, phase: Phase) -> bool {
    let Some(slot) = bank.publication.slots.get(stamp.slot) else {
        return false;
    };
    if stamp.root != bank.root
        || slot.phase != phase
        || slot.identity != Some(stamp.owner)
        || slot.incarnation != stamp.incarnation
        || slot.archive >= bank.publication.archives.len()
        || !bank.publication.archives[slot.archive]
    {
        return false;
    }
    let refs = slot.domain_refs;
    !(refs[0].is_some() && refs[0] == refs[1])
        && refs.into_iter().flatten().all(|token| {
            token.root == bank.root
                && bank
                    .domains
                    .get(token.slot)
                    .is_some_and(|domain| domain.matches(token) && domain.uses > 0)
        })
}
fn release_slot(bank: &mut NativeBank, stamp: Stamp, phase: Phase) -> bool {
    if !can_release_slot(bank, stamp, phase) {
        return false;
    }
    let slot = &mut bank.publication.slots[stamp.slot];
    let archive = slot.archive;
    let domains = slot.domain_refs;
    slot.phase = Phase::Vacant;
    slot.identity = None;
    slot.summary = None;
    slot.context_generation = 0;
    slot.policy = None;
    slot.retained = 0;
    slot.peak = 0;
    slot.domain_refs = [None; 2];
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        slot.native_refs = native_sources::References::default();
    }
    bank.publication.archives[archive] = false;
    for token in domains.into_iter().flatten() {
        // All counts and exact associations were checked together before moves.
        bank.domains[token.slot].uses -= 1;
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    bank.sources.request_unreferenced_returns(&bank.publication);
    true
}
