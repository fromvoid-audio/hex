//! One pre-publication phase in the already reserved runtime cells. The worker
//! borrows rooted construction storage; the adapter retains only this token.
use super::*;
mod captured_main;
pub use captured_main::{CapturedMainFile, CapturedMainFileError, CapturedMainFileLoan};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCandidatePreparationToken {
    pub(super) root: u64,
    pub(super) slot: usize,
    pub(super) incarnation: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct AudioCandidatePreparationReservation {
    pub source_capacity: usize,
    pub storage: hex_engine::AudioStorageReservation,
    pub member_rows: usize,
    pub transfer_rows: usize,
    pub handle_rows: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidatePreparationProgress {
    Collecting,
    Ready,
    Retiring,
    Refused(&'static str),
}
pub struct AudioCandidatePreparationRefusal {
    pub reason: &'static str,
    pub domains: Vec<AudioCandidateDomainToken>,
}
pub(super) struct PreparationExecution {
    pub(super) token: AudioCandidatePreparationToken,
    pub(super) cell: Option<hex_engine::AudioCandidatePreparation>,
    pub(super) domains: Vec<AudioCandidateDomainToken>,
    pub(super) pending: bool,
    pub(super) failed: bool,
    pub(super) refusal: Option<&'static str>,
    pub(super) captured_main: Option<hex_engine::AudioCandidateIdentity>,
}
#[cfg(test)]
impl PreparationExecution {
    pub(super) fn test_summary(&self) -> (bool, bool, Option<&'static str>, bool) {
        (self.pending, self.failed, self.refusal, self.cell.is_some())
    }
}
impl Custody {
    pub(in crate::candidate_custody) fn reserve_runtime_preparation(
        &self,
        reservation: AudioCandidatePreparationReservation,
        domains: Vec<AudioCandidateDomainToken>,
    ) -> Result<AudioCandidatePreparationToken, AudioCandidatePreparationRefusal> {
        let Ok(mut work) = self.work.try_lock() else {
            return Err(AudioCandidatePreparationRefusal {
                reason: "candidate preparation is busy",
                domains,
            });
        };
        let preflight = (|| {
            let mut bank = lock(&self.bank);
            let limits = bank
                .publication
                .reservation
                .ok_or("candidate publication is not reserved")?;
            if bank.closing
                || domains.len() > 2
                || reservation.storage.handle_obligations > reservation.handle_rows
                || reservation.storage.allocation_items > limits.data_rows
                || reservation.handle_rows > limits.handle_rows
                || reservation.storage.max_construction_data_bytes
                    < reservation.storage.max_prepared_data_bytes
                || bank
                    .publication
                    .slots
                    .iter()
                    .any(|s| s.preparation_reservation.is_some())
            {
                return Err("candidate preparation is unavailable");
            }
            let index = bank
                .publication
                .slots
                .iter()
                .position(|s| {
                    s.phase == Phase::Vacant
                        && s.state_storage.is_none()
                        && s.claim_storage.iter().all(Option::is_none)
                })
                .ok_or("candidate runtime cells are full")?;
            let archive = bank
                .publication
                .archives
                .iter()
                .position(|occupied| !occupied)
                .ok_or("candidate archive cells are full")?;
            let incarnation = bank.publication.slots[index]
                .incarnation
                .checked_add(1)
                .ok_or("candidate preparation incarnation exhausted")?;
            if work.publication.preparations[index].is_some() {
                return Err("candidate preparation destination is occupied");
            }
            let mut retained = bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.data))
                .ok_or("candidate preparation DATA overflow")?;
            let mut peak = bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.peak))
                .ok_or("candidate preparation peak overflow")?;
            let mut members = reservation.member_rows;
            let mut transfers = reservation.transfer_rows;
            for slot in &bank.publication.slots {
                retained = retained
                    .checked_add(slot.retained)
                    .ok_or("candidate preparation DATA overflow")?;
                peak = peak
                    .checked_add(slot.peak)
                    .ok_or("candidate preparation peak overflow")?;
                if let Some(summary) = slot.summary {
                    members = members
                        .checked_add(summary.member_rows)
                        .ok_or("candidate member overflow")?;
                    transfers = transfers
                        .checked_add(summary.transfer_rows)
                        .ok_or("candidate transfer overflow")?;
                }
            }
            let metadata = domains
                .capacity()
                .checked_mul(std::mem::size_of::<AudioCandidateDomainToken>())
                .ok_or("candidate domain metadata overflow")?;
            let retained_charge = reservation
                .storage
                .max_prepared_data_bytes
                .checked_add(metadata)
                .ok_or("candidate preparation DATA overflow")?;
            let peak_charge = reservation
                .storage
                .max_construction_data_bytes
                .checked_add(metadata)
                .ok_or("candidate preparation peak overflow")?;
            if retained
                .checked_add(retained_charge)
                .is_none_or(|n| n > limits.max_retained_data_bytes)
                || peak
                    .checked_add(peak_charge)
                    .is_none_or(|n| n > limits.max_construction_peak_data_bytes)
                || members > limits.member_rows
                || transfers > limits.transfer_rows
                || reservation.handle_rows
                    > remaining_handle_reservation(&bank)
                        .ok_or("candidate handle reservation exceeded")?
            {
                return Err("candidate preparation reservation exceeded");
            }
            for (i, token) in domains.iter().enumerate() {
                if token.root != bank.root
                    || domains[..i].contains(token)
                    || bank.domains.get(token.slot).is_none_or(|s| {
                        !s.matches(*token)
                            || s.phase != Phase::Accepted
                            || s.retiring
                            || s.owner.is_none()
                            || s.uses == usize::MAX
                    })
                {
                    return Err("candidate preparation domain is unavailable");
                }
            }
            let token = AudioCandidatePreparationToken {
                root: bank.root,
                slot: index,
                incarnation,
            };
            for domain in &domains {
                bank.domains[domain.slot].uses += 1;
            }
            bank.publication.archives[archive] = true;
            let slot = &mut bank.publication.slots[index];
            slot.incarnation = incarnation;
            slot.archive = archive;
            slot.phase = Phase::Reserved;
            slot.preparation_reservation = Some(reservation);
            slot.preparation_retiring = false;
            slot.retained = retained_charge;
            slot.peak = peak_charge;
            Ok(token)
        })();
        let token = match preflight {
            Ok(token) => token,
            Err(reason) => return Err(AudioCandidatePreparationRefusal { reason, domains }),
        };
        // Dedicated work custody spans construction, never an ordinary bank or
        // terminal lock. This exact destination exists before any allocation.
        work.publication.preparations[token.slot] = Some(PreparationExecution {
            token,
            cell: None,
            domains,
            pending: false,
            failed: false,
            refusal: None,
            captured_main: None,
        });
        let Some(execution) = work.publication.preparations[token.slot].as_mut() else {
            let mut bank = lock(&self.bank);
            bank.publication.slots[token.slot].phase = Phase::Failed;
            self.mark_closing(&mut bank);
            return Ok(token);
        };
        execution.pending = true;
        match catch_unwind(AssertUnwindSafe(|| {
            hex_engine::AudioCandidatePreparation::try_new_on_control(
                reservation.source_capacity,
                reservation.storage,
            )
        })) {
            Ok(Ok(cell)) => {
                execution.cell = Some(cell);
                execution.pending = false;
            }
            Ok(Err(_)) => {
                execution.refusal = Some("candidate preparation reservation refused");
                execution.pending = false;
                lock(&self.bank).publication.slots[token.slot].preparation_retiring = true;
            }
            Err(payload) => {
                execution.failed = true;
                work.publication.failed_payloads[token.slot] = Some(payload);
                let mut bank = lock(&self.bank);
                bank.publication.slots[token.slot].phase = Phase::Failed;
                self.mark_closing(&mut bank);
            }
        }
        Ok(token)
    }
    pub(in crate::candidate_custody) fn bind_runtime_preparation_domains(
        &self,
        token: AudioCandidatePreparationToken,
        domains: Vec<AudioCandidateDomainToken>,
    ) -> Result<(), AudioCandidatePreparationRefusal> {
        let Ok(mut work) = self.work.try_lock() else {
            return Err(AudioCandidatePreparationRefusal {
                reason: "candidate preparation is busy",
                domains,
            });
        };
        let Some(execution) = work
            .publication
            .preparations
            .get_mut(token.slot)
            .and_then(Option::as_mut)
            .filter(|e| e.token == token && !e.pending && !e.failed && e.refusal.is_none())
        else {
            return Err(AudioCandidatePreparationRefusal {
                reason: "candidate preparation is incomplete",
                domains,
            });
        };
        let checked = (|| {
            let mut bank = lock(&self.bank);
            let limits = bank
                .publication
                .reservation
                .ok_or("candidate publication is not reserved")?;
            let slot = bank
                .publication
                .slots
                .get(token.slot)
                .filter(|s| {
                    bank.root == token.root
                        && s.incarnation == token.incarnation
                        && s.phase == Phase::Reserved
                        && !s.preparation_retiring
                        && s.preparation_reservation.is_some()
                })
                .ok_or("candidate preparation token differs")?;
            if bank.closing || domains.len() > 2 {
                return Err("candidate preparation is unavailable");
            }
            if execution.domains.capacity() != 0 {
                return Err("candidate preparation domains already bound");
            }
            let metadata = domains
                .capacity()
                .checked_mul(std::mem::size_of::<AudioCandidateDomainToken>())
                .ok_or("candidate domain metadata overflow")?;
            let retained = bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.data))
                .and_then(|n| n.checked_add(bank.publication.retained_bytes()?))
                .and_then(|n| n.checked_add(metadata))
                .ok_or("candidate DATA overflow")?;
            let peak = bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.peak))
                .and_then(|n| n.checked_add(bank.publication.peak_bytes()?))
                .and_then(|n| n.checked_add(metadata))
                .ok_or("candidate peak overflow")?;
            if retained > limits.max_retained_data_bytes
                || peak > limits.max_construction_peak_data_bytes
            {
                return Err("candidate domain metadata reservation exceeded");
            }
            let charge = slot
                .retained
                .checked_add(metadata)
                .ok_or("candidate DATA overflow")?;
            let peak_charge = slot
                .peak
                .checked_add(metadata)
                .ok_or("candidate peak overflow")?;
            for (index, domain) in domains.iter().enumerate() {
                if domain.root != bank.root
                    || domains[..index].contains(domain)
                    || bank.domains.get(domain.slot).is_none_or(|s| {
                        !s.matches(*domain)
                            || s.phase != Phase::Accepted
                            || s.retiring
                            || s.owner.is_none()
                            || s.uses == usize::MAX
                    })
                {
                    return Err("candidate preparation domain is unavailable");
                }
            }
            for domain in &domains {
                bank.domains[domain.slot].uses += 1;
            }
            let slot = &mut bank.publication.slots[token.slot];
            slot.retained = charge;
            slot.peak = peak_charge;
            Ok(())
        })();
        if let Err(reason) = checked {
            return Err(AudioCandidatePreparationRefusal { reason, domains });
        }
        // The same dedicated guard and exact token preserve this destination;
        // only a zero-capacity empty Vec is replaced after counter commitment.
        execution.domains = domains;
        Ok(())
    }
    pub(in crate::candidate_custody) fn with_runtime_preparation(
        &self,
        token: AudioCandidatePreparationToken,
        prepare: impl FnOnce(
            &mut hex_engine::AudioCandidatePreparation,
        ) -> Result<(), hex_engine::AudioStorageError>,
    ) -> Result<AudioCandidatePreparationProgress, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "candidate preparation is busy")?;
        let retiring = {
            let bank = lock(&self.bank);
            let slot = bank
                .publication
                .slots
                .get(token.slot)
                .ok_or("candidate preparation token differs")?;
            if bank.root != token.root
                || slot.incarnation != token.incarnation
                || slot.phase != Phase::Reserved
                || slot.preparation_reservation.is_none()
            {
                return Err("candidate preparation token differs");
            }
            bank.closing || slot.preparation_retiring
        };
        let execution = work
            .publication
            .preparations
            .get_mut(token.slot)
            .and_then(Option::as_mut)
            .filter(|p| p.token == token)
            .ok_or("candidate preparation storage differs")?;
        if let Some(reason) = execution.refusal {
            return Ok(AudioCandidatePreparationProgress::Refused(reason));
        }
        if execution.captured_main.is_some() {
            return Err("candidate preparation belongs to captured Main work");
        }
        if retiring {
            return Ok(AudioCandidatePreparationProgress::Retiring);
        }
        if execution.pending || execution.failed {
            return Ok(AudioCandidatePreparationProgress::Refused(
                "candidate preparation is incomplete",
            ));
        }
        execution.pending = true;
        let result = catch_unwind(AssertUnwindSafe(|| {
            prepare(
                execution
                    .cell
                    .as_mut()
                    .ok_or(hex_engine::AudioStorageError::SourceMismatch)?,
            )
        }));
        match result {
            Ok(Ok(())) => execution.pending = false,
            Ok(Err(_)) => {
                execution.pending = false;
                execution.refusal = Some("candidate preparation refused");
                let mut bank = lock(&self.bank);
                bank.publication.slots[token.slot].preparation_retiring = true;
                if execution
                    .cell
                    .as_ref()
                    .is_some_and(|cell| cell.failed_incomplete())
                {
                    execution.failed = true;
                    bank.publication.slots[token.slot].phase = Phase::Failed;
                    self.mark_closing(&mut bank);
                }
            }
            Err(payload) => {
                execution.failed = true;
                work.publication.failed_payloads[token.slot] = Some(payload);
                let mut bank = lock(&self.bank);
                bank.publication.slots[token.slot].phase = Phase::Failed;
                self.mark_closing(&mut bank);
                return Ok(AudioCandidatePreparationProgress::Refused(
                    "candidate preparation is incomplete",
                ));
            }
        }
        if let Some(reason) = execution.refusal {
            return Ok(AudioCandidatePreparationProgress::Refused(reason));
        }
        let bank = lock(&self.bank);
        if bank.closing || bank.publication.slots[token.slot].preparation_retiring {
            return Ok(AudioCandidatePreparationProgress::Retiring);
        }
        Ok(
            if execution
                .cell
                .as_mut()
                .and_then(|cell| cell.owner_on_control())
                .is_some()
            {
                AudioCandidatePreparationProgress::Ready
            } else {
                AudioCandidatePreparationProgress::Collecting
            },
        )
    }
    pub(in crate::candidate_custody) fn stage_runtime_preparation(
        &self,
        root: &CandidateRoot,
        token: AudioCandidatePreparationToken,
        header: Header,
        rate: u32,
        enqueue: impl FnOnce(Queued),
    ) -> Result<AudioCandidateTicket, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "candidate preparation is busy")?;
        let (ledger, external) = {
            let bank = lock(&self.bank);
            let slot = bank
                .publication
                .slots
                .get(token.slot)
                .filter(|slot| {
                    bank.root == token.root
                        && slot.incarnation == token.incarnation
                        && slot.phase == Phase::Reserved
                        && !slot.preparation_retiring
                        && slot.preparation_reservation.is_some()
                })
                .ok_or("candidate preparation token differs")?;
            if bank.closing {
                return Err("candidate Root is closing");
            }
            let execution = work
                .publication
                .preparations
                .get(token.slot)
                .and_then(Option::as_ref)
                .filter(|e| e.token == token && !e.pending && !e.failed && e.refusal.is_none())
                .ok_or("candidate preparation is incomplete")?;
            let external = execution
                .domains
                .capacity()
                .checked_mul(std::mem::size_of::<AudioCandidateDomainToken>())
                .and_then(|n| n.checked_add(state_peak_storage_bytes(bank.native.is_some())?))
                .and_then(|n| {
                    n.checked_add(
                        slot.streaming
                            .as_ref()
                            .map_or(Some(0), |w| w.backing_bytes())?,
                    )
                })
                .ok_or("candidate mutable runtime storage overflow")?;
            let limits = bank
                .publication
                .reservation
                .ok_or("candidate publication is not reserved")?;
            let pending = slot
                .preparation_reservation
                .ok_or("candidate preparation reservation differs")?;
            if pending.storage.allocation_items > limits.data_rows
                || pending.handle_rows > limits.handle_rows
            {
                return Err("candidate ledger reservation exceeded");
            }
            let mut ledger = limits.ledger();
            ledger.data_rows = pending.storage.allocation_items;
            ledger.handle_rows = pending.handle_rows;
            (ledger, external)
        };
        let execution = work.publication.preparations[token.slot]
            .as_mut()
            .ok_or("candidate preparation storage differs")?;
        // Candidate-local budget preflight is NOT the final joint admission.
        // It retains the existing split allowance before allocating that Box.
        // Pending native reservation already covers this whole constructor.
        let summary = match catch_unwind(AssertUnwindSafe(|| {
            let owner = execution
                .cell
                .as_mut()
                .and_then(|cell| cell.owner_on_control())
                .ok_or("candidate preparation is not ready")?;
            if owner.sample_rate() != rate
                || owner.phase() != hex_engine::AudioCandidateDisposalPhase::Owned
                || header.policy != crate::prepared::PublicationPolicy::InheritRuntime
            {
                return Err("candidate constructor rate or phase differs");
            }
            owner
                .prepare_device_metering_on_control(true)
                .map_err(|_| "candidate constructor metering setup refused")?;
            owner
                .seal_constructor_with_runtime_storage_on_control(ledger, external)
                .map_err(|_| "candidate DATA or resource closure refused")
        })) {
            Ok(result) => result?,
            Err(payload) => {
                execution.failed = true;
                work.publication.failed_payloads[token.slot] = Some(payload);
                let mut bank = lock(&self.bank);
                bank.publication.slots[token.slot].phase = Phase::Failed;
                self.mark_closing(&mut bank);
                return Err("candidate preparation is incomplete");
            }
        };
        let cell = execution
            .cell
            .as_mut()
            .ok_or("candidate preparation is not ready")?;
        // Allocate the ACTUAL claimed wrapper after its local allowance check,
        // but BEFORE the short terminal scope. Original owner remains rooted.
        let split = match catch_unwind(AssertUnwindSafe(move || {
            // Consume the actual mutable reference in this FnOnce call; a
            // FnMut reborrow cannot escape as the lifetime-bound split handle.
            let cell = cell;
            hex_engine::AudioCandidatePreparation::preallocate_publication_split_on_control(cell)
        })) {
            Ok(result) => {
                result.map_err(|_| "candidate constructor split preallocation refused")?
            }
            Err(payload) => {
                execution.failed = true;
                work.publication.failed_payloads[token.slot] = Some(payload);
                let mut bank = lock(&self.bank);
                bank.publication.slots[token.slot].phase = Phase::Failed;
                self.mark_closing(&mut bank);
                return Err("candidate preparation is incomplete");
            }
        };
        let owner = split
            .owner_on_control()
            .ok_or("candidate preparation is not ready")?;
        let preliminary = self.preflight_stage(
            root,
            owner,
            &execution.domains,
            header,
            summary,
            Some(token),
        )?;
        // This actual unpublished State Box and paired Root/Life handles exist
        // BEFORE the terminal barrier. Final preflight rechecks its exact stamp;
        // neither the handles nor this preliminary plan grant installation.
        let state =
            self.preconstruct_publication_state(root, &preliminary, &execution.domains, header)?;
        let barrier = lock(&root.0.bank);
        if barrier.closing || barrier.retiring {
            drop(barrier);
            self.discard_unpublished_state_on_control(state);
            return Err("candidate Root is closing");
        }
        // Preserve the existing outer-root -> eligible-bank order, retaining
        // the ACTUAL preparation-cancellation exclusion through queue commit.
        // External family/worker inspection will precede these native guards;
        // this candidate-local operation does not yet provide that joint proof.
        let mut bank = lock(&self.bank);
        let final_plan = (|| {
            let owner = split
                .owner_on_control()
                .ok_or("candidate preparation is not ready")?;
            let plan = Self::preflight_stage_with_bank(
                &bank,
                root,
                owner,
                &execution.domains,
                header,
                summary,
                Some(token),
            )?;
            if plan.stamp != state.stamp {
                return Err("candidate preallocated publication stamp differs");
            }
            Ok(plan)
        })();
        let plan = match final_plan {
            Ok(plan) => plan,
            Err(reason) => {
                drop(bank);
                drop(barrier);
                self.discard_unpublished_state_on_control(state);
                return Err(reason);
            }
        };
        execution.pending = true;
        // Only the preallocated transfer moves owners. This remains the legacy
        // candidate-local stage: the actual joint ledger/source/cancel frontier
        // still needs the separate closed-graph route. The held eligible guard
        // excludes native preparation cancellation, not the worker watermark.
        let parts = catch_unwind(AssertUnwindSafe(|| split.transfer_on_control()));
        let (runtime, anchors) = match parts {
            Ok(Ok(parts)) => parts,
            Ok(Err(_)) => {
                execution.pending = false;
                drop(bank);
                drop(barrier);
                self.discard_unpublished_state_on_control(state);
                return Err("candidate constructor split refused");
            }
            Err(payload) => {
                execution.failed = true;
                work.publication.failed_payloads[token.slot] = Some(payload);
                bank.publication.slots[token.slot].phase = Phase::Failed;
                self.mark_closing(&mut bank);
                drop(bank);
                drop(barrier);
                self.discard_unpublished_state_on_control(state);
                return Err("candidate preparation is incomplete");
            }
        };
        // Read only the actual privately retained constructor halves. This
        // Engine check traverses their owned fields/receipts without locking a
        // native bank or invoking provider/user code. No Root/Life clone occurs.
        let matched = anchors
            .constructor_claims(&runtime)
            .is_some_and(|proof| proof.summary() == Some(summary));
        let domains = std::mem::take(&mut execution.domains);
        let (queued, ticket) = self.commit_stage_with_bank(
            StageCommit {
                runtime,
                anchors,
                domains,
                header,
                summary,
                plan,
                state,
            },
            matched,
            &mut bank,
        );
        enqueue(queued);
        execution.pending = false;
        drop(bank);
        drop(barrier);
        self.advance_runtime_preparations(&mut work.publication, false);
        Ok(ticket)
    }
    pub(in crate::candidate_custody) fn cancel_runtime_preparation(
        &self,
        token: AudioCandidatePreparationToken,
    ) -> bool {
        let mut bank = lock(&self.bank);
        if bank.root != token.root {
            return false;
        }
        let Some(slot) = bank.publication.slots.get_mut(token.slot).filter(|s| {
            s.incarnation == token.incarnation
                && s.phase == Phase::Reserved
                && s.preparation_reservation.is_some()
        }) else {
            return false;
        };
        slot.preparation_retiring = true;
        true
    }
    pub(super) fn advance_runtime_preparations(&self, work: &mut Work, abandoned: bool) {
        for index in 0..work.preparations.len() {
            let Some(execution) = work.preparations[index].as_mut() else {
                continue;
            };
            if execution.pending
                || execution.failed
                || execution.captured_main.is_some()
                || work.failed_payloads[index].is_some()
            {
                continue;
            }
            let metadata_only = {
                let bank = lock(&self.bank);
                let slot = &bank.publication.slots[index];
                slot.preparation_metadata_pending && slot.preparation_reservation.is_none()
            };
            {
                let mut bank = lock(&self.bank);
                let closing = bank.closing;
                let slot = &mut bank.publication.slots[index];
                if !metadata_only
                    && (slot.phase != Phase::Reserved
                        || (!slot.preparation_retiring
                            && !closing
                            && !abandoned
                            && execution.refusal.is_none()))
                {
                    continue;
                }
                if !metadata_only {
                    slot.phase = Phase::Cleaning;
                }
            }
            execution.pending = true;
            let result = catch_unwind(AssertUnwindSafe(|| {
                if let Some(cell) = execution.cell.as_mut() {
                    cell.cleanup_on_control()?;
                }
                drop(execution.cell.take());
                Ok::<_, hex_engine::AudioStorageError>(())
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(hex_engine::AudioStorageError::ExternalReadersPending)) => {
                    // Cleanup has not consumed the constructor root. Its actual
                    // registered readers and all charges remain in this slot.
                    execution.pending = false;
                    lock(&self.bank).publication.slots[index].phase = Phase::Reserved;
                    continue;
                }
                result => {
                    execution.failed = true;
                    let mut bank = lock(&self.bank);
                    bank.publication.slots[index].phase = Phase::Failed;
                    self.mark_closing(&mut bank);
                    if let Err(payload) = result {
                        work.failed_payloads[index] = Some(payload);
                    }
                    continue;
                }
            }
            if metadata_only {
                let removed = work.preparations[index].take();
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(removed))) {
                    work.failed_payloads[index] = Some(payload);
                    let mut bank = lock(&self.bank);
                    bank.publication.slots[index].phase = Phase::Failed;
                    self.mark_closing(&mut bank);
                } else {
                    lock(&self.bank).publication.slots[index].preparation_metadata_pending = false;
                }
                continue;
            }
            // Domains are scalar associations; remove uses under the bank, then
            // destroy the actual metadata backing outside it before vacancy.
            {
                let mut bank = lock(&self.bank);
                if execution.domains.iter().any(|token| {
                    bank.domains
                        .get(token.slot)
                        .is_none_or(|d| !d.matches(*token) || d.uses == 0)
                }) {
                    execution.failed = true;
                } else {
                    for token in &execution.domains {
                        bank.domains[token.slot].uses -= 1;
                    }
                }
                if execution.failed {
                    bank.publication.slots[index].phase = Phase::Failed;
                    self.mark_closing(&mut bank);
                    continue;
                }
            }
            let archive = { lock(&self.bank).publication.slots[index].archive };
            let removed = work.preparations[index].take();
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(removed))) {
                work.failed_payloads[index] = Some(payload);
                let mut bank = lock(&self.bank);
                bank.publication.slots[index].phase = Phase::Failed;
                self.mark_closing(&mut bank);
                continue;
            }
            let mut bank = lock(&self.bank);
            bank.publication.archives[archive] = false;
            let slot = &mut bank.publication.slots[index];
            slot.preparation_reservation = None;
            slot.preparation_retiring = false;
            if slot.state_storage.is_some() || slot.claim_storage.iter().any(Option::is_some) {
                slot.phase = Phase::Failed;
                self.mark_closing(&mut bank);
                continue;
            }
            slot.phase = Phase::Vacant;
            slot.retained = 0;
            slot.peak = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn root() -> CandidateRoot {
        root_with_rows(4096, 4096)
    }
    fn root_with_rows(member_rows: usize, transfer_rows: usize) -> CandidateRoot {
        let root = CandidateRoot::new(2).unwrap();
        root.0
            .eligible
            .configure_publications(
                root.root_id(),
                AudioCandidatePublicationReservation {
                    runtime_slots: 2,
                    archive_slots: 2,
                    member_rows,
                    transfer_rows,
                    receipt_rows: 2,
                    data_rows: 4096,
                    handle_rows: 4096,
                    max_retained_data_bytes: 64 * 1024 * 1024,
                    max_construction_peak_data_bytes: 64 * 1024 * 1024,
                },
                std::mem::size_of::<Inner>() + 2 * std::mem::size_of::<Slot>(),
                root.0.resident.as_ref(),
                root.0.root_fields.as_ref(),
            )
            .unwrap();
        root
    }
    fn reservation() -> AudioCandidatePreparationReservation {
        AudioCandidatePreparationReservation {
            source_capacity: 1,
            member_rows: 1024,
            transfer_rows: 1024,
            handle_rows: 512,
            storage: hex_engine::AudioStorageReservation {
                allocation_items: 256,
                handle_obligations: 512,
                max_prepared_data_bytes: 16 * 1024 * 1024,
                max_construction_data_bytes: 16 * 1024 * 1024,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        }
    }
    fn project() -> (
        hex_project::Project,
        hex_project::AssetId,
        hex_engine::Audio,
    ) {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "audio.wav".into(),
                sample_rate: 48000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("asset");
        };
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 8,
                source_offset: 0,
                source_length: 8,
                looping: false,
            })
            .unwrap();
        (
            editor.project().clone(),
            asset,
            hex_engine::Audio::new(48000, vec![[0.25; 2]; 8]).unwrap(),
        )
    }
    #[test]
    fn actual_native_work_contention_refuses_loan_and_retains_prepared_sources() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported constructor layout; no positive actual work proof");
            return;
        }
        let root = root();
        let custody = &root.0.eligible;
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let (_, asset, audio) = project();
        assert_eq!(
            custody
                .with_runtime_preparation(token, |cell| cell
                    .copy_resident_source_on_control(asset, &audio))
                .unwrap(),
            AudioCandidatePreparationProgress::Collecting
        );
        let before = lock(&custody.work).publication.preparations[token.slot]
            .as_ref()
            .unwrap()
            .test_summary();
        crate::candidate_custody::inline_lock_tests::held_on_other_thread(&custody.work, || {
            let measured = allocation_counter::measure(|| {
                assert_eq!(
                    custody
                        .with_runtime_preparation(token, |_| panic!("contended loan must not run")),
                    Err("candidate preparation is busy")
                );
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
        });
        assert_eq!(
            lock(&custody.work).publication.preparations[token.slot]
                .as_ref()
                .unwrap()
                .test_summary(),
            before
        );
        assert_eq!(
            custody
                .with_runtime_preparation(token, |cell| {
                    assert!(!cell.failed_incomplete());
                    Ok(())
                })
                .unwrap(),
            AudioCandidatePreparationProgress::Collecting
        );
        assert!(custody.cancel_runtime_preparation(token));
        custody.advance_publications_on_control(&root, false);
        assert!(custody.complete());
        eprintln!(
            "SUPPORTED actual native Work other-thread hold: real loan busy BEFORE callback/allocfree0, same parent/source reservation retained; after release loan then cancellation cleanup completes"
        );
    }

    #[test]
    fn rooted_stage_holds_terminal_and_preparation_cancellation_guards_through_enqueue() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported rooted native guard test; no positive stage proof");
            return;
        }
        eprintln!("SUPPORTED actual rooted stage guards, cancel-before refusal and normal return");
        for cancel_before in [true, false] {
            let (project, asset, audio) = project();
            // The older root() fixture covers partial construction only. This
            // case stages a complete runtime, so reserve its independently
            // quoted actual project rows without changing any DATA ceiling.
            let quote =
                hex_engine::AudioCandidateReservationQuote::for_project(&project, 1).unwrap();
            let root = root_with_rows(quote.member_rows, quote.transfer_rows);
            let custody = &root.0.eligible;
            let mut limits = reservation();
            limits.member_rows = quote.member_rows;
            limits.transfer_rows = quote.transfer_rows;
            limits.storage = quote
                .storage_reservation(8 * std::mem::size_of::<hex_engine::Frame>())
                .unwrap();
            limits.handle_rows = limits.storage.handle_obligations;
            let token = custody
                .reserve_runtime_preparation(limits, Vec::new())
                .ok()
                .unwrap();
            let mut identity = None;
            custody
                .with_runtime_preparation(token, |cell| {
                    cell.copy_resident_source_on_control(asset, &audio)?;
                    cell.prepare_on_control(
                        &project,
                        11,
                        hex_engine::AudioSourceContext {
                            project_generation: 2,
                            editor_revision: 8,
                        },
                    )?;
                    identity = cell.owner_on_control().map(|owner| owner.identity());
                    Ok(())
                })
                .unwrap();
            if cancel_before {
                assert!(custody.cancel_runtime_preparation(token));
            }
            let mut queued = None;
            let result =
                custody.stage_runtime_preparation(
                    &root,
                    token,
                    Header {
                        revision: 1,
                        context_generation: 2,
                        policy: crate::prepared::PublicationPolicy::InheritRuntime,
                    },
                    48_000,
                    |value| {
                        // Test-only queue observer. Production supplies its already
                        // reserved rtrb write, not an application callback.
                        assert!(matches!(
                        root.0.bank.try_lock(),
                        Err(crate::candidate_custody::custody_mutex::CustodyTryLockError::Contended)
                    ));
                        assert!(matches!(
                        custody.bank.try_lock(),
                        Err(crate::candidate_custody::custody_mutex::CustodyTryLockError::Contended)
                    ));
                        assert!(matches!(
                        custody.work.try_lock(),
                        Err(crate::candidate_custody::custody_mutex::CustodyTryLockError::Contended)
                    ));
                        queued = Some(value);
                    },
                );
            if cancel_before {
                assert!(result.is_err());
                assert!(queued.is_none());
                let mut work = lock(&custody.work);
                let owner = work.publication.preparations[token.slot]
                    .as_mut()
                    .unwrap()
                    .cell
                    .as_mut()
                    .unwrap()
                    .owner_on_control()
                    .unwrap();
                assert_eq!(Some(owner.identity()), identity);
                assert_eq!(
                    owner.phase(),
                    hex_engine::AudioCandidateDisposalPhase::Owned
                );
                drop(work);
                std::thread::scope(|scope| {
                    scope.spawn(|| root.advance_on_control()).join().unwrap();
                });
            } else {
                let ticket = result.unwrap();
                // Once accepted, constructor cancellation cannot retake this
                // owner; the existing publication/return protocol owns it.
                assert!(!custody.cancel_runtime_preparation(token));
                custody
                    .ingest_publication(&root, queued.unwrap().reject_before_install())
                    .ok()
                    .unwrap();
                std::thread::scope(|scope| {
                    scope.spawn(|| root.advance_on_control()).join().unwrap();
                });
                let returned = custody.recover_publication(&ticket).unwrap();
                assert_eq!(Some(returned.disposed.identity()), identity);
                assert!(returned.inputs.owning_handle_count() > 0);
                assert!(custody.recover_publication(&ticket).is_none());
            }
            assert!(custody.complete());
        }
    }
    #[test]
    #[cfg(feature = "allocation-probe")]
    fn preallocated_split_transfer_allocates_and_frees_nothing_and_preserves_runtime_storage() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported split allocation constructor; no positive transfer proof");
            return;
        }
        eprintln!(
            "SUPPORTED preallocated constructor split allocation probe; queue/native admission remains separate"
        );
        let (project, asset, audio) = project();
        let mut cell =
            hex_engine::AudioCandidatePreparation::try_new_on_control(1, reservation().storage)
                .unwrap();
        // Historical audio-clip fixture only, not Sampler original-PCM loading.
        cell.copy_resident_source_on_control(asset, &audio).unwrap();
        cell.prepare_on_control(
            &project,
            11,
            hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 7,
            },
        )
        .unwrap();
        let identity = cell.owner_on_control().unwrap().identity();
        let mut prepared = cell.preallocate_publication_split_on_control().unwrap();
        prepared
            .seal_constructor_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 4096,
                    handle_rows: 2048,
                    root_rows: 4,
                    max_retained_bytes: 64 * 1024 * 1024,
                    max_construction_peak_bytes: 64 * 1024 * 1024,
                },
                0,
            )
            .unwrap();
        let mut result = None;
        let measured = allocation_counter::measure(|| {
            result = Some(prepared.transfer_on_control().unwrap());
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
        let (mut runtime, mut anchor) = result.unwrap();
        assert_eq!(runtime.identity(), identity);
        assert!(anchor.matches_current(&runtime));
        assert!(cell.owner_on_control().is_none());
        runtime
            .apply_action(hex_engine::PlaybackAction::Transport(
                hex_engine::PlaybackCommand::Play,
            ))
            .unwrap();
        let mut output = [[0.0; 2]; 8];
        runtime
            .render_with_post_node_observer(&mut output, None)
            .unwrap();
        assert_eq!(output, [[0.25; 2]; 8]);
        assert!(anchor.recover_inputs_on_control().is_none());
        let proof = runtime.dispose_on_control().unwrap();
        let mut anchors = [Some(anchor)];
        proof.record_anchor_cells_on_control(&mut anchors).unwrap();
        let anchor = anchors[0].as_mut().unwrap();
        assert!(
            matches!(
                anchor.finish_disposal_on_control(),
                Err(hex_engine::AudioStorageError::ExternalReadersPending)
            ),
            "actual proof still owns its constructor group-book backing"
        );
        drop(proof); // Same actual proof, only after complete origin fanout.
        assert!(
            matches!(
                anchor.finish_disposal_on_control(),
                Err(hex_engine::AudioStorageError::ExternalReadersPending)
            ),
            "actual claimed Box remains separate from DSP/proof destruction"
        );
        drop(runtime);
        anchor.finish_disposal_on_control().unwrap();
        drop(anchor.recover_inputs_on_control().unwrap());
        cell.cleanup_on_control().unwrap();
    }
    #[test]
    fn registered_compiled_reader_keeps_closed_preparation_slot_charged_until_release() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported constructor layout; no positive compiled-reader custody proof");
            return;
        }
        let root = root();
        let custody = &root.0.eligible;
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let held = lock(&custody.bank).publication.retained_bytes().unwrap();
        let (project, asset, audio) = project();
        let mut reader = None;
        custody
            .with_runtime_preparation(token, |cell| {
                cell.copy_resident_source_on_control(asset, &audio)?;
                cell.prepare_on_control(
                    &project,
                    11,
                    hex_engine::AudioSourceContext {
                        project_generation: 2,
                        editor_revision: 8,
                    },
                )?;
                reader = Some(cell.register_compiled_reader_on_control()?);
                Ok(())
            })
            .unwrap();
        assert!(custody.cancel_runtime_preparation(token));
        for _ in 0..2 {
            std::thread::scope(|scope| {
                scope.spawn(|| root.advance_on_control()).join().unwrap();
            });
            let bank = lock(&custody.bank);
            assert_eq!(
                bank.publication.slots[token.slot].incarnation,
                token.incarnation
            );
            assert_eq!(bank.publication.slots[token.slot].phase, Phase::Reserved);
            assert_eq!(bank.publication.retained_bytes(), Some(held));
            drop(bank);
            let work = lock(&custody.work);
            let execution = work.publication.preparations[token.slot].as_ref().unwrap();
            assert!(!execution.pending && !execution.failed);
            assert!(!execution.cell.as_ref().unwrap().failed_incomplete());
        }
        assert!(reader.as_ref().unwrap().try_duplicate_on_control().is_err());
        drop(reader.take());
        std::thread::scope(|scope| {
            scope.spawn(|| root.advance_on_control()).join().unwrap();
        });
        assert!(custody.complete());
        assert_eq!(lock(&custody.bank).publication.retained_bytes(), Some(0));
        assert!(lock(&custody.work).publication.preparations[token.slot].is_none());
        assert!(custody.with_runtime_preparation(token, |_| Ok(())).is_err());
        eprintln!(
            "SUPPORTED actual registered compiled reader; same preparation slot/charge retained twice; normal release cleanup completed"
        );
    }

    #[test]
    fn reserved_slot_incarnation_survives_actual_preparation_close_and_normal_cleanup() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let root = root();
        let custody = &root.0.eligible;
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let held = lock(&custody.bank).publication.retained_bytes().unwrap();
        assert!(held >= reservation().storage.max_prepared_data_bytes);
        assert!(
            custody
                .reserve_runtime_preparation(reservation(), Vec::new())
                .is_err()
        );
        let (project, asset, audio) = project();
        assert!(
            custody
                .with_runtime_preparation(
                    AudioCandidatePreparationToken {
                        incarnation: token.incarnation + 1,
                        ..token
                    },
                    |_| panic!("stale closure ran")
                )
                .is_err()
        );
        assert_eq!(
            custody
                .with_runtime_preparation(token, |cell| {
                    cell.copy_resident_source_on_control(asset, &audio)?;
                    cell.prepare_on_control(
                        &project,
                        11,
                        hex_engine::AudioSourceContext {
                            project_generation: 2,
                            editor_revision: 8,
                        },
                    )?;
                    custody.close(); // closure wins after constructing the actual owner
                    Ok(())
                })
                .unwrap(),
            AudioCandidatePreparationProgress::Retiring
        );
        {
            let bank = lock(&custody.bank);
            assert_eq!(
                bank.publication.slots[token.slot].incarnation,
                token.incarnation
            );
            assert_eq!(bank.publication.slots[token.slot].phase, Phase::Reserved);
            assert_eq!(bank.publication.retained_bytes(), Some(held));
        }
        std::thread::scope(|scope| {
            scope
                .spawn(|| custody.advance_publications_on_control(&root, false))
                .join()
                .unwrap();
        });
        assert!(custody.complete());
        assert_eq!(lock(&custody.bank).publication.retained_bytes(), Some(0));
        assert!(lock(&custody.work).publication.preparations[token.slot].is_none());
        assert!(custody.with_runtime_preparation(token, |_| Ok(())).is_err());
    }
    #[test]
    fn cancellation_and_pressure_preserve_slot_until_partial_sources_and_wrappers_are_destroyed() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let root = root();
        let custody = &root.0.eligible;
        let mut excessive = reservation();
        excessive.storage.max_prepared_data_bytes = usize::MAX;
        excessive.storage.max_construction_data_bytes = usize::MAX;
        assert!(
            custody
                .reserve_runtime_preparation(excessive, Vec::new())
                .is_err()
        );
        assert_eq!(lock(&custody.bank).publication.occupied(), Some(0));
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let (_, asset, audio) = project();
        assert_eq!(
            custody
                .with_runtime_preparation(token, |cell| cell
                    .copy_resident_source_on_control(asset, &audio))
                .unwrap(),
            AudioCandidatePreparationProgress::Collecting
        );
        assert!(custody.cancel_runtime_preparation(token));
        assert_eq!(lock(&custody.bank).publication.occupied(), Some(1));
        assert_eq!(
            custody
                .with_runtime_preparation(token, |_| panic!("cancelled closure ran"))
                .unwrap(),
            AudioCandidatePreparationProgress::Retiring
        );
        custody.advance_publications_on_control(&root, false);
        assert_eq!(lock(&custody.bank).publication.occupied(), Some(0));
        let reused = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        assert_eq!(reused.slot, token.slot);
        assert!(reused.incarnation > token.incarnation);
        assert!(!custody.cancel_runtime_preparation(token));
        assert!(custody.cancel_runtime_preparation(reused));
        custody.advance_publications_on_control(&root, false);
        assert!(custody.complete());
    }
    #[test]
    fn actual_preparation_refusal_remains_distinct_from_retiring_until_cleanup() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let root = root();
        let custody = &root.0.eligible;
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let (project, asset, audio) = project();
        let result = custody
            .with_runtime_preparation(token, |cell| {
                cell.copy_resident_source_on_control(asset, &audio)?;
                // An invalid source generation is a real constructor refusal, not
                // asynchronous Busy or a successful no-install publication.
                cell.prepare_on_control(
                    &project,
                    0,
                    hex_engine::AudioSourceContext {
                        project_generation: 2,
                        editor_revision: 8,
                    },
                )
            })
            .unwrap();
        assert_eq!(
            result,
            AudioCandidatePreparationProgress::Refused("candidate preparation refused")
        );
        assert!(lock(&custody.bank).publication.slots[token.slot].preparation_retiring);
        assert_eq!(
            custody
                .with_runtime_preparation(token, |_| panic!("refused preparation retried"))
                .unwrap(),
            result
        );
        assert_eq!(lock(&custody.bank).publication.occupied(), Some(1));
        custody.advance_publications_on_control(&root, false);
        assert!(custody.complete());
    }
    #[test]
    fn graph_reservation_precedes_source_start_and_one_shot_binding_holds_pair_until_cleanup() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let root = root();
        let custody = &root.0.eligible;
        let graph = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let domain = custody
            .begin_preparation(
                hex_stream::StreamConfig::with_pool_pages(4),
                Vec::new(),
                hex_stream::StreamReadinessReservation {
                    extents_per_role: 2,
                },
            )
            .ok()
            .unwrap();
        let wanted = Vec::from([domain]);
        let ptr = wanted.as_ptr();
        let cap = wanted.capacity();
        // Wrong incarnation refuses before association/counter changes.
        let refused = custody
            .bind_runtime_preparation_domains(
                AudioCandidatePreparationToken {
                    incarnation: graph.incarnation + 1,
                    ..graph
                },
                wanted,
            )
            .err()
            .unwrap();
        assert_eq!(
            (refused.domains.as_ptr(), refused.domains.capacity()),
            (ptr, cap)
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let pages = loop {
            let (progress, pages) = custody.poll_preparation(domain).unwrap();
            if progress == AudioCandidateDomainPreparationProgress::Ready {
                break pages.unwrap();
            }
            assert!(pages.is_none());
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        custody
            .bind_runtime_preparation_domains(graph, refused.domains)
            .ok()
            .unwrap();
        assert_eq!(lock(&custody.bank).domains[domain.slot].uses, 1);
        assert!(
            custody
                .bind_runtime_preparation_domains(graph, Vec::from([domain]))
                .is_err()
        );
        assert!(custody.retire_domain(domain));
        assert!(custody.ingest_pages(pages).is_ok());
        custody.advance_on_control(false);
        assert_eq!(
            lock(&custody.bank).domains[domain.slot].phase,
            Phase::Accepted
        );
        assert!(custody.cancel_runtime_preparation(graph));
        custody.advance_publications_on_control(&root, false);
        assert_eq!(lock(&custody.bank).domains[domain.slot].uses, 0);
        let report = loop {
            custody.advance_on_control(false);
            if let Some(report) = custody.recover_domain(domain) {
                break report;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert!(report.accounting_complete);
        assert!(custody.complete());
    }
    #[test]
    fn partial_construction_unwind_keeps_actual_cell_charge_and_first_payload_without_retry() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        use std::sync::atomic::AtomicUsize;
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let root = root();
        let custody = &root.0.eligible;
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let (_, asset, audio) = project();
        let drops = Arc::new(AtomicUsize::new(0));
        let payload = drops.clone();
        assert!(matches!(
            custody
                .with_runtime_preparation(token, |cell| {
                    cell.copy_resident_source_on_control(asset, &audio)?;
                    std::panic::panic_any(Payload(payload));
                })
                .unwrap(),
            AudioCandidatePreparationProgress::Refused(_)
        ));
        let held = lock(&custody.bank).publication.retained_bytes().unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        for _ in 0..2 {
            custody.advance_publications_on_control(&root, true);
        }
        assert_eq!(lock(&custody.bank).publication.retained_bytes(), Some(held));
        assert_eq!(
            lock(&custody.bank).publication.slots[token.slot].phase,
            Phase::Failed
        );
        assert!(
            lock(&custody.work).publication.preparations[token.slot]
                .as_ref()
                .unwrap()
                .cell
                .is_some()
        );
        assert!(
            custody
                .with_runtime_preparation(token, |_| panic!("failed closure retried"))
                .is_err()
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
}
