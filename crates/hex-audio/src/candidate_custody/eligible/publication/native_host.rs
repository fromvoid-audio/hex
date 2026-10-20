//! Private five-plus-one immutable inspection facade. No field, owner or approval leaves Audio.
use super::*;
use crate::candidate_custody::{AbandonedReturns, Bank as OuterBank, Work as OuterWork};
use hex_engine::{
    AudioStorageError,
    sampler_host_fields::{
        BodyRef, NativeFields, NativePreparationFields, NativeRuntimeFields, SharedMetadataRef,
        VecRef,
    },
};

pub(super) struct NativePreparationHost<'a> {
    root: &'a CandidateRoot,
    outer_bank: CustodyMutexGuard<'a, OuterBank>,
    outer_work: CustodyMutexGuard<'a, OuterWork>,
    returns: CustodyMutexGuard<'a, Option<AbandonedReturns>>,
    work: CustodyMutexGuard<'a, NativeWork>,
    bank: CustodyMutexGuard<'a, NativeBank>,
    resident: Option<crate::prepared::resident_forwarding::ResidentRead<'a>>,
    token: AudioCandidatePreparationToken,
    selected_address: usize,
}
impl CandidateRoot {
    /// Whole-host original construction, not a callback or publication loan.
    /// Actual Session/family writers are acquired by the caller before Audio.
    #[expect(clippy::too_many_arguments, reason = "finite real host operands")]
    pub(crate) fn construct_original_sampler_in_host(
        &self,
        token: AudioCandidatePreparationToken,
        worker: &hex_engine::sampler_host_fields::SamplerWorkerFields<'_>,
        source: &hex_engine::sampler_host_fields::CapturedSourceFields<'_>,
        files: &dyn hex_engine::sampler_host_fields::FileSlotsFields<'_>,
        families: &dyn hex_engine::sampler_host_fields::SamplerFamilyInventoryFields,
        resident: &dyn hex_engine::sampler_host_fields::OriginalSamplerResidentFields,
        producer: &hex_engine::native_device_resources::OriginalResourceBankPlan<'_>,
        previous: Option<&hex_engine::AudioCompiledRevision>,
        original: &hex_engine::AudioRootStorageLedger<'_>,
        writers: &mut hex_engine::audio_transform::OriginalSamplerTransaction<'_>,
        reservation: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<(), AudioStorageError> {
        let mut host = NativePreparationHost::acquire(self, token)?;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            hex_engine::sampler_host_fields::construct_original_sampler_native_with_host_on_control(
                &mut host,
                worker,
                source,
                files,
                families,
                resident,
                producer,
                previous,
                original,
                writers,
                reservation,
            )
        }));
        self.finish_original_sampler_host(host, token, outcome)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "finite actual pending host operands"
    )]
    pub(crate) fn construct_pending_original_sampler_in_host<
        'owner,
        P: hex_engine::sampler_host_fields::OriginalSamplerPendingSession<'owner>,
    >(
        &self,
        token: AudioCandidatePreparationToken,
        worker: &hex_engine::sampler_host_fields::SamplerWorkerFields<'_>,
        current_source: &hex_engine::sampler_host_fields::CapturedSourceFields<'_>,
        files: &dyn hex_engine::sampler_host_fields::FileSlotsFields<'_>,
        families: &dyn hex_engine::sampler_host_fields::SamplerFamilyInventoryFields,
        session: &mut P,
        previous: Option<&hex_engine::AudioCompiledRevision>,
        reservation: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<bool, AudioStorageError> {
        let mut host = NativePreparationHost::acquire(self, token)?;
        let mut changed = false;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            changed = hex_engine::sampler_host_fields::construct_pending_original_sampler_native_on_control(
                &mut host, worker, current_source, files, families, session, previous, reservation,
            )?;
            Ok(())
        }));
        self.finish_original_sampler_host(host, token, outcome)?;
        Ok(changed)
    }
    fn finish_original_sampler_host(
        &self,
        mut host: NativePreparationHost<'_>,
        token: AudioCandidatePreparationToken,
        outcome: Result<Result<(), AudioStorageError>, Box<dyn std::any::Any + Send>>,
    ) -> Result<(), AudioStorageError> {
        let mut secondary_payload = None;
        let result = match outcome {
            Ok(result) => {
                if result.is_err() {
                    let execution = host.work.publication.preparations[token.slot]
                        .as_mut()
                        .ok_or(AudioStorageError::SourceMismatch)?;
                    execution.refusal = Some("candidate preparation refused");
                    host.bank.publication.slots[token.slot].preparation_retiring = true;
                    if execution
                        .cell
                        .as_ref()
                        .is_some_and(|cell| cell.failed_incomplete())
                    {
                        execution.failed = true;
                        host.bank.publication.slots[token.slot].phase = Phase::Failed;
                        self.0.eligible.mark_closing(&mut host.bank);
                    }
                }
                result
            }
            Err(payload) => {
                // SAME persistent cell and first payload remain in this Root.
                // Consumed failed custody can never be retried as a fresh cell.
                let execution = host.work.publication.preparations[token.slot]
                    .as_mut()
                    .ok_or(AudioStorageError::SourceMismatch)?;
                execution.pending = true;
                execution.failed = true;
                if host.work.publication.failed_payloads[token.slot].is_none() {
                    host.work.publication.failed_payloads[token.slot] = Some(payload);
                } else {
                    secondary_payload = Some(payload);
                }
                host.bank.publication.slots[token.slot].phase = Phase::Failed;
                self.0.eligible.mark_closing(&mut host.bank);
                Err(AudioStorageError::UnknownStorage)
            }
        };
        // No structural cleanup, backend work or owning destructor runs under
        // these exclusions. The existing lifecycle owns later retirement.
        drop(host);
        crate::lifecycle::request_candidate_cleanup(self);
        drop(secondary_payload);
        result
    }
}
impl<'a> NativePreparationHost<'a> {
    pub(super) fn acquire(
        root: &'a CandidateRoot,
        token: AudioCandidatePreparationToken,
    ) -> Result<Self, AudioStorageError> {
        // EVERY acquisition is one try. In particular no bank/control recovery
        // can wait while another actual exclusion is retained.
        let busy = |reason| match reason {
            super::super::super::custody_mutex::CustodyTryLockError::Contended => {
                AudioStorageError::ExternalReadersPending
            }
            super::super::super::custody_mutex::CustodyTryLockError::Poisoned => {
                AudioStorageError::UnknownStorage
            }
        };
        let outer_bank = root.0.bank.try_lock().map_err(busy)?;
        let outer_work = root.0.execution.try_lock().map_err(busy)?;
        let returns = root.0.returns.try_lock().map_err(busy)?;
        let work = root.0.eligible.work.try_lock().map_err(busy)?;
        let bank = root.0.eligible.bank.try_lock().map_err(busy)?;
        // The inspection-only sixth exclusion is ALWAYS last. Resident mutator
        // paths end their Slot loan before touching any Root exclusion.
        let resident = root
            .0
            .resident
            .as_ref()
            .map(|archive| archive.try_read_on_control())
            .transpose()?;
        let mut host = Self {
            root,
            outer_bank,
            outer_work,
            returns,
            work,
            bank,
            resident,
            token,
            selected_address: 0,
        };
        host.validate_selection()?;
        host.selected_address = std::ptr::from_ref(host.selected()?).addr();
        Ok(host)
    }
    fn validate_selection(&self) -> Result<(), AudioStorageError> {
        let token = self.token;
        let slot = self
            .bank
            .publication
            .slots
            .get(token.slot)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let execution = self
            .work
            .publication
            .preparations
            .get(token.slot)
            .and_then(Option::as_ref)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if self.root.root_id() != token.root
            || self.outer_bank.root != token.root
            || self.bank.root != token.root
            || self.bank.closing
            || self.outer_bank.closing
            || self.outer_bank.retiring
            || self.root.0.abandoned.load(Ordering::Acquire)
            || slot.incarnation != token.incarnation
            || slot.phase != Phase::Reserved
            || slot.preparation_reservation.is_none()
            || slot.preparation_retiring
            || execution.token != token
            || execution.pending
            || execution.failed
            || execution.refusal.is_some()
            || execution.cell.is_none()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    fn selected(&self) -> Result<&hex_engine::AudioCandidatePreparation, AudioStorageError> {
        self.work.publication.preparations[self.token.slot]
            .as_ref()
            .and_then(|p| p.cell.as_ref())
            .ok_or(AudioStorageError::SourceMismatch)
    }
    /// Borrow checker requires EVERY inspection/ledger loan to end before this
    /// mutable reborrow; guards stay in this same facade. Address equality is
    /// correlation only, never dereferenced or exported as construction proof.
    fn selected_mut(
        &mut self,
    ) -> Result<&mut hex_engine::AudioCandidatePreparation, AudioStorageError> {
        self.validate_selection()?;
        let selected = self.work.publication.preparations[self.token.slot]
            .as_mut()
            .and_then(|p| p.cell.as_mut())
            .ok_or(AudioStorageError::SourceMismatch)?;
        if std::ptr::from_ref(selected).addr() != self.selected_address {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(selected)
    }
}
impl hex_engine::sampler_host_fields::OriginalSamplerNativeHost for NativePreparationHost<'_> {
    fn construction_frame(&self) -> BodyRef<'_> {
        BodyRef::of(self)
    }
    fn selected_preparation_on_control(
        &self,
    ) -> Result<&hex_engine::AudioCandidatePreparation, AudioStorageError> {
        self.validate_selection()?;
        self.selected()
    }
    fn selected_operations_on_control(
        &mut self,
    ) -> Result<hex_engine::AudioCandidatePreparationOperations<'_>, AudioStorageError> {
        // selected_mut repeats root/slot/incarnation/closing and SAME-address
        // checks while all original exclusions remain held. It exposes no
        // mutable owner/preparation to the Engine driver or its caller.
        Ok(self.selected_mut()?.operations_on_control())
    }
}
impl NativeFields for NativePreparationHost<'_> {
    fn root(&self) -> SharedMetadataRef<'_> {
        SharedMetadataRef::of(&self.root.0)
    }
    fn guarded_bodies(&self) -> [BodyRef<'_>; 5] {
        [
            BodyRef::of(&*self.outer_bank),
            BodyRef::of(&*self.outer_work),
            BodyRef::of(&*self.returns),
            BodyRef::of(&*self.bank),
            BodyRef::of(&*self.work),
        ]
    }
    fn outer_slots(&self) -> VecRef<'_> {
        VecRef::of(&self.outer_bank.slots)
    }
    fn outer_slot(&self, index: usize) -> Result<BodyRef<'_>, AudioStorageError> {
        let slot = self
            .outer_bank
            .slots
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let Slot {
            incarnation: _,
            identity,
            phase,
            catalog,
            returned,
            ready,
            domain_count,
        } = slot;
        if *phase != Phase::Vacant
            || identity.is_some()
            || catalog.is_some()
            || returned.is_some()
            || ready.is_some()
            || *domain_count != 0
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(BodyRef::of(slot))
    }
    fn root_fields(&self) -> Option<&hex_engine::sampler_host_fields::NativeRootFieldRegistration> {
        self.root.0.root_fields.as_ref()
    }
    fn spectrum_packets(
        &self,
    ) -> &[Option<hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration>] {
        &self.bank.spectrum_packets
    }
    fn spectrum_results(
        &self,
    ) -> &[Option<hex_engine::sampler_host_fields::NativeSpectrumResultRegistration>] {
        &self.bank.spectrum_results
    }
    fn spectrum_stop(
        &self,
    ) -> Option<&hex_engine::sampler_host_fields::NativeAtomicSignalRegistration> {
        self.bank.spectrum_stop.as_ref()
    }
    fn spectrum_rings(
        &self,
    ) -> [Option<&hex_engine::sampler_host_fields::NativeFixedRingRegistration>; 6] {
        self.bank.spectrum_rings.each_ref().map(Option::as_ref)
    }
    fn spectrum_capture(
        &self,
    ) -> Option<&hex_engine::sampler_host_fields::NativeSpectrumCapturedRegistration> {
        self.bank.spectrum_capture.as_ref()
    }
    fn carriers(
        &self,
    ) -> [Option<&hex_engine::sampler_host_fields::NativeCarrierStorageRegistration>; 4] {
        self.bank.carriers.each_ref().map(Option::as_ref)
    }
    fn prepared_rings(
        &self,
    ) -> [Option<&hex_engine::sampler_host_fields::NativeFixedRingRegistration>; 9] {
        self.bank.prepared_rings.each_ref().map(Option::as_ref)
    }
    fn atomic_signals(
        &self,
    ) -> [Option<&hex_engine::sampler_host_fields::NativeAtomicSignalRegistration>; 8] {
        self.bank.atomic_signals.each_ref().map(Option::as_ref)
    }
    fn status(
        &self,
    ) -> Result<Option<hex_engine::sampler_host_fields::NativeStatusFields<'_>>, AudioStorageError>
    {
        self.bank
            .status
            .as_ref()
            .map(|status| status.fields())
            .transpose()
    }
    fn resident(
        &self,
    ) -> Result<Option<hex_engine::sampler_host_fields::NativeResidentFields<'_>>, AudioStorageError>
    {
        self.resident.as_ref().map(|read| read.fields()).transpose()
    }
    fn domain_signals(&self) -> Result<[SharedMetadataRef<'_>; 2], AudioStorageError> {
        for domain in &self.bank.domains {
            let DomainSlot {
                incarnation: _,
                owner,
                preparing,
                preparation_bound,
                preparation_failure,
                starting,
                phase,
                uses,
                retiring,
                ready,
                data,
                peak,
                resources,
                signal: _,
                loan_outstanding,
                returned_pages,
            } = domain;
            if *phase != Phase::Vacant
                || owner.is_some()
                || preparing.is_some()
                || preparation_bound.is_some()
                || preparation_failure.is_some()
                || *starting
                || *uses != 0
                || *retiring
                || ready.is_some()
                || *data != 0
                || *peak != 0
                || resources.is_some()
                || *loan_outstanding
                || returned_pages.is_some()
            {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        Ok([
            SharedMetadataRef::of(&self.bank.domains[0].signal),
            SharedMetadataRef::of(&self.bank.domains[1].signal),
        ])
    }
    fn runtime_slots(&self) -> VecRef<'_> {
        VecRef::of(&self.bank.publication.slots)
    }
    fn runtime(&self, index: usize) -> Result<NativeRuntimeFields<'_>, AudioStorageError> {
        let slot = self
            .bank
            .publication
            .slots
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let RuntimeSlot {
            incarnation: _,
            identity,
            preparation_reservation: _,
            preparation_retiring,
            preparation_metadata_pending,
            phase,
            archive: _,
            life,
            anchors,
            state_storage,
            claim_storage,
            domains,
            domain_refs,
            streaming,
            summary,
            context_generation: _,
            policy,
            retained: _,
            peak: _,
            returned,
            ready,
        } = slot;
        if *preparation_retiring
            || *preparation_metadata_pending
            || streaming.is_some()
            || returned.is_some()
            || ready.is_some()
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        let transferred = match phase {
            Phase::Vacant | Phase::Reserved => {
                if identity.is_some()
                    || anchors.is_some()
                    || state_storage.is_some()
                    || domain_refs.iter().any(Option::is_some)
                    || summary.is_some()
                    || policy.is_some()
                    || life.live.load(Ordering::Acquire)
                {
                    return Err(AudioStorageError::UnknownStorage);
                }
                None
            }
            Phase::Accepted => {
                let anchors = anchors.as_ref().ok_or(AudioStorageError::UnknownStorage)?;
                let state = state_storage
                    .as_ref()
                    .ok_or(AudioStorageError::UnknownStorage)?;
                if *identity != Some(anchors.identity())
                    || !life.live.load(Ordering::Acquire)
                    || life.incarnation.load(Ordering::Acquire) != slot.incarnation
                    || summary.is_none()
                    || policy.is_none()
                    || *domain_refs != std::array::from_fn(|i| domains.get(i).copied())
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                Some(
                    hex_engine::sampler_host_fields::NativeTransferredRuntimeFields {
                        anchors,
                        state,
                    },
                )
            }
            _ => return Err(AudioStorageError::UnknownStorage),
        };
        Ok(NativeRuntimeFields {
            body: BodyRef::of(slot),
            life: SharedMetadataRef::of(life),
            domains: VecRef::of(domains),
            claims: std::array::from_fn(|i| claim_storage[i].as_ref()),
            transferred,
        })
    }
    fn archives(&self) -> VecRef<'_> {
        VecRef::of(&self.bank.publication.archives)
    }
    fn work_backings(&self) -> [VecRef<'_>; 5] {
        let Work {
            preparations,
            anchors,
            executions,
            failed_payloads,
            banks,
        } = &self.work.publication;
        [
            VecRef::of(preparations),
            VecRef::of(anchors),
            VecRef::of(executions),
            VecRef::of(failed_payloads),
            VecRef::of(banks),
        ]
    }
    fn preparation(
        &self,
        index: usize,
    ) -> Result<Option<NativePreparationFields<'_>>, AudioStorageError> {
        let execution = self
            .work
            .publication
            .preparations
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let Some(execution) = execution else {
            return Ok(None);
        };
        let preparation::PreparationExecution {
            token,
            pending,
            failed,
            refusal,
            cell,
            domains,
        } = execution;
        if *pending
            || *failed
            || refusal.is_some()
            || token.root != self.token.root
            || token.slot != index
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        let slot = self
            .bank
            .publication
            .slots
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if slot.incarnation != token.incarnation || slot.phase != Phase::Reserved {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(Some(NativePreparationFields {
            body: BodyRef::of(execution),
            domains: VecRef::of(domains),
            cell: cell.as_ref().ok_or(AudioStorageError::UnknownStorage)?,
        }))
    }
    fn validate_remaining_children(&self) -> Result<(), AudioStorageError> {
        self.validate_selection()?;
        // Exhaustive owning-field patterns make additions a review boundary;
        // a body footprint never substitutes for its descendant schema.
        let crate::candidate_custody::Inner {
            root: _,
            eligible: _,
            bank: _,
            execution: _,
            abandoned: _,
            returns: _,
            resident,
            carrier_birth: _,
            root_fields: _,
        } = &*self.root.0;
        let Custody {
            enabled: _,
            closed: _,
            bank: _,
            work: _,
        } = &self.root.0.eligible;
        let OuterBank {
            root: _,
            closing: _,
            slots: _,
            scheduled: _,
            retiring: _,
        } = &*self.outer_bank;
        let NativeBank {
            root: _,
            reservation: _,
            domains: _,
            metadata_bytes: _,
            closing: _,
            publication: _,
            native,
            prepared_rings: _,
            atomic_signals: _,
            status: _,
            carriers: _,
            spectrum_capture: _,
            spectrum_rings: _,
            spectrum_stop: _,
            spectrum_packets: _,
            spectrum_results: _,
        } = &*self.bank;
        let Bank {
            slots: _,
            archives: _,
            reservation: _,
            metadata_bytes: _,
            stream_extents: _,
        } = &self.bank.publication;
        let NativeWork {
            domains,
            publication: _,
        } = &*self.work;
        let OuterWork {
            current,
            bank_drop,
            bank_drop_pending,
            metadata_drop_pending,
            first_panic,
            #[cfg(test)]
                last_thread: _,
        } = &*self.outer_work;
        if let Some(native) = native {
            // The registered factory inspected actual empty owning callback
            // fields before carrier escape. Existing constructor-mode fences
            // then prohibit legacy Playback, preview and resident ingress.
            // Every allowed queued/deferred/active/held Eligible State remains
            // in the SAME Root registrations, visited independently below.
            // The boundary's old scalar DATA/resource report is NOT inventory.
            if !native.matches(self.root.root_id())
                || self.root.0.carrier_birth.get().is_none()
                || self.root.0.root_fields.is_none()
                || self.root.0.resident.is_none()
                || self.bank.carriers.iter().any(Option::is_none)
                || self.bank.prepared_rings.iter().any(Option::is_none)
                || self.bank.atomic_signals.iter().any(Option::is_none)
                || !self
                    .bank
                    .status
                    .as_ref()
                    .is_some_and(|status| status.constructor_inputs_sealed())
            {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        if (self.root.0.carrier_birth.get().is_some() && native.is_none())
            || resident.is_some() != self.resident.is_some()
            || current.is_some()
            || bank_drop.is_some()
            || *bank_drop_pending
            || *metadata_drop_pending
            || first_panic.is_some()
            || self.returns.is_some()
            || domains.iter().any(Option::is_some)
            || self.work.publication.anchors.iter().any(Option::is_some)
            || self.work.publication.executions.iter().any(Option::is_some)
            || self
                .work
                .publication
                .failed_payloads
                .iter()
                .any(Option::is_some)
            || self.work.publication.banks.iter().any(Option::is_some)
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
