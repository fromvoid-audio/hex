//! Bounded, source-owned editor receipt drain during native source retirement.
use super::*;
use crate::plugin_worker::{
    NativeWorkerControllerProgress, NativeWorkerControllerQuery, StoppedEditProgress,
};
use hex_plugin_host::protocol::{
    ControllerRequest, EditorSealState, ParameterEdit, PluginEditorState, RecoveryEdits,
};

pub(super) enum Stage {
    Live(Option<bool>),
    Prior(bool),
    Close(Option<NativeWorkerControllerQuery>),
    Seal(Option<NativeWorkerControllerQuery>),
    Drain,
    Disposition,
    Resolved,
}

#[derive(Clone, Copy)]
pub(super) enum ShutdownHandoff {
    Unaccepted,
    Claim(Option<bool>),
    Drain(Option<Receipt>),
}

pub(super) struct Drain {
    pub(super) stage: Stage,
    ordinal: u64,
    delivery: Option<(u64, RecoveryEdits)>,
    accepted_latency: Option<std::num::NonZeroU64>,
    retained: Vec<ParameterEdit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    binding: Binding,
    ordinal: u64,
}

#[allow(clippy::large_enum_variant)] // One bounded control-side batch remains in its source cell.
pub enum Progress {
    Pending,
    Batch(Receipt, RecoveryEdits),
    Disposition(Receipt),
    Complete,
}

/// Unique control-side reservation of the original worker through receipt ACK.
/// Dropping it after retirement starts records failed custody; it never claims
/// that pending receipts were delivered. Do not drop this owner on a callback.
pub struct RetiringEditLease {
    root: crate::candidate_custody::CandidateRoot,
    binding: Binding,
    index: usize,
    claimed: bool,
    complete: bool,
    shutdown: Option<ShutdownHandoff>,
}

impl RetiringEditLease {
    pub(super) fn renewal_index(
        &self,
        root: &CandidateRoot,
        binding: Binding,
    ) -> Result<usize, &'static str> {
        if self.root.same_root(root)
            && self.binding == binding
            && !self.claimed
            && !self.complete
            && self.shutdown.is_none()
        {
            Ok(self.index)
        } else {
            Err("native epoch editor receipt reservation differs")
        }
    }

    // Only the original Root renewal may install the resulting binding.
    pub(super) fn set_renewed_binding(&mut self, binding: Binding) {
        self.binding = binding;
    }

    /// Request retirement and release an unclaimed, never-accepted source.
    /// The publication owner supplies the never-accepted fact; a busy or
    /// referenced source retains this lease and must be retried on control.
    pub fn cancel_unaccepted(&mut self) -> Result<(), &'static str> {
        if self.claimed || self.complete {
            return Err("native retiring edit reservation was already claimed");
        }
        let _ = self.root.retire_native_source_preparation(self.binding);
        self.release_unaccepted()
    }

    /// Release an unclaimed reservation only for a candidate that the
    /// publication owner knows was never accepted by a project graph. Root
    /// independently requires that no current graph still references it; that
    /// check alone does not prove the candidate was never accepted earlier.
    pub fn release_unaccepted(&mut self) -> Result<(), &'static str> {
        if self.claimed || self.complete {
            return Err("native retiring edit reservation was already claimed");
        }
        let custody = &self.root.0.eligible;
        let work = custody
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let mut bank = lock(&custody.bank);
        let index = bank
            .sources
            .index(self.binding)
            .ok_or("native source differs")?;
        if index != self.index {
            return Err("native retiring edit reservation differs");
        }
        if bank.publication.references_native_source(index) {
            return Err("native unaccepted source is still referenced");
        }
        let slot = &mut bank.sources.slots[index];
        let cell = &work.sources.cells[index];
        if slot.starting || cell.pending {
            return Err("native preparation is busy");
        }
        if !slot.retiring {
            return Err("native unaccepted source is not retiring");
        }
        if !slot.retiring_edit_reserved
            || !custody.retiring_edit_alive[index].load(Ordering::Acquire)
            || cell.retiring_edits.is_some()
        {
            return Err("native retiring edit reservation differs");
        }
        slot.retiring_edit_reserved = false;
        custody.retiring_edit_alive[index].store(false, Ordering::Release);
        self.complete = true;
        drop(bank);
        drop(work);
        crate::lifecycle::request_candidate_cleanup(&self.root);
        Ok(())
    }

    pub(crate) fn reserve(
        root: &crate::candidate_custody::CandidateRoot,
        binding: Binding,
    ) -> Result<Self, &'static str> {
        let index = root.0.eligible.reserve_retiring_edits(binding)?;
        Ok(Self {
            root: root.clone(),
            binding,
            index,
            claimed: false,
            complete: false,
            shutdown: None,
        })
    }
    /// Transfer the exact reader to the existing Root control cleanup pump.
    /// Once quiescent, Root seals the producer and retains every pending page.
    /// A nonempty sealed sequence without Session disposition remains in
    /// custody. The publication owner supplies any exact completed Session
    /// receipt whose ACK still needs to reach Root.
    pub fn handoff_on_shutdown(
        mut self,
        ever_accepted: bool,
        pending_read: Option<bool>,
        accepted_receipt: Option<Receipt>,
    ) {
        self.shutdown = Some(if self.claimed {
            ShutdownHandoff::Drain(accepted_receipt)
        } else if ever_accepted {
            ShutdownHandoff::Claim(pending_read)
        } else {
            ShutdownHandoff::Unaccepted
        });
    }

    /// Take the sole edit reader before or during retirement. An already
    /// admitted read keeps its mode; a guarded editor detach and already
    /// admitted controller replies can continue on the same worker.
    pub fn claim(&mut self, recovery: Option<bool>) -> Result<(), &'static str> {
        if self.claimed || self.complete {
            return Err("native retiring edits already claimed");
        }
        self.root
            .0
            .eligible
            .claim_retiring_edits(self.binding, recovery)?;
        self.claimed = true;
        Ok(())
    }
    pub fn poll(&mut self) -> Result<Progress, &'static str> {
        if !self.claimed || self.complete {
            return Err("native retiring edits are not claimed");
        }
        let progress = self.root.0.eligible.poll_retiring_edits(self.binding)?;
        if matches!(progress, Progress::Complete) {
            self.complete = true;
            crate::lifecycle::request_candidate_cleanup(&self.root);
        }
        Ok(progress)
    }
    /// Move an exact delivered page to the original owner's bounded retained
    /// sequence. This permits the next read while project disposition is open.
    pub fn retain_page(&self, receipt: Receipt) -> Result<(), &'static str> {
        if !self.claimed || self.complete {
            return Err("native retiring edits are not claimed");
        }
        self.root
            .0
            .eligible
            .retain_retiring_edit_page(self.binding, receipt)
    }

    /// Copy the sealed sequence without releasing its original ownership.
    pub fn retained_sequence(&self, receipt: Receipt) -> Result<Vec<ParameterEdit>, &'static str> {
        if !self.claimed || self.complete {
            return Err("native retiring edits are not claimed");
        }
        self.root
            .0
            .eligible
            .retained_retiring_edit_sequence(self.binding, receipt)
    }

    /// Accept responsibility for every edit and any latency restart in this
    /// exact batch. Repeated latency ordinals remain the caller's obligation;
    /// acknowledgement does not claim the new latency has been applied.
    pub fn acknowledge(&self, receipt: Receipt) -> Result<(), &'static str> {
        if !self.claimed || self.complete {
            return Err("native retiring edits are not claimed");
        }
        self.root
            .0
            .eligible
            .acknowledge_retiring_edits(self.binding, receipt)
    }
}

impl Drop for RetiringEditLease {
    fn drop(&mut self) {
        let custody = &self.root.0.eligible;
        let mut bank = lock(&custody.bank);
        if let Some(index) = bank.sources.index(self.binding)
            && index == self.index
        {
            let slot = &mut bank.sources.slots[index];
            if let Some(handoff) = self.shutdown.take()
                && !self.complete
                && slot.retiring_edit_reserved
            {
                slot.retiring = true;
                slot.retiring_edit_shutdown = Some(handoff);
                drop(bank);
                crate::lifecycle::request_candidate_cleanup(&self.root);
                return;
            }
            if !slot.retiring && !self.claimed {
                slot.retiring_edit_reserved = false;
            } else if !slot.retiring {
                // An admitted reader cannot be silently abandoned just
                // because the graph has not requested retirement yet.
                slot.retiring = true;
            }
            custody.retiring_edit_alive[index].store(false, Ordering::Release);
        }
        drop(bank);
        crate::lifecycle::request_candidate_cleanup(&self.root);
    }
}

impl Custody {
    pub(super) fn reserve_retiring_edits(&self, binding: Binding) -> Result<usize, &'static str> {
        let mut bank = lock(&self.bank);
        if !self.enabled() || bank.closing {
            return Err("native capture is closing");
        }
        let index = bank.sources.index(binding).ok_or("native source differs")?;
        let slot = &mut bank.sources.slots[index];
        if slot.phase != Phase::Accepted
            || slot.retiring
            || slot.starting
            || slot.processor.is_none()
        {
            return Err("native retiring edit reservation differs");
        }
        if slot.retiring_edit_reserved {
            return Err("native retiring edits already reserved");
        }
        slot.retiring_edit_reserved = true;
        self.retiring_edit_alive[index].store(true, Ordering::Release);
        Ok(index)
    }

    pub(super) fn claim_retiring_edits(
        &self,
        binding: Binding,
        recovery: Option<bool>,
    ) -> Result<(), &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        self.claim_retiring_edits_with_work(&mut work.sources, binding, recovery)
    }

    fn claim_retiring_edits_with_work(
        &self,
        work: &mut Work,
        binding: Binding,
        recovery: Option<bool>,
    ) -> Result<(), &'static str> {
        let bank = lock(&self.bank);
        let index = bank.sources.index(binding).ok_or("native source differs")?;
        let slot = &bank.sources.slots[index];
        let cell = &mut work.cells[index];
        if slot.starting || cell.pending {
            return Err("native preparation is busy");
        }
        if !slot.retiring_edit_reserved
            || !self.retiring_edit_alive[index].load(Ordering::Acquire)
            || slot.phase != Phase::Accepted
            || cell.first_panic.is_some()
            || cell.cause().is_some()
            || cell.cleanup_error.is_some()
            || cell.disposal.is_some()
            || cell.retiring_edits.is_some()
        {
            return Err("native retiring edit claim differs");
        }
        let mode = match cell.owner.as_ref() {
            Some(Owner::Effect(owner))
                if owner.binding() == binding
                    && slot.identity == Some(owner.identity())
                    && slot.processor == Some(owner.prepared())
                    && !slot.endpoint_loaned
                    && slot.endpoint_return.is_none() =>
            {
                owner.edit_read_mode()?
            }
            Some(Owner::Loaned(owner))
                if owner.binding() == binding
                    && slot.identity == Some(owner.identity())
                    && slot.processor == Some(owner.prepared())
                    && slot.endpoint_loaned
                    && slot.endpoint_return.is_some() =>
            {
                owner.edit_read_mode()?
            }
            _ => return Err("native retiring edit owner differs"),
        };
        let stage = match (mode, recovery) {
            (None, None) => Stage::Live(None),
            (Some(actual), Some(expected)) if actual == expected => Stage::Live(Some(actual)),
            _ => return Err("native edit observation mode differs"),
        };
        cell.retiring_edits = Some(Drain {
            stage,
            ordinal: 0,
            delivery: None,
            accepted_latency: None,
            retained: Vec::new(),
        });
        Ok(())
    }

    pub(super) fn poll_retiring_edits(&self, binding: Binding) -> Result<Progress, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        self.poll_retiring_edits_with_work(&mut work.sources, binding)
    }

    fn poll_retiring_edits_with_work(
        &self,
        work: &mut Work,
        binding: Binding,
    ) -> Result<Progress, &'static str> {
        let (index, quiescent) = {
            let bank = lock(&self.bank);
            let index = bank.sources.index(binding).ok_or("native source differs")?;
            let slot = &bank.sources.slots[index];
            let cell = &work.cells[index];
            if slot.retiring_edit_reserved
                && self.retiring_edit_alive[index].load(Ordering::Acquire)
                && let Some((ordinal, batch)) = cell
                    .retiring_edits
                    .as_ref()
                    .and_then(|drain| drain.delivery)
            {
                return Ok(Progress::Batch(Receipt { binding, ordinal }, batch));
            }
            if slot.starting || cell.pending {
                return Err("native preparation is busy");
            }
            if !slot.retiring_edit_reserved
                || !self.retiring_edit_alive[index].load(Ordering::Acquire)
                || slot.phase != Phase::Accepted
                || cell.first_panic.is_some()
                || cell.cause().is_some()
                || cell.cleanup_error.is_some()
                || cell.disposal.is_some()
            {
                return Err("native retiring edit custody differs");
            }
            let quiescent = slot.retiring
                && !(slot.endpoint_loaned
                    || slot.endpoint_return.is_some()
                    || bank.publication.references_native_source(index));
            match cell.owner.as_ref() {
                Some(Owner::Effect(owner))
                    if owner.binding() == binding
                        && slot.identity == Some(owner.identity())
                        && slot.processor == Some(owner.prepared())
                        && !slot.endpoint_loaned
                        && slot.endpoint_return.is_none()
                        && owner.constructor_storage_on_control().is_some() => {}
                Some(Owner::Loaned(owner))
                    if owner.binding() == binding
                        && slot.identity == Some(owner.identity())
                        && slot.processor == Some(owner.prepared())
                        && slot.endpoint_loaned
                        && slot.endpoint_return.is_some()
                        && owner.constructor_storage_on_control().is_some() => {}
                _ => return Err("native retiring edit owner differs"),
            }
            (index, quiescent)
        };
        let cell = &mut work.cells[index];
        let drain = cell
            .retiring_edits
            .as_mut()
            .ok_or("native retiring edit claim missing")?;
        if let Stage::Live(pending) = &mut drain.stage {
            if quiescent {
                let Some(Owner::Effect(owner)) = cell.owner.as_ref() else {
                    return Err("native retiring edit owner differs");
                };
                owner.revoke_retiring_edits();
                drain.stage = pending.map_or(Stage::Seal(None), Stage::Prior);
                return Ok(Progress::Pending);
            }
            // A live tail can still request latency compensation changes.
            // Observe them on this same reader rather than silently hiding one.
            let mode = pending.unwrap_or(true);
            let result = match cell.owner.as_mut() {
                Some(Owner::Effect(owner)) if owner.transport_is_live_on_control() => {
                    if mode {
                        owner.poll_recovery_edits(false)
                    } else {
                        owner.poll_parameter_edits().map(|result| {
                            result.map(|edits| RecoveryEdits {
                                edits,
                                pending_latency_restart: None,
                            })
                        })
                    }
                }
                Some(Owner::Loaned(owner)) if owner.transport_is_live_on_control() => {
                    if mode {
                        owner.poll_recovery_edits(false)
                    } else {
                        owner.poll_parameter_edits().map(|result| {
                            result.map(|edits| RecoveryEdits {
                                edits,
                                pending_latency_restart: None,
                            })
                        })
                    }
                }
                Some(Owner::Effect(_) | Owner::Loaned(_)) => return Ok(Progress::Pending),
                _ => return Err("native retiring edit owner differs"),
            };
            return match result {
                Ok(None) => {
                    *pending = Some(mode);
                    Ok(Progress::Pending)
                }
                Ok(Some(batch)) => {
                    *pending = None;
                    // ACK transfers the restart obligation; it does not
                    // reactivate the still-audible source. The helper keeps
                    // reporting the same ordinal until actual reactivation.
                    let latency_delivered = batch.pending_latency_restart.is_none_or(|ordinal| {
                        drain
                            .accepted_latency
                            .is_some_and(|accepted| accepted >= ordinal)
                    });
                    if batch.edits.as_slice().is_empty() && latency_delivered {
                        return Ok(Progress::Pending);
                    }
                    let ordinal = drain
                        .ordinal
                        .checked_add(1)
                        .ok_or("native retiring edit receipt sequence exhausted")?;
                    drain.ordinal = ordinal;
                    drain.delivery = Some((ordinal, batch));
                    Ok(Progress::Batch(Receipt { binding, ordinal }, batch))
                }
                Err("native edit source is not live") => Ok(Progress::Pending),
                Err(reason) => Err(reason),
            };
        }
        let owner = match cell.owner.as_mut() {
            Some(Owner::Effect(owner)) if quiescent => owner,
            Some(Owner::Effect(_) | Owner::Loaned(_)) => return Ok(Progress::Pending),
            _ => return Err("native retiring edit owner differs"),
        };
        let result = match &mut drain.stage {
            Stage::Live(_) => return Err("native retiring edit stage differs"),
            Stage::Prior(recovery) => match owner.poll_stopped_edits(*recovery)? {
                StoppedEditProgress::Pending => return Ok(Progress::Pending),
                StoppedEditProgress::Completed(batch) => {
                    drain.stage = Stage::Seal(None);
                    Some(batch)
                }
            },
            Stage::Close(query) => {
                if query.is_none() {
                    *query = Some(owner.begin_controller_query(ControllerRequest::CloseEditor)?);
                    return Ok(Progress::Pending);
                }
                match owner.poll_controller_query(
                    query.as_ref().ok_or("native editor close token missing")?,
                )? {
                    NativeWorkerControllerProgress::Pending => return Ok(Progress::Pending),
                    NativeWorkerControllerProgress::Editor(PluginEditorState::Closed) => {
                        drain.stage = Stage::Drain;
                        return Ok(Progress::Pending);
                    }
                    _ => return Err("native retiring editor close failed"),
                }
            }
            // Keep an open editor available while the producer reports an
            // active gesture; closing it first could prevent the real End.
            Stage::Seal(query) => {
                if query.is_none() {
                    *query =
                        Some(owner.begin_controller_query(ControllerRequest::SealEditorEdits)?);
                    return Ok(Progress::Pending);
                }
                match owner.poll_controller_query(
                    query.as_ref().ok_or("native editor seal token missing")?,
                )? {
                    NativeWorkerControllerProgress::Pending => return Ok(Progress::Pending),
                    NativeWorkerControllerProgress::EditorSeal(EditorSealState::Sealed) => {
                        drain.stage = Stage::Close(None);
                        return Ok(Progress::Pending);
                    }
                    NativeWorkerControllerProgress::EditorSeal(EditorSealState::NotSealed) => {
                        *query = None;
                        return Err("native editor gesture remains active");
                    }
                    _ => return Err("native retiring editor seal failed"),
                }
            }
            Stage::Disposition => {
                return Ok(Progress::Disposition(Receipt {
                    binding,
                    ordinal: drain.ordinal,
                }));
            }
            Stage::Resolved => {
                lock(&self.bank).sources.slots[index].retiring_edit_reserved = false;
                self.retiring_edit_alive[index].store(false, Ordering::Release);
                cell.retiring_edits = None;
                return Ok(Progress::Complete);
            }
            Stage::Drain => {
                let batch = owner.poll_sealed_edits()?;
                let latency_delivered = batch.pending_latency_restart.is_none_or(|ordinal| {
                    drain
                        .accepted_latency
                        .is_some_and(|accepted| accepted >= ordinal)
                });
                if batch.edits.pending == 0
                    && batch.edits.as_slice().is_empty()
                    && latency_delivered
                {
                    if !drain.retained.is_empty() {
                        drain.ordinal = drain
                            .ordinal
                            .checked_add(1)
                            .ok_or("native retiring edit receipt sequence exhausted")?;
                        drain.stage = Stage::Disposition;
                        return Ok(Progress::Disposition(Receipt {
                            binding,
                            ordinal: drain.ordinal,
                        }));
                    }
                    lock(&self.bank).sources.slots[index].retiring_edit_reserved = false;
                    self.retiring_edit_alive[index].store(false, Ordering::Release);
                    cell.retiring_edits = None;
                    return Ok(Progress::Complete);
                }
                Some(batch)
            }
        };
        if let Some(batch) = result {
            let ordinal = drain
                .ordinal
                .checked_add(1)
                .ok_or("native retiring edit receipt sequence exhausted")?;
            drain.ordinal = ordinal;
            drain.delivery = Some((ordinal, batch));
            return Ok(Progress::Batch(Receipt { binding, ordinal }, batch));
        }
        Ok(Progress::Pending)
    }

    /// Retain an admitted page before allowing the source to deliver another.
    fn retain_retiring_edit_page(
        &self,
        binding: Binding,
        receipt: Receipt,
    ) -> Result<(), &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        self.consume_retiring_edit_receipt(&mut work.sources, binding, receipt, true)
    }

    fn retained_retiring_edit_sequence(
        &self,
        binding: Binding,
        receipt: Receipt,
    ) -> Result<Vec<ParameterEdit>, &'static str> {
        let work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        // Work keeps the source cell pinned while copying the retained pages.
        let index = lock(&self.bank)
            .sources
            .index(binding)
            .ok_or("native source differs")?;
        let drain = work.sources.cells[index]
            .retiring_edits
            .as_ref()
            .ok_or("native retiring edit claim missing")?;
        if receipt.binding != binding
            || receipt.ordinal != drain.ordinal
            || !matches!(drain.stage, Stage::Disposition)
        {
            return Err("native retiring edit disposition differs");
        }
        let mut sequence = Vec::new();
        sequence
            .try_reserve_exact(drain.retained.len())
            .map_err(|_| "native retiring edit allocation failed")?;
        sequence.extend_from_slice(&drain.retained);
        Ok(sequence)
    }

    pub(super) fn acknowledge_retiring_edits(
        &self,
        binding: Binding,
        receipt: Receipt,
    ) -> Result<(), &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        self.acknowledge_retiring_edits_with_work(&mut work.sources, binding, receipt)
    }

    fn acknowledge_retiring_edits_with_work(
        &self,
        work: &mut Work,
        binding: Binding,
        receipt: Receipt,
    ) -> Result<(), &'static str> {
        self.consume_retiring_edit_receipt(work, binding, receipt, false)
    }

    fn consume_retiring_edit_receipt(
        &self,
        work: &mut Work,
        binding: Binding,
        receipt: Receipt,
        retain: bool,
    ) -> Result<(), &'static str> {
        let index = lock(&self.bank)
            .sources
            .index(binding)
            .ok_or("native source differs")?;
        let drain = work.cells[index]
            .retiring_edits
            .as_mut()
            .ok_or("native retiring edit claim missing")?;
        if receipt.binding != binding {
            return Err("native retiring edit receipt differs");
        }
        if matches!(drain.stage, Stage::Disposition) {
            if retain || receipt.ordinal != drain.ordinal {
                return Err("native retiring edit disposition differs");
            }
            drain.retained.clear();
            drain.stage = Stage::Resolved;
            return Ok(());
        }
        let Some((_, batch)) = drain
            .delivery
            .filter(|(ordinal, _)| *ordinal == receipt.ordinal)
        else {
            return Err("native retiring edit receipt differs");
        };
        if retain {
            let edits = batch.edits.as_slice();
            if drain.retained.len() + edits.len() > crate::MAX_RETAINED_EDITOR_EDITS {
                return Err("native retiring edit sequence is full");
            }
            if !edits.is_empty() && drain.retained.capacity() == 0 {
                drain
                    .retained
                    .try_reserve_exact(crate::MAX_RETAINED_EDITOR_EDITS)
                    .map_err(|_| "native retiring edit allocation failed")?;
            }
            drain.retained.extend_from_slice(edits);
        }
        if let Some((_, batch)) = drain.delivery.take()
            && let Some(ordinal) = batch.pending_latency_restart
        {
            drain.accepted_latency = Some(
                drain
                    .accepted_latency
                    .map_or(ordinal, |old| old.max(ordinal)),
            );
        }
        Ok(())
    }

    /// Step a transferred reader only after the original callback and graph
    /// release it. Pending keeps the same stage and tokens in the same cell.
    /// No callback or destructor waits for this control-side pump.
    pub(super) fn advance_retiring_edits_on_shutdown(&self, work: &mut Work, index: usize) {
        let (binding, handoff) = {
            let bank = lock(&self.bank);
            let slot = &bank.sources.slots[index];
            if !slot.retiring
                || slot.endpoint_loaned
                || slot.endpoint_return.is_some()
                || bank.publication.references_native_source(index)
                || slot.starting
                || work.cells[index].pending
                || slot.phase == Phase::Failed
            {
                return;
            }
            let (Some(binding), Some(handoff)) = (slot.binding, slot.retiring_edit_shutdown) else {
                return;
            };
            (binding, handoff)
        };

        let next = match handoff {
            ShutdownHandoff::Unaccepted => {
                let mut bank = lock(&self.bank);
                let slot = &mut bank.sources.slots[index];
                slot.retiring_edit_reserved = false;
                self.retiring_edit_alive[index].store(false, Ordering::Release);
                Ok(None)
            }
            ShutdownHandoff::Claim(recovery) => self
                .claim_retiring_edits_with_work(work, binding, recovery)
                .map(|()| Some(ShutdownHandoff::Drain(None))),
            ShutdownHandoff::Drain(Some(receipt)) => self
                .acknowledge_retiring_edits_with_work(work, binding, receipt)
                .map(|()| Some(ShutdownHandoff::Drain(None))),
            ShutdownHandoff::Drain(None) => {
                match self.poll_retiring_edits_with_work(work, binding) {
                    Ok(Progress::Pending) => return,
                    Ok(Progress::Complete) => Ok(None),
                    Ok(Progress::Batch(receipt, _)) => {
                        // Move the page to the existing retained sequence so
                        // the quiescent worker can return the rest. Latency
                        // restarts have no remaining PCM to compensate here.
                        self.consume_retiring_edit_receipt(work, binding, receipt, true)
                            .map(|()| Some(ShutdownHandoff::Drain(None)))
                    }
                    Ok(Progress::Disposition(_)) => {
                        Err("vst3 editor receipts need project disposition after shutdown")
                    }
                    Err(reason) => Err(reason),
                }
            }
        };

        match next {
            Ok(state) => lock(&self.bank).sources.slots[index].retiring_edit_shutdown = state,
            Err(
                "native preparation is busy"
                | "native controller mailbox is busy"
                | "native edit observation is busy",
            ) => {}
            Err(reason) => {
                let cell = &mut work.cells[index];
                cell.cleanup_error = Some(hex_plugin_host::Error::Protocol(reason));
                self.fail_native_source(index);
            }
        }
    }
}

#[cfg(test)]
mod tests;
